-- Enqueue a step run for the ingestion service to pick up: insert a `pending`
-- row with the next attempt number for that (job, step). The partial unique
-- index `one_active_run_per_job` (which counts `pending` as active) guarantees a
-- job has at most one pending run, so this fails if one is already in flight.
INSERT INTO ingestion_step_run (job_id, step, attempt, status)
VALUES (
    $1,
    $2,
    (
        SELECT coalesce(max(attempt), 0) + 1
        FROM ingestion_step_run
        WHERE job_id = $1 AND step = $2
    ),
    'pending'
)
RETURNING id, job_id, step, attempt, status, started_at, completed_at, error
