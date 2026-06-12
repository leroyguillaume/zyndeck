# zyndeck-cli

The command-line interface of [Zyndeck](../README.md). The crate is
`zyndeck-cli`, but the compiled binary is named **`zyndeck`**.

It is Zyndeck's **control surface**: it drives the system by writing directly to
the database (the [`zyndeck-db`](../zyndeck-db) layer), and the services — like
[`zyndeck-ingester`](../zyndeck-ingester) — then act on what they find there.

## Commands

| Command | Description |
| --- | --- |
| `ingestion start` | Start a new rule-ingestion job for a document. Prints the new job's id to stdout. |

### The ingestion job model

Ingestion is modelled as a **job** (`IngestionJob`) that works through one step
at a time — `extract` → `chunk` → `embed`. A freshly created job starts on
`extract`; the ingestion service then runs the steps and records the outcome of
each attempt in a **run history** (`ingestion_step_run`).

A job has a **mode** that decides what happens once a step succeeds, chosen at
creation with `--mode` (default `auto`):

- **`auto`** (default) — the job advances through the steps on its own, running
  each in turn, and stops only when the pipeline **completes** or a step
  **fails**.
- **`manual`** — the job stops after each step so its output (notably the
  extracted transcript) can be reviewed and corrected before the next step runs.

A job stores its inputs (game, source document, language) so a step can be
re-run without re-supplying them. `ingestion start` only **creates** the job
row; running its steps is the ingestion service's job.

## Configure

Configuration resolves in the order **CLI flags → environment variables →
defaults**. Every option is settable both ways.

Global (every command):

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--log-filter` | `RUST_LOG` | `info` | `tracing` filter directive (e.g. `info`, `zyndeck=debug`). |
| `--database-url` | `DATABASE_URL` | _(required)_ | PostgreSQL connection URL. |
| `--db-max-connections` | `DB_MAX_CONNECTIONS` | `10` | Connection pool size. |

The CLI connects to the database and **applies any outstanding migrations at
startup**, so it works against a fresh database without a service having run
first.

`ingestion start`:

| Flag | Environment variable | Required | Description |
| --- | --- | --- | --- |
| `--game-id` | `GAME_ID` | yes | Identifier (UUID) of the game the rules belong to. |
| `--file` | `RULES_FILE` | yes | Path to the file holding the rules to ingest. |
| `--language` | `RULES_LANGUAGE` | yes | ISO 639-1 language of the document (e.g. `fr`, `en`). |
| `--mode` | `INGESTION_MODE` | no (`auto`) | How the job advances between steps: `auto` runs straight through until it completes or a step fails; `manual` stops after each step for review. |
| `--created-by` | `CREATED_BY` | no | Identifier (UUID) of the user starting the job; omitted for anonymous CLI runs. |

## Run

From the workspace root, with `-p`. The CLI needs a database (start the compose
Postgres first: `docker compose up -d postgres`):

```bash
export DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck

# Start an ingestion job (default auto mode); prints the new job id
cargo run -p zyndeck-cli -- ingestion start \
  --game-id 00000000-0000-0000-0000-000000000001 \
  --file path/to/rules.pdf \
  --language en

# Or start a job that stops after each step for review
cargo run -p zyndeck-cli -- ingestion start \
  --game-id 00000000-0000-0000-0000-000000000001 \
  --file path/to/rules.pdf \
  --language en \
  --mode manual
```

## Test

```bash
cargo test -p zyndeck-cli
```
