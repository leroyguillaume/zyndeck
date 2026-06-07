-- Only a still-running run can be finished: if it was aborted meanwhile, this
-- matches no row (and the caller learns the run was stopped) rather than
-- overwriting the `aborted` status.
UPDATE ingestion_step_run
SET status = $2, completed_at = now(), error = $3
WHERE id = $1 AND status = 'running'
RETURNING id, job_id, step, attempt, status, started_at, completed_at, error
