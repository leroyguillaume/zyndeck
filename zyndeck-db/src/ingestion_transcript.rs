use std::future::Future;

use sqlx::PgPool;
use uuid::Uuid;

use crate::{Error, Result};

/// Persistence for a job's reviewable transcript — the Markdown produced by the
/// extract step, which the later steps consume and a human may edit in between.
/// One transcript per job, replaced when the extract step is re-run.
///
/// A trait so callers can depend on the abstraction and swap in a `mockall`
/// double in unit tests. Methods return `impl Future + Send` so they stay
/// awaitable inside an async handler; inject with generics, never `dyn`.
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait IngestionTranscriptRepository {
    /// Stores (or replaces) the transcript for a job.
    fn upsert(&self, job_id: Uuid, content: String) -> impl Future<Output = Result<()>> + Send;

    /// Fetches a job's transcript, or `None` if it has none yet.
    fn find(&self, job_id: Uuid) -> impl Future<Output = Result<Option<String>>> + Send;
}

/// Postgres-backed [`IngestionTranscriptRepository`].
#[derive(Debug, Clone)]
pub struct PgIngestionTranscriptRepository {
    pool: PgPool,
}

impl PgIngestionTranscriptRepository {
    /// Builds a repository over the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl IngestionTranscriptRepository for PgIngestionTranscriptRepository {
    async fn upsert(&self, job_id: Uuid, content: String) -> Result<()> {
        tracing::debug!(%job_id, bytes = content.len(), "storing transcript");
        sqlx::query(include_str!("../queries/ingestion_transcript/upsert.sql"))
            .bind(job_id)
            .bind(content)
            .execute(&self.pool)
            .await
            .map_err(Error::Query)?;
        Ok(())
    }

    async fn find(&self, job_id: Uuid) -> Result<Option<String>> {
        tracing::debug!(%job_id, "fetching transcript");
        sqlx::query_scalar(include_str!("../queries/ingestion_transcript/find.sql"))
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::Query)
    }
}
