-- Notify listeners when an ingestion job is created, so the ingestion service
-- can pick it up without polling. The CLI (and any other writer) just inserts a
-- row; this trigger turns that insert into a `NOTIFY` on the
-- `ingestion_job_created` channel, carrying the new job's id as the payload.
--
-- NOTIFY is transactional: the notification is delivered only when the inserting
-- transaction commits, and never if it rolls back — so a listener is told about
-- a job exactly when the job becomes visible.
CREATE FUNCTION notify_ingestion_job_created()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM pg_notify('ingestion_job_created', new.id::text);
    RETURN new;
END;
$$;

CREATE TRIGGER ingestion_job_created_notify
AFTER INSERT ON ingestion_job
FOR EACH ROW
EXECUTE FUNCTION notify_ingestion_job_created();
