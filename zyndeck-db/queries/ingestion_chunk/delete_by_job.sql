-- Remove all of a job's chunks, so the `chunk` step can replace them wholesale
-- on a re-run. The `ingestion_chunk_embedding` rows cascade away with them.
DELETE FROM ingestion_chunk
WHERE job_id = $1
