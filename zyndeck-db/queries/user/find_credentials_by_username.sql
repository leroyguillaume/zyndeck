SELECT
    id,
    password_hash
FROM "user"
WHERE username = $1
