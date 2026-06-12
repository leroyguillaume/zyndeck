-- Insert one chunk for a job, returning its assigned id.
INSERT INTO ingestion_chunk (job_id, position, heading, page, content)
VALUES ($1, $2, $3, $4, $5)
RETURNING id, position, heading, page, content
