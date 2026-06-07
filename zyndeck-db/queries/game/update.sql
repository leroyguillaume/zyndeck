UPDATE game
SET name = $1
WHERE id = $2
RETURNING id, name, created_at, created_by
