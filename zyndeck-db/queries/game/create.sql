INSERT INTO game (name, created_by)
VALUES ($1, $2)
RETURNING id, name, created_at, created_by
