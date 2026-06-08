-- How a job advances between steps, stored as text constrained to the known
-- IngestionMode values (mirrors zyndeck-core's `IngestionMode`). `auto` (the
-- default) runs straight through until the job completes or a step fails;
-- `manual` pauses after each step so its output can be reviewed and corrected.
ALTER TABLE ingestion_job
ADD COLUMN mode text NOT NULL DEFAULT 'auto'
CHECK (mode IN ('manual', 'auto'));
