INSERT INTO "user" (username, password_hash, role)
VALUES ($1, $2, $3)
RETURNING id, username, role, created_at
