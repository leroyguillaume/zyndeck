# Zyndeck

Zyndeck is an application for managing deckbuilding games. It lets you build
decks, validate them against each game's rules, and ask a built-in LLM
questions about how a game's rules work.

It is implemented as a Rust
[Cargo workspace](https://doc.rust-lang.org/cargo/reference/workspaces.html);
each component lives in its own crate directory at the repository root.

## Crates

| Crate | Description |
| --- | --- |
| [`zyndeck-core`](zyndeck-core) | Domain model: entities and value types (e.g. `Game`, `User`, `LocalizedString`), no I/O. |
| [`zyndeck-db`](zyndeck-db) | Database access layer: PostgreSQL connection pool, migrations (pgvector for rule embeddings), and repositories. |
| [`zyndeck-api`](zyndeck-api) | HTTP API: CRUD for games and users, with JWT auth and role-based access control. |
| [`zyndeck-ingester`](zyndeck-ingester) | Service that ingests game rules so they can be validated against and queried by the LLM. |

## Requirements

- A Rust toolchain (edition 2024 — Rust 1.85 or newer; tested with 1.94).
  Install via [rustup](https://rustup.rs/).
- [`pre-commit`](https://pre-commit.com/) for the git hooks.
- [Docker](https://docs.docker.com/) with Compose v2.30 or newer (for the
  `post_start` hook) to run the local Ollama backend.

## Models

The ingester targets CPU-only, low-RAM hosts (no GPU). It relies on two
locally-served [Ollama](https://ollama.com/) models, both small enough to run
comfortably without a GPU:

| Model | Tag | Role | Size | Why |
| --- | --- | --- | --- | --- |
| [BGE-M3](https://ollama.com/library/bge-m3) | `bge-m3:567m` | Embeddings | ~1.2 GB | Strong cross-lingual retrieval (English rules, French questions), 8K context. |
| [Gemma 3 4B](https://ollama.com/library/gemma3) | `gemma3:4b-it-qat` | Generation | ~3 GB | QAT build for a low memory footprint, solid French, simple non-reasoning Q&A. |

Game rules are written in English; users can ask questions in French. The
multilingual embedding model maps both languages into a shared vector space, so
a French query retrieves the relevant English rule chunks, which the generation
model then answers in French.

These tags reflect a benchmark snapshot from **June 2026** — local-model
leaderboards move fast, so revisit the choice periodically.

`docker-compose.yaml` runs the Ollama server and pulls both models on startup
via a `post_start` hook:

```bash
docker compose up -d
```

The models are cached in the `ollama-models` volume, so subsequent starts are
fast and the `post_start` pulls become no-ops. The server is reachable at
`http://localhost:11434`.

## Install

```bash
git clone https://github.com/leroyguillaume/zyndeck.git
cd zyndeck
cargo build
pre-commit install
```

## Configure

Configuration resolves in the order **CLI flags → environment variables →
defaults**. Every option is settable both ways.

Common options:

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--log-filter` | `RUST_LOG` | `info` | `tracing` filter directive (e.g. `info`, `zyndeck_api=debug`). |

`zyndeck-api` and `zyndeck-db`-backed binaries additionally take:

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--database-url` | `DATABASE_URL` | _(required)_ | PostgreSQL connection URL. |
| `--db-max-connections` | `DB_MAX_CONNECTIONS` | `10` | Connection pool size. |
| `--bind-addr` | `BIND_ADDR` | `0.0.0.0:8080` | Address the API server binds to. |
| `--jwt-secret` | `JWT_SECRET` | _(required for the API)_ | Secret used to sign and verify HS256 JWTs. |
| `--jwt-ttl` | `JWT_TTL_SECONDS` | `86400` | Lifetime (seconds) of tokens issued by `/auth/login`. |
| `--admin-username` | `ADMIN_USERNAME` | `admin` | Username of the bootstrap super-admin. |
| `--admin-password` | `ADMIN_PASSWORD` | _(required for the API)_ | Password of the bootstrap super-admin. |

## Run

Run a specific crate from the workspace root with `-p`. The ingester is
database-backed and applies migrations at startup, so it needs `DATABASE_URL`:

```bash
docker compose up -d postgres
DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
  cargo run -p zyndeck-ingester -- run
```

See the [`zyndeck-ingester` README](zyndeck-ingester) for its subcommands.

The HTTP API needs a database and a JWT secret:

```bash
docker compose up -d postgres
DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
  JWT_SECRET=change-me-at-least-32-bytes-long-secret \
  ADMIN_PASSWORD=change-me \
  cargo run -p zyndeck-api
```

It applies migrations on startup, then bootstraps (or resets) a super-admin from
`ADMIN_USERNAME`/`ADMIN_PASSWORD` — both are required (passwords are Argon2-hashed;
the plaintext is never stored) — before serving the API. Interactive docs
(Scalar) are at `/docs` and the OpenAPI document at `/openapi.json`.

Callers authenticate with an HS256-signed bearer token whose `sub` claim is the
user's id. Obtain one from `POST /auth/login` (username + password →
`{ accessToken, tokenType, expiresIn }`); the token is then sent as
`Authorization: Bearer <token>`. Authorization:

- **Games** — reads are public; create/update/delete require an admin.
- **Users** — all reads require authentication; a plain user may only read
  themselves, admins may read anyone. Create/update require an admin. An admin
  may delete plain users but not other admins; a super admin may delete anyone.

Every service runs until it receives `SIGINT` (Ctrl-C) or `SIGTERM`, then shuts
down gracefully.

## Test

`zyndeck-api`'s tests mock the database (via `zyndeck-db`'s `mock` feature), so
they need no Postgres. `zyndeck-db`'s own integration tests, however, run against
a real Postgres (the `postgres` service in `docker-compose.yaml`);
`#[sqlx::test]` provisions an isolated database per test. Start the service and
point `DATABASE_URL` at it:

```bash
docker compose up -d postgres
export DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck
```

Run the whole workspace test suite:

```bash
cargo test --workspace
```

Lint and format checks (also run as pre-commit hooks):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```
