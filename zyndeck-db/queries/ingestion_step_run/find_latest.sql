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
-- A pending run (not yet started, NULL `started_at`) is the newest, so it sorts
-- first: `NULLS FIRST` keeps it ahead of any earlier, already-started attempt.
ORDER BY started_at DESC NULLS FIRST, attempt DESC
LIMIT 1
