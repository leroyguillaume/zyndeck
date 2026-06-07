SELECT
    id,
    username,
    role,
    created_at
FROM "user"
WHERE id = $1
