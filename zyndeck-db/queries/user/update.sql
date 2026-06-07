UPDATE "user"
SET
    username = $1,
    role = $2
WHERE id = $3
RETURNING id, username, role, created_at
