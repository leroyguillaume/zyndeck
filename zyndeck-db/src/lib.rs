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
use sqlx::postgres::{PgListener, PgPoolOptions};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;
use zyndeck_core::{IngestionJob, IngestionStep, IngestionStepRun};

/// Embedded SQL migrations, applied in order by [`Db::migrate`].
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Postgres `NOTIFY` channel on which a row is announced as soon as an ingestion
/// job is created (see migration `0009`). The payload is the new job's id.
const JOB_CREATED_CHANNEL: &str = "ingestion_job_created";

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

    /// Claims a freshly-created job for processing: if it has never been run and
    /// is not completed, begins a run for its (first) step and returns it.
    /// Returns `None` when there is nothing to claim — the job already has a run
    /// (in progress, paused for review, failed, or done) or is completed.
    ///
    /// Like the other transitions it holds a `FOR UPDATE` lock on the job row,
    /// so two services reacting to the same notification cannot both claim it:
    /// the second waits for the first to commit, then sees the run and returns
    /// `None`.
    pub async fn begin_initial_run(&self, job_id: Uuid) -> Result<Option<IngestionStepRun>> {
        let mut tx = self.pool.begin().await.map_err(Error::Query)?;

        let job = lock_job(&mut tx, job_id).await?;
        if job.step.is_completed() || latest_run(&mut tx, job_id).await?.is_some() {
            return Ok(None);
        }
        let run = begin_run(&mut tx, job_id, job.step).await?;

        tx.commit().await.map_err(Error::Query)?;
        Ok(Some(run))
    }

    /// Ids of jobs that have never been run and are not yet completed — the
    /// fresh jobs the service still has to pick up. Used for a startup
    /// catch-up sweep, since a job created while no service was listening had
    /// its `NOTIFY` dropped.
    pub async fn unprocessed_job_ids(&self) -> Result<Vec<Uuid>> {
        sqlx::query_scalar::<_, Uuid>(include_str!(
            "../queries/ingestion_job/find_unprocessed_ids.sql"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(Error::Query)
    }

    /// Subscribes to ingestion-job-created notifications, yielding each new
    /// job's id via [`IngestionJobListener::recv`]. The subscription owns its
    /// own connection (separate from the pool's), released when the listener is
    /// dropped — e.g. on shutdown.
    pub async fn listen_ingestion_jobs(&self) -> Result<IngestionJobListener> {
        let mut listener = PgListener::connect_with(&self.pool)
            .await
            .map_err(Error::Connect)?;
        listener
            .listen(JOB_CREATED_CHANNEL)
            .await
            .map_err(Error::Query)?;
        tracing::debug!(
            channel = JOB_CREATED_CHANNEL,
            "listening for ingestion jobs"
        );
        Ok(IngestionJobListener { inner: listener })
    }
}

/// A subscription to ingestion-job-created notifications.
///
/// Wraps a Postgres `LISTEN` connection so callers depend on a job-id stream
/// rather than on `sqlx` directly. Reconnects transparently if the connection
/// drops; notifications missed during a reconnect are recovered by
/// [`Db::unprocessed_job_ids`] on the next startup sweep.
pub struct IngestionJobListener {
    inner: PgListener,
}

impl IngestionJobListener {
    /// Waits for the next created job and returns its id. Payloads that are not
    /// a valid job id are skipped (logged), so this only ever yields ids.
    pub async fn recv(&mut self) -> Result<Uuid> {
        loop {
            let notification = self.inner.recv().await.map_err(Error::Query)?;
            match notification.payload().parse::<Uuid>() {
                Ok(job_id) => return Ok(job_id),
                Err(_) => tracing::warn!(
                    payload = notification.payload(),
                    "ignoring job notification with a non-uuid payload",
                ),
            }
        }
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
