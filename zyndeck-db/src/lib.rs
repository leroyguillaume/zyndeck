//! Database access layer for Zyndeck.
//!
//! Owns the PostgreSQL connection pool and the embedded migrations the rest of
//! the workspace runs against. Rule embeddings live in Postgres via pgvector,
//! so the migrations enable the `vector` extension.

mod config;
mod error;
mod game;
mod ingestion_job;
mod ingestion_step_run;
mod ingestion_transcript;
mod user;

pub use config::DbConfig;
pub use error::{Error, Result};
pub use game::{GameRepository, GameUpdate, NewGame, PgGameRepository};
pub use ingestion_job::{IngestionJobRepository, NewIngestionJob, PgIngestionJobRepository};
pub use ingestion_step_run::{
    IngestionStepRunRepository, PgIngestionStepRunRepository, StepOutcome,
};
pub use ingestion_transcript::{IngestionTranscriptRepository, PgIngestionTranscriptRepository};
pub use user::{Credentials, NewUser, PgUserRepository, UserRepository, UserUpdate};

#[cfg(feature = "mock")]
pub use game::MockGameRepository;
#[cfg(feature = "mock")]
pub use ingestion_job::MockIngestionJobRepository;
#[cfg(feature = "mock")]
pub use ingestion_step_run::MockIngestionStepRunRepository;
#[cfg(feature = "mock")]
pub use ingestion_transcript::MockIngestionTranscriptRepository;
#[cfg(feature = "mock")]
pub use user::MockUserRepository;

use ingestion_job::{IngestionJobRow, map_create_error};
use ingestion_step_run::{IngestionStepRunRow, map_begin_error};
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;
use zyndeck_core::{IngestionJob, IngestionStep, IngestionStepRun};

/// Embedded SQL migrations, applied in order by [`Db::migrate`].
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Outcome of advancing a job past its current (succeeded) step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Advanced {
    /// The next step's run has begun; execute it, then finish the run.
    Running(IngestionStepRun),
    /// There was no next step — the job is now complete.
    Completed,
}

/// Handle to the database: a cheaply-cloneable wrapper around a [`PgPool`].
///
/// The pool is reference-counted, so clone `Db` to share it across tasks rather
/// than opening multiple pools.
#[derive(Debug, Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Opens a connection pool from `config` without touching the schema.
    pub async fn connect(config: &DbConfig) -> Result<Self> {
        tracing::debug!(
            max_connections = config.max_connections,
            "connecting to database"
        );
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(&config.url)
            .await
            .map_err(Error::Connect)?;
        tracing::debug!("database connection pool established");
        Ok(Self::new(pool))
    }

    /// Wraps an existing pool — useful for composition and tests.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Applies any outstanding migrations. Idempotent: migrations already
    /// recorded as applied are skipped.
    pub async fn migrate(&self) -> Result<()> {
        tracing::debug!("running database migrations");
        MIGRATOR.run(&self.pool).await.map_err(Error::Migrate)?;
        tracing::debug!("database migrations up to date");
        Ok(())
    }

    /// Borrows the underlying pool for issuing queries.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Returns a [`GameRepository`] backed by this database's pool.
    pub fn games(&self) -> PgGameRepository {
        PgGameRepository::new(self.pool.clone())
    }

    /// Returns a [`UserRepository`] backed by this database's pool.
    pub fn users(&self) -> PgUserRepository {
        PgUserRepository::new(self.pool.clone())
    }

    /// Returns an [`IngestionJobRepository`] backed by this database's pool.
    pub fn ingestion_jobs(&self) -> PgIngestionJobRepository {
        PgIngestionJobRepository::new(self.pool.clone())
    }

    /// Returns an [`IngestionStepRunRepository`] backed by this database's pool.
    pub fn step_runs(&self) -> PgIngestionStepRunRepository {
        PgIngestionStepRunRepository::new(self.pool.clone())
    }

    /// Returns an [`IngestionTranscriptRepository`] backed by this database's pool.
    pub fn transcripts(&self) -> PgIngestionTranscriptRepository {
        PgIngestionTranscriptRepository::new(self.pool.clone())
    }

    /// Creates an ingestion job and atomically begins its first step's run,
    /// returning both. The run is `running`; execute the step, then finish it via
    /// [`IngestionStepRunRepository::finish`].
    pub async fn start_job(
        &self,
        new: NewIngestionJob,
    ) -> Result<(IngestionJob, IngestionStepRun)> {
        let mut tx = self.pool.begin().await.map_err(Error::Query)?;

        let job: IngestionJob = sqlx::query_as::<_, IngestionJobRow>(include_str!(
            "../queries/ingestion_job/create.sql"
        ))
        .bind(new.game_id)
        .bind(new.source.to_string_lossy().into_owned())
        .bind(new.language.as_str())
        .bind(new.mode.as_str())
        .bind(new.created_by)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| map_create_error(e, new.game_id))?
        .try_into()?;
        let run = begin_run(&mut tx, job.id, job.step).await?;

        tx.commit().await.map_err(Error::Query)?;
        Ok((job, run))
    }

    /// Advances a job to its next step and begins that step's run.
    ///
    /// Takes a `FOR UPDATE` lock on the job row so the read-check-advance-begin
    /// sequence is atomic with respect to other transitions on the same job —
    /// this, not just the active-run unique index, is what keeps `step` and the
    /// run history consistent under concurrency. The lock is held only for this
    /// short transaction, never while the step itself executes. The current step
    /// must have succeeded; returns [`Advanced::Completed`] if none remains.
    pub async fn continue_job(&self, job_id: Uuid) -> Result<Advanced> {
        let mut tx = self.pool.begin().await.map_err(Error::Query)?;

        let job = lock_job(&mut tx, job_id).await?;
        if job.step.is_completed() {
            return Err(Error::JobCompleted(job_id));
        }

        let current_succeeded = latest_run(&mut tx, job_id)
            .await?
            .is_some_and(|run| run.step == job.step && run.status.is_succeeded());
        if !current_succeeded {
            return Err(Error::StepNotSucceeded {
                job: job_id,
                step: job.step,
            });
        }

        let target = job.step.next();
        sqlx::query(include_str!("../queries/ingestion_job/update_step.sql"))
            .bind(target.as_str())
            .bind(job_id)
            .execute(&mut *tx)
            .await
            .map_err(Error::Query)?;

        let advanced = if target.is_completed() {
            Advanced::Completed
        } else {
            Advanced::Running(begin_run(&mut tx, job_id, target).await?)
        };

        tx.commit().await.map_err(Error::Query)?;
        Ok(advanced)
    }

    /// Re-runs a job's current step as a fresh attempt (incrementing the
    /// attempt counter), under the same `FOR UPDATE` lock as [`Db::continue_job`].
    pub async fn restart_job(&self, job_id: Uuid) -> Result<IngestionStepRun> {
        let mut tx = self.pool.begin().await.map_err(Error::Query)?;

        let job = lock_job(&mut tx, job_id).await?;
        if job.step.is_completed() {
            return Err(Error::JobCompleted(job_id));
        }
        let run = begin_run(&mut tx, job_id, job.step).await?;

        tx.commit().await.map_err(Error::Query)?;
        Ok(run)
    }
}

/// Selects the job row `FOR UPDATE`, locking it for the rest of the transaction.
async fn lock_job(tx: &mut Transaction<'_, Postgres>, job_id: Uuid) -> Result<IngestionJob> {
    sqlx::query_as::<_, IngestionJobRow>(include_str!("../queries/ingestion_job/lock.sql"))
        .bind(job_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(Error::Query)?
        .ok_or(Error::JobNotFound(job_id))?
        .try_into()
}

/// Inserts a `running` run for `step` within the transaction; a unique-violation
/// (another active run) maps to [`Error::JobAlreadyRunning`].
async fn begin_run(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
    step: IngestionStep,
) -> Result<IngestionStepRun> {
    sqlx::query_as::<_, IngestionStepRunRow>(include_str!(
        "../queries/ingestion_step_run/begin.sql"
    ))
    .bind(job_id)
    .bind(step.as_str())
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| map_begin_error(e, job_id))?
    .try_into()
}

/// Reads the most recent run for a job within the transaction.
async fn latest_run(
    tx: &mut Transaction<'_, Postgres>,
    job_id: Uuid,
) -> Result<Option<IngestionStepRun>> {
    sqlx::query_as::<_, IngestionStepRunRow>(include_str!(
        "../queries/ingestion_step_run/find_latest.sql"
    ))
    .bind(job_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(Error::Query)?
    .map(IngestionStepRun::try_from)
    .transpose()
}
