use std::future::Future;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use zyndeck_core::{IngestionStep, IngestionStepRun, StepRunStatus};

use crate::{Error, Result};

/// How a step run ended — the input to [`IngestionStepRunRepository::finish`].
/// An enum rather than a `(status, Option<error>)` pair so a success can never
/// carry an error and a failure always does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepOutcome {
    Succeeded,
    Failed { error: String },
}

/// Persistence operations for [`IngestionStepRun`] — a job's step-execution
/// history.
///
/// A trait so callers can depend on the abstraction and swap in a `mockall`
/// double in unit tests. Methods return `impl Future + Send` so they stay
/// awaitable inside an async handler; inject with generics, never `dyn`.
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait IngestionStepRunRepository {
    /// Starts a run of `step` for the job: inserts a `running` row with the next
    /// attempt number for that `(job, step)`. Fails with
    /// [`Error::JobAlreadyRunning`] if the job already has an active run, which
    /// is how concurrent runs of one job are prevented.
    fn begin(
        &self,
        job_id: Uuid,
        step: IngestionStep,
    ) -> impl Future<Output = Result<IngestionStepRun>> + Send;

    /// Marks a run finished with the given `outcome`, stamping `completed_at`.
    /// Returns the updated run, or `None` if no run has that id.
    fn finish(
        &self,
        run_id: Uuid,
        outcome: StepOutcome,
    ) -> impl Future<Output = Result<Option<IngestionStepRun>>> + Send;

    /// Fetches the most recent run for a job, or `None` if it has none yet.
    fn find_latest(
        &self,
        job_id: Uuid,
    ) -> impl Future<Output = Result<Option<IngestionStepRun>>> + Send;

    /// Fetches a run by id, or `None` if it does not exist. Used to poll whether
    /// a running step has been stopped.
    fn find(&self, run_id: Uuid) -> impl Future<Output = Result<Option<IngestionStepRun>>> + Send;

    /// Aborts the job's active (pending/running) run, stamping `completed_at`.
    /// A write-only signal: the process executing the step observes the status
    /// change and stops. Returns the aborted run, or `None` if nothing was
    /// running.
    fn abort(&self, job_id: Uuid) -> impl Future<Output = Result<Option<IngestionStepRun>>> + Send;
}

/// Maps the error from a `begin` insert: a unique-constraint violation means the
/// job already has an active run (the partial unique index), which is how
/// concurrent runs are prevented. Shared by the repository and [`crate::Db`]'s
/// transactional transitions.
pub(crate) fn map_begin_error(e: sqlx::Error, job_id: Uuid) -> Error {
    match e {
        sqlx::Error::Database(db) if db.is_unique_violation() => Error::JobAlreadyRunning(job_id),
        other => Error::Query(other),
    }
}

/// A single `ingestion_step_run` row, fallibly mapped to [`IngestionStepRun`].
/// The status text plus the nullable timestamp/error columns are reassembled
/// into the [`StepRunStatus`] enum (the storage shape is nullable; the domain
/// shape is not). Visible to the crate so [`crate::Db`]'s transitions can reuse
/// the mapping.
#[derive(sqlx::FromRow)]
pub(crate) struct IngestionStepRunRow {
    id: Uuid,
    job_id: Uuid,
    step: String,
    attempt: i32,
    status: String,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    error: Option<String>,
}

impl TryFrom<IngestionStepRunRow> for IngestionStepRun {
    type Error = Error;

    fn try_from(row: IngestionStepRunRow) -> Result<Self> {
        let step: IngestionStep = row
            .step
            .parse()
            .map_err(|_| Error::InvalidIngestionStep(row.step.clone()))?;
        let status = status_from_columns(&row.status, row.started_at, row.completed_at, row.error)?;
        Ok(IngestionStepRun {
            id: row.id,
            job_id: row.job_id,
            step,
            attempt: row.attempt,
            status,
        })
    }
}

/// Reassembles a [`StepRunStatus`] from the status text and the nullable
/// timestamp/error columns. A column required by the status but missing means a
/// corrupt row, reported as [`Error::InvalidStepRunStatus`].
fn status_from_columns(
    status: &str,
    started_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    error: Option<String>,
) -> Result<StepRunStatus> {
    let corrupt = || Error::InvalidStepRunStatus(status.to_owned());
    Ok(match status {
        "pending" => StepRunStatus::Pending,
        "running" => StepRunStatus::Running {
            started_at: started_at.ok_or_else(corrupt)?,
        },
        "succeeded" => StepRunStatus::Succeeded {
            started_at: started_at.ok_or_else(corrupt)?,
            completed_at: completed_at.ok_or_else(corrupt)?,
        },
        "failed" => StepRunStatus::Failed {
            started_at: started_at.ok_or_else(corrupt)?,
            completed_at: completed_at.ok_or_else(corrupt)?,
            error: error.ok_or_else(corrupt)?,
        },
        "aborted" => StepRunStatus::Aborted {
            started_at: started_at.ok_or_else(corrupt)?,
            completed_at: completed_at.ok_or_else(corrupt)?,
        },
        _ => return Err(corrupt()),
    })
}

/// Postgres-backed [`IngestionStepRunRepository`].
#[derive(Debug, Clone)]
pub struct PgIngestionStepRunRepository {
    pool: PgPool,
}

impl PgIngestionStepRunRepository {
    /// Builds a repository over the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl IngestionStepRunRepository for PgIngestionStepRunRepository {
    async fn begin(&self, job_id: Uuid, step: IngestionStep) -> Result<IngestionStepRun> {
        tracing::debug!(%job_id, step = step.as_str(), "beginning step run");
        let row = sqlx::query_as::<_, IngestionStepRunRow>(include_str!(
            "../queries/ingestion_step_run/begin.sql"
        ))
        .bind(job_id)
        .bind(step.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| map_begin_error(e, job_id))?;
        tracing::debug!(run_id = %row.id, attempt = row.attempt, "step run started");
        row.try_into()
    }

    async fn finish(&self, run_id: Uuid, outcome: StepOutcome) -> Result<Option<IngestionStepRun>> {
        let (status, error) = match outcome {
            StepOutcome::Succeeded => ("succeeded", None),
            StepOutcome::Failed { error } => ("failed", Some(error)),
        };
        tracing::debug!(%run_id, status, "finishing step run");
        let row = sqlx::query_as::<_, IngestionStepRunRow>(include_str!(
            "../queries/ingestion_step_run/finish.sql"
        ))
        .bind(run_id)
        .bind(status)
        .bind(error)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::Query)?;
        row.map(IngestionStepRun::try_from).transpose()
    }

    async fn find_latest(&self, job_id: Uuid) -> Result<Option<IngestionStepRun>> {
        tracing::debug!(%job_id, "fetching latest step run");
        let row = sqlx::query_as::<_, IngestionStepRunRow>(include_str!(
            "../queries/ingestion_step_run/find_latest.sql"
        ))
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::Query)?;
        row.map(IngestionStepRun::try_from).transpose()
    }

    async fn find(&self, run_id: Uuid) -> Result<Option<IngestionStepRun>> {
        let row = sqlx::query_as::<_, IngestionStepRunRow>(include_str!(
            "../queries/ingestion_step_run/find_by_id.sql"
        ))
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::Query)?;
        row.map(IngestionStepRun::try_from).transpose()
    }

    async fn abort(&self, job_id: Uuid) -> Result<Option<IngestionStepRun>> {
        tracing::debug!(%job_id, "aborting active step run");
        let row = sqlx::query_as::<_, IngestionStepRunRow>(include_str!(
            "../queries/ingestion_step_run/abort.sql"
        ))
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::Query)?;
        row.map(IngestionStepRun::try_from).transpose()
    }
}
