SELECT
    id,
    username,
    role,
    created_at
FROM "user"
WHERE username = $1
