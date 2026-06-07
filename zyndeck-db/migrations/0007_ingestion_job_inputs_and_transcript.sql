-- The inputs an ingestion job needs across its steps and retries: which game
-- the rules belong to, the source document path, and its language. Required, so
-- the extract step (and re-runs of it) never need them re-supplied.
ALTER TABLE ingestion_job
ADD COLUMN game_id uuid NOT NULL REFERENCES game (id) ON DELETE CASCADE,
ADD COLUMN source text NOT NULL,
ADD COLUMN language text NOT NULL;

-- The reviewable transcript produced by the extract step: one per job, replaced
-- when the step is re-run. A separate table so the job entity stays free of a
-- nullable "not extracted yet" column.
CREATE TABLE ingestion_transcript (
    job_id uuid PRIMARY KEY REFERENCES ingestion_job (id) ON DELETE CASCADE,
    content text NOT NULL
);
