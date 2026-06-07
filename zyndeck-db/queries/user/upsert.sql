INSERT INTO "user" (username, password_hash, role)
VALUES ($1, $2, $3)
ON CONFLICT (username) DO UPDATE
    SET
        password_hash = excluded.password_hash,
        role = excluded.role
RETURNING id, username, role, created_at
