-- Enable pgvector so rule embeddings can be stored and queried directly in
-- Postgres, alongside the relational data.
CREATE EXTENSION IF NOT EXISTS vector;

-- User accounts.
--
-- `role` is stored as text constrained to the known Role values (mirrors
-- zyndeck-core's `Role`); `username` is unique. `user` is a reserved word in
-- Postgres, so the table name must be quoted everywhere it appears.
CREATE TABLE "user" (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    username text NOT NULL UNIQUE,
    password_hash text NOT NULL,
    role text NOT NULL CHECK (role IN ('super_admin', 'admin', 'user')),
    created_at timestamptz NOT NULL DEFAULT now()
);

-- Games catalogued by Zyndeck.
--
-- `name` is localised: a JSON object mapping a language code to the game's name
-- in that language, e.g. {"fr": "Marvel Champions", "en": "Marvel Champions"}.
-- The shape (ISO 639-1 keys, string values) is validated in the application
-- layer, not by a database CHECK. `created_by` references the user who added it.
CREATE TABLE game (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid NOT NULL REFERENCES "user" (id)
);

-- A run of the rule-ingestion pipeline for one document.
--
-- `step` records the next step to run, stored as text constrained to the known
-- IngestionStep values (mirrors zyndeck-core's `IngestionStep`). `created_by` is
-- the user who started the job; it is nullable because CLI runs may be anonymous,
-- and the reference is cleared rather than cascaded so a job outlives its creator.
--
-- `game_id`, `source` and `language` are the inputs the job needs across its
-- steps and retries (which game the rules belong to, the source document path,
-- and its language), so the extract step and its re-runs never need them
-- re-supplied. The pipeline always pauses after `extract` for human validation
-- of the transcript; once validated it runs `chunk` then `embed` straight
-- through, so no per-job advancement mode is stored.
CREATE TABLE ingestion_job (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    step text NOT NULL DEFAULT 'extract'
    CHECK (step IN ('extract', 'chunk', 'embed', 'completed')),
    game_id uuid NOT NULL REFERENCES game (id) ON DELETE CASCADE,
    source text NOT NULL,
    language text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid REFERENCES "user" (id) ON DELETE SET NULL
);

-- History of step executions for an ingestion job: one row per attempt of a
-- step, so retries and failures are auditable. `attempt` is 1-based per
-- (job, step) and increments on each retry (mirrors zyndeck-core's
-- `IngestionStepRun`); `status` mirrors `StepRunStatus` (`aborted` is a run
-- stopped externally, via the `stop` command, before it finished).
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
    CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'aborted')),
    started_at timestamptz,
    completed_at timestamptz,
    error text,
    UNIQUE (job_id, step, attempt)
);

CREATE UNIQUE INDEX one_active_run_per_job
ON ingestion_step_run (job_id)
WHERE status IN ('pending', 'running');

-- The reviewable transcript produced by the extract step: one per job, replaced
-- when the step is re-run. A separate table so the job entity stays free of a
-- nullable "not extracted yet" column.
CREATE TABLE ingestion_transcript (
    job_id uuid PRIMARY KEY REFERENCES ingestion_job (id) ON DELETE CASCADE,
    content text NOT NULL
);

-- Notify the ingestion service that a job has work waiting, so it can pick it up
-- without polling. The single `ingestion_job_ready` channel carries the job's id
-- as the payload and is fired both by this trigger (on job creation) and by the
-- application's validate/restart transitions (which enqueue a pending step run
-- then `pg_notify` the same channel). The service reacts identically: claim the
-- job's pending run and execute it.
--
-- NOTIFY is transactional: the notification is delivered only when the enqueuing
-- transaction commits, and never if it rolls back — so the service is told about
-- the pending run exactly when it becomes visible.
CREATE FUNCTION notify_ingestion_job_ready()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM pg_notify('ingestion_job_ready', new.id::text);
    RETURN new;
END;
$$;

CREATE TRIGGER ingestion_job_ready_notify
AFTER INSERT ON ingestion_job
FOR EACH ROW
EXECUTE FUNCTION notify_ingestion_job_ready();
