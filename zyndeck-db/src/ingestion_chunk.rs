use std::future::Future;

use pgvector::Vector;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{Error, Result};

/// A chunk to insert for a job: the chunk text plus the provenance the chunking
/// step derived. `position` is the 0-based order within the job, `page` the
/// 1-based source page the chunk starts on, and `heading` the section title it
/// falls under (empty when it precedes any heading).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChunk {
    pub position: i32,
    pub heading: String,
    pub page: i32,
    pub content: String,
}

/// A stored chunk, read back by the embed step (and, later, retrieval) with its
/// database-assigned id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub id: Uuid,
    pub position: i32,
    pub heading: String,
    pub page: i32,
    pub content: String,
}

/// Persistence for a job's transcript chunks and their embeddings — the
/// retrieval units the `chunk` step produces and the `embed` step vectorises.
///
/// A trait so callers can depend on the abstraction and swap in a `mockall`
/// double in unit tests. Methods return `impl Future + Send` so they stay
/// awaitable inside an async handler; inject with generics, never `dyn`.
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait IngestionChunkRepository {
    /// Replaces a job's chunks with `chunks` in one transaction (delete then
    /// insert), so re-running the `chunk` step never leaves stale chunks behind.
    /// Returns the inserted chunks with their assigned ids, in document order.
    /// Dropping a chunk cascades its embedding away.
    fn replace(
        &self,
        job_id: Uuid,
        chunks: Vec<NewChunk>,
    ) -> impl Future<Output = Result<Vec<Chunk>>> + Send;

    /// Reads a job's chunks back in document order.
    fn find_by_job(&self, job_id: Uuid) -> impl Future<Output = Result<Vec<Chunk>>> + Send;

    /// Stores (or replaces) the embedding vector for a chunk. The vector length
    /// must match the `vector(N)` column or Postgres rejects it.
    fn store_embedding(
        &self,
        chunk_id: Uuid,
        embedding: Vec<f32>,
    ) -> impl Future<Output = Result<()>> + Send;
}

/// A single `ingestion_chunk` row, mapped to [`Chunk`] via [`From`].
#[derive(sqlx::FromRow)]
struct ChunkRow {
    id: Uuid,
    position: i32,
    heading: String,
    page: i32,
    content: String,
}

impl From<ChunkRow> for Chunk {
    fn from(row: ChunkRow) -> Self {
        Chunk {
            id: row.id,
            position: row.position,
            heading: row.heading,
            page: row.page,
            content: row.content,
        }
    }
}

/// Postgres-backed [`IngestionChunkRepository`].
#[derive(Debug, Clone)]
pub struct PgIngestionChunkRepository {
    pool: PgPool,
}

impl PgIngestionChunkRepository {
    /// Builds a repository over the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl IngestionChunkRepository for PgIngestionChunkRepository {
    async fn replace(&self, job_id: Uuid, chunks: Vec<NewChunk>) -> Result<Vec<Chunk>> {
        tracing::debug!(%job_id, count = chunks.len(), "replacing job chunks");
        let mut tx = self.pool.begin().await.map_err(Error::Query)?;

        sqlx::query(include_str!("../queries/ingestion_chunk/delete_by_job.sql"))
            .bind(job_id)
            .execute(&mut *tx)
            .await
            .map_err(Error::Query)?;

        let mut inserted = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let row = sqlx::query_as::<_, ChunkRow>(include_str!(
                "../queries/ingestion_chunk/insert.sql"
            ))
            .bind(job_id)
            .bind(chunk.position)
            .bind(chunk.heading)
            .bind(chunk.page)
            .bind(chunk.content)
            .fetch_one(&mut *tx)
            .await
            .map_err(Error::Query)?;
            inserted.push(row.into());
        }

        tx.commit().await.map_err(Error::Query)?;
        Ok(inserted)
    }

    async fn find_by_job(&self, job_id: Uuid) -> Result<Vec<Chunk>> {
        tracing::debug!(%job_id, "fetching job chunks");
        let rows = sqlx::query_as::<_, ChunkRow>(include_str!(
            "../queries/ingestion_chunk/find_by_job.sql"
        ))
        .bind(job_id)
        .fetch_all(&self.pool)
        .await
        .map_err(Error::Query)?;
        Ok(rows.into_iter().map(Chunk::from).collect())
    }

    async fn store_embedding(&self, chunk_id: Uuid, embedding: Vec<f32>) -> Result<()> {
        tracing::debug!(%chunk_id, dim = embedding.len(), "storing chunk embedding");
        sqlx::query(include_str!(
            "../queries/ingestion_chunk_embedding/upsert.sql"
        ))
        .bind(chunk_id)
        .bind(Vector::from(embedding))
        .execute(&self.pool)
        .await
        .map_err(Error::Query)?;
        Ok(())
    }
}
