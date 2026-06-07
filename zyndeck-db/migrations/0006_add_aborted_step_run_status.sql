-- Allow the `aborted` status: a run stopped externally (via the `stop` command)
-- before it finished. Mirrors zyndeck-core's `StepRunStatus::Aborted`.
ALTER TABLE ingestion_step_run
DROP CONSTRAINT ingestion_step_run_status_check,
ADD CONSTRAINT ingestion_step_run_status_check
CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'aborted'));
