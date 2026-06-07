-- A run of the rule-ingestion pipeline for one document.
--
-- `step` records the next step to run, stored as text constrained to the known
-- IngestionStep values (mirrors zyndeck-core's `IngestionStep`). `created_by` is
-- the user who started the job; it is nullable because CLI runs may be anonymous,
-- and the reference is cleared rather than cascaded so a job outlives its creator.
CREATE TABLE ingestion_job (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    step text NOT NULL DEFAULT 'extract'
    CHECK (step IN ('extract', 'chunk', 'embed', 'completed')),
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid REFERENCES "user" (id) ON DELETE SET NULL
);
