-- Claim a job's pending run for execution: flip it from `pending` to `running`
-- and stamp `started_at`. The `WHERE status = 'pending'` makes this an atomic
-- claim — only one caller wins, so duplicate notifications (or several service
-- instances) cannot execute the same run twice. Matches no row when there is
-- nothing pending.
UPDATE ingestion_step_run
SET status = 'running', started_at = now()
WHERE job_id = $1 AND status = 'pending'
RETURNING id, job_id, step, attempt, status, started_at, completed_at, error
