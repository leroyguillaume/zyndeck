-- Read a job's chunks back in document order, for the `embed` step and (later)
-- retrieval inspection.
SELECT
    id,
    position,
    heading,
    page,
    content
FROM ingestion_chunk
WHERE job_id = $1
ORDER BY position
