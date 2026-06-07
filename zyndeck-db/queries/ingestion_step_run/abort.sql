UPDATE ingestion_step_run
SET status = 'aborted', completed_at = now()
WHERE job_id = $1 AND status IN ('pending', 'running')
RETURNING id, job_id, step, attempt, status, started_at, completed_at, error
