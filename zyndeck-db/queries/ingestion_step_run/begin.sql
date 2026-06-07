INSERT INTO ingestion_step_run (job_id, step, attempt, status, started_at)
VALUES (
    $1,
    $2,
    (
        SELECT coalesce(max(attempt), 0) + 1
        FROM ingestion_step_run
        WHERE job_id = $1 AND step = $2
    ),
    'running',
    now()
)
RETURNING id, job_id, step, attempt, status, started_at, completed_at, error
