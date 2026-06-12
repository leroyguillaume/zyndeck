-- Ids of jobs that have a step run still pending: work the ingestion service has
-- not yet claimed. Used for the startup catch-up sweep, in case the job was
-- enqueued (created, validated, or restarted) while the service was down and its
-- NOTIFY was therefore lost.
SELECT DISTINCT job_id
FROM ingestion_step_run
WHERE status = 'pending'
