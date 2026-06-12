-- Chunks of a job's validated transcript: the unit of retrieval the
-- question-answering will search over. Produced by the `chunk` step from the
-- (reviewed) transcript and embedded by the `embed` step.
--
-- `position` is the 0-based order of the chunk within the job, so chunks read
-- back in document order; `heading` is the section title the chunk falls under
-- ('' when it precedes any heading) and `page` the 1-based source page it starts
-- on, both kept so retrieval can cite where a rule came from. `content` is the
-- chunk text, embedded as-is. Re-running the step replaces a job's chunks
-- wholesale, so `(job_id, position)` is unique.
CREATE TABLE ingestion_chunk (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    job_id uuid NOT NULL REFERENCES ingestion_job (id) ON DELETE CASCADE,
    position integer NOT NULL,
    heading text NOT NULL,
    page integer NOT NULL,
    content text NOT NULL,
    UNIQUE (job_id, position)
);

-- The embedding vector for a chunk, produced by the `embed` step. A separate
-- table so a chunk row carries no "not embedded yet" nullable column, mirroring
-- how the transcript is split from the job. The dimension (1024) matches the
-- BGE-M3 model the ingester embeds with (see the root README's models table).
CREATE TABLE ingestion_chunk_embedding (
    chunk_id uuid PRIMARY KEY REFERENCES ingestion_chunk (id) ON DELETE CASCADE,
    embedding vector(1024) NOT NULL
);

-- HNSW index for cosine-distance nearest-neighbour search, the similarity the
-- normalised BGE-M3 embeddings are queried with. Built now so the retrieval the
-- API will run does not fall back to a sequential scan as the store grows.
--
-- Not CONCURRENTLY (which PG01 would have us use): migrations run inside a
-- transaction, where CONCURRENTLY is illegal, and the table is empty at creation
-- so the build locks nothing of consequence.
CREATE INDEX ingestion_chunk_embedding_hnsw -- noqa: PG01
ON ingestion_chunk_embedding
USING hnsw (embedding vector_cosine_ops);
