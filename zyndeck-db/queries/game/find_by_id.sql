SELECT
    id,
    name,
    created_at,
    created_by
FROM game
WHERE id = $1
