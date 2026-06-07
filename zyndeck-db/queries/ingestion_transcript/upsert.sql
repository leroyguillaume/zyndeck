INSERT INTO ingestion_transcript (job_id, content)
VALUES ($1, $2)
ON CONFLICT (job_id) DO UPDATE SET content = excluded.content
