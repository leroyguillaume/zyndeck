SELECT
    id,
    job_id,
    step,
    attempt,
    status,
    started_at,
    completed_at,
    error
FROM ingestion_step_run
WHERE id = $1
