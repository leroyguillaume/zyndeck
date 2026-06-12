-- Store (or replace) the embedding vector for a chunk, so re-running the `embed`
-- step overwrites a stale vector rather than failing.
INSERT INTO ingestion_chunk_embedding (chunk_id, embedding)
VALUES ($1, $2)
ON CONFLICT (chunk_id) DO UPDATE SET embedding = excluded.embedding
