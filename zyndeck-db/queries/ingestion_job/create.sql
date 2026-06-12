INSERT INTO ingestion_job (game_id, source, language, created_by)
VALUES ($1, $2, $3, $4)
RETURNING id, game_id, source, language, step, created_at, created_by
