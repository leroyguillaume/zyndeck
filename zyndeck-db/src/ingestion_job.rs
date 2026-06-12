use std::future::Future;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use zyndeck_core::{IngestionJob, IngestionStep, LanguageCode};

use crate::{Error, Result};

/// Data needed to create an ingestion job. A new job starts on
/// [`IngestionStep::FIRST`] (the database default).
#[derive(Debug, Clone)]
pub struct NewIngestionJob {
    pub game_id: Uuid,
    pub source: PathBuf,
    pub language: LanguageCode,
    pub created_by: Option<Uuid>,
}

/// Persistence operations for [`IngestionJob`].
///
/// A trait so callers can depend on the abstraction and swap in a `mockall`
/// double in unit tests. Methods return `impl Future + Send` so they stay
/// awaitable inside an async handler; inject with generics, never `dyn`.
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait IngestionJobRepository {
    /// Inserts a job, returning it with its database-assigned id, starting step,
    /// and creation timestamp.
    fn create(&self, job: NewIngestionJob) -> impl Future<Output = Result<IngestionJob>> + Send;

    /// Fetches a job by id, or `None` if no such job exists.
    fn find_by_id(&self, id: Uuid) -> impl Future<Output = Result<Option<IngestionJob>>> + Send;

    /// Sets the job's step, returning the updated job, or `None` if no job has
    /// that id.
    fn update_step(
        &self,
        id: Uuid,
        step: IngestionStep,
    ) -> impl Future<Output = Result<Option<IngestionJob>>> + Send;
}

/// Maps the error from creating a job: a foreign-key violation on `game_id`
/// means the referenced game does not exist, reported as [`Error::GameNotFound`]
/// rather than a raw query error. Shared by the repository and [`crate::Db`]'s
/// `start_job`.
pub(crate) fn map_create_error(e: sqlx::Error, game_id: Uuid) -> Error {
    if let sqlx::Error::Database(db) = &e
        && db.is_foreign_key_violation()
        && db.constraint() == Some("ingestion_job_game_id_fkey")
    {
        return Error::GameNotFound(game_id);
    }
    Error::Query(e)
}

/// A single `ingestion_job` row, fallibly mapped to [`IngestionJob`] (the step
/// text may, in principle, fail to parse). Visible to the crate so the
/// transactional transitions in [`crate::Db`] can reuse the mapping.
#[derive(sqlx::FromRow)]
pub(crate) struct IngestionJobRow {
    id: Uuid,
    game_id: Uuid,
    source: String,
    language: String,
    step: String,
    created_at: DateTime<Utc>,
    created_by: Option<Uuid>,
}

impl TryFrom<IngestionJobRow> for IngestionJob {
    type Error = Error;

    fn try_from(row: IngestionJobRow) -> Result<Self> {
        let step: IngestionStep = row
            .step
            .parse()
            .map_err(|_| Error::InvalidIngestionStep(row.step.clone()))?;
        let language: LanguageCode = row
            .language
            .parse()
            .map_err(|_| Error::InvalidLanguage(row.language.clone()))?;
        Ok(IngestionJob {
            id: row.id,
            game_id: row.game_id,
            source: PathBuf::from(row.source),
            language,
            step,
            created_by: row.created_by,
            created_at: row.created_at,
        })
    }
}

/// Postgres-backed [`IngestionJobRepository`].
#[derive(Debug, Clone)]
pub struct PgIngestionJobRepository {
    pool: PgPool,
}

impl PgIngestionJobRepository {
    /// Builds a repository over the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl IngestionJobRepository for PgIngestionJobRepository {
    async fn create(&self, job: NewIngestionJob) -> Result<IngestionJob> {
        tracing::debug!(game_id = %job.game_id, "inserting ingestion job");
        let row = sqlx::query_as::<_, IngestionJobRow>(include_str!(
            "../queries/ingestion_job/create.sql"
        ))
        .bind(job.game_id)
        .bind(job.source.to_string_lossy().into_owned())
        .bind(job.language.as_str())
        .bind(job.created_by)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| map_create_error(e, job.game_id))?;
        tracing::debug!(job_id = %row.id, "ingestion job inserted");
        row.try_into()
    }

    async fn find_by_id(&self, id: Uuid) -> Result<Option<IngestionJob>> {
        tracing::debug!(job_id = %id, "fetching ingestion job by id");
        let row = sqlx::query_as::<_, IngestionJobRow>(include_str!(
            "../queries/ingestion_job/find_by_id.sql"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::Query)?;
        row.map(IngestionJob::try_from).transpose()
    }

    async fn update_step(&self, id: Uuid, step: IngestionStep) -> Result<Option<IngestionJob>> {
        tracing::debug!(job_id = %id, step = step.as_str(), "updating ingestion job step");
        let row = sqlx::query_as::<_, IngestionJobRow>(include_str!(
            "../queries/ingestion_job/update_step.sql"
        ))
        .bind(step.as_str())
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::Query)?;
        row.map(IngestionJob::try_from).transpose()
    }
}
