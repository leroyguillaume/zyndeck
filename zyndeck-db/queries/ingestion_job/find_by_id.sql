SELECT
    id,
    game_id,
    source,
    language,
    step,
    created_at,
    created_by
FROM ingestion_job
WHERE id = $1
