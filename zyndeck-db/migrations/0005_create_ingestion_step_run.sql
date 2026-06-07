-- History of step executions for an ingestion job: one row per attempt of a
-- step, so retries and failures are auditable. `attempt` is 1-based per
-- (job, step) and increments on each retry (mirrors zyndeck-core's
-- `IngestionStepRun`); `status` mirrors `StepRunStatus`.
--
-- A job runs at most one step at a time: the partial unique index forbids a
-- second active (pending/running) run for the same job, which is how concurrent
-- runs of one job are prevented.
CREATE TABLE ingestion_step_run (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    job_id uuid NOT NULL REFERENCES ingestion_job (id) ON DELETE CASCADE,
    step text NOT NULL CHECK (step IN ('extract', 'chunk', 'embed')),
    attempt integer NOT NULL,
    status text NOT NULL
    CHECK (status IN ('pending', 'running', 'succeeded', 'failed')),
    started_at timestamptz,
    completed_at timestamptz,
    error text,
    UNIQUE (job_id, step, attempt)
);

CREATE UNIQUE INDEX one_active_run_per_job
ON ingestion_step_run (job_id)
WHERE status IN ('pending', 'running');
