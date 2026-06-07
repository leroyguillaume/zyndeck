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
WHERE job_id = $1
ORDER BY started_at DESC NULLS LAST, attempt DESC
LIMIT 1
