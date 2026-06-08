SELECT
    id,
    game_id,
    source,
    language,
    step,
    mode,
    created_at,
    created_by
FROM ingestion_job
WHERE id = $1
