-- Ids of jobs that have never been run and are not yet completed: a fresh job
-- the ingestion service still has to pick up. Used for the startup catch-up
-- sweep, in case a job was created while the service was down (and its NOTIFY
-- was therefore lost).
SELECT j.id
FROM ingestion_job AS j
LEFT JOIN ingestion_step_run AS r ON j.id = r.job_id
WHERE
    j.step <> 'completed'
    AND r.id IS NULL
