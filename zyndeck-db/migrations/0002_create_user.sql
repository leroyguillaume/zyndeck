-- User accounts.
--
-- `role` is stored as text constrained to the known Role values (mirrors
-- zyndeck-core's `Role`); `username` is unique. `user` is a reserved word in
-- Postgres, so the table name must be quoted everywhere it appears.
CREATE TABLE "user" (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    username text NOT NULL UNIQUE,
    password_hash text NOT NULL,
    role text NOT NULL CHECK (role IN ('super_admin', 'admin', 'user')),
    created_at timestamptz NOT NULL DEFAULT now()
);
