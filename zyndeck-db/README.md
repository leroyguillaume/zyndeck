# zyndeck-db

The database access layer of [Zyndeck](../README.md): the PostgreSQL connection
pool, the schema migrations, and the repositories the rest of the workspace uses
to read and write data.

It owns the single source of truth. Both the [`zyndeck-api`](../zyndeck-api)
server and the [`zyndeck-cli`](../zyndeck-cli) control surface go through it, and
the [`zyndeck-ingester`](../zyndeck-ingester) service drives ingestion jobs by
calling its transition methods. Rule embeddings live in Postgres via
[pgvector](https://github.com/pgvector/pgvector), so the migrations enable the
`vector` extension.

## What's in it

- **`Db`** — a cheaply-cloneable handle around a connection pool. `Db::connect`
  opens the pool, `Db::migrate` applies any outstanding migrations, and accessor
  methods (`games()`, `users()`, `ingestion_jobs()`, `step_runs()`,
  `transcripts()`, `chunks()`) hand out repositories.
- **Repositories** — one trait + Postgres implementation per entity
  (`GameRepository`, `UserRepository`, `IngestionJobRepository`,
  `IngestionStepRunRepository`, `IngestionTranscriptRepository`,
  `IngestionChunkRepository`). Each is generic-friendly (static dispatch) and has
  a `mockall` double behind the `mock` feature for downstream unit tests.
- **Ingestion transitions** — atomic job-lifecycle operations on `Db`
  (`start_job`, `validate_job`, `continue_job`, `restart_job`,
  `claim_pending_run`), each taking a `FOR UPDATE` lock on the job row so the
  pipeline stays consistent under concurrency. `listen_ingestion_jobs` exposes
  the `LISTEN`/`NOTIFY` stream the ingester reacts to.
- **Migrations** ([`migrations/`](migrations)) — plain SQL, embedded at compile
  time with `sqlx::migrate!` and applied in order (a `build.rs` re-expands them
  when a file changes). They define the `user`, `game`, `ingestion_job`,
  `ingestion_step_run`, `ingestion_transcript`, `ingestion_chunk` and
  `ingestion_chunk_embedding` (`vector(1024)`) tables. Queries live as `.sql`
  files under [`queries/`](queries), loaded with `include_str!`.

## Configure

`DbConfig` is a `clap` argument group that binaries flatten into their own CLI,
so configuration resolves in the order **CLI flags → environment variables →
defaults**:

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--database-url` | `DATABASE_URL` | _(required)_ | PostgreSQL connection URL. |
| `--db-max-connections` | `DB_MAX_CONNECTIONS` | `10` | Connection pool size. |

## Recreate the database

The crate ships a small operator binary, **`zyndeck-db-tool`**, whose `recreate`
command drops the target database, creates it fresh, and applies every
migration — handy for resetting a local or CI database to a clean,
fully-migrated state. It needs a connection URL whose role may create databases,
and it flattens the same `DbConfig` flags as everything else.

It is destructive, so it asks for confirmation unless you pass `--yes`:

```bash
DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
  cargo run -p zyndeck-db --bin zyndeck-db-tool -- recreate --yes
```

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--yes` / `-y` | `ASSUME_YES` | `false` | Skip the confirmation prompt. |
| `--log-filter` | `RUST_LOG` | `info` | `tracing` filter directive. |

## Test

The integration tests run against a **real Postgres** (the
[compose service](../docker-compose.yaml), which uses the pgvector image) and
isolate with `#[sqlx::test]` — a fresh database per test, migrations applied
automatically. Start Postgres and point `DATABASE_URL` at it:

```bash
docker compose up -d postgres

DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck \
  cargo test -p zyndeck-db
```
