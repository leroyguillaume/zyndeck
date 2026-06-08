INSERT INTO ingestion_job (game_id, source, language, mode, created_by)
VALUES ($1, $2, $3, $4, $5)
RETURNING id, game_id, source, language, step, mode, created_at, created_by
