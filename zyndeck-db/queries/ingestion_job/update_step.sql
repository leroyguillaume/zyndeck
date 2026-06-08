UPDATE ingestion_job
SET step = $1
WHERE id = $2
RETURNING id, game_id, source, language, step, mode, created_at, created_by
