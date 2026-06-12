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
| `ingestion edit` | Open a job's transcript in `$EDITOR` and save any edits. |
| `ingestion validate` | Validate a job's transcript to continue the pipeline (chunk + embed). |
| `ingestion restart` | Re-run a job's transcription (before it has been validated). |

### The ingestion job model

Ingestion is modelled as a **job** (`IngestionJob`) that works through one step
at a time — `extract` → `chunk` → `embed` — recording the outcome of each
attempt in a **run history** (`ingestion_step_run`). It runs in **two phases
with a human gate** between them:

1. **Transcription** — the `extract` step reads the document into a reviewable
   transcript, and the job then **stops** and waits.
2. **Validation** — a human reviews (and optionally edits) the transcript and
   **validates** it. Only then do `chunk` and `embed` run, straight through to
   completion.

Review is two steps: `ingestion edit` opens the transcript in your editor as
many times as you like, and `ingestion validate` opens the gate once you're
happy. Before validation the transcription can also be **restarted**
(`ingestion restart`) to re-extract from scratch. Once validated, the job is
locked into phase 2 and cannot be edited or restarted.

A job stores its inputs (game, source document, language) so the extract step
can be re-run without re-supplying them. The CLI only **writes** to the database;
running the steps is the [`zyndeck-ingester`](../zyndeck-ingester) service's job.

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
| `--created-by` | `CREATED_BY` | no | Identifier (UUID) of the user starting the job; omitted for anonymous CLI runs. |

`ingestion edit`:

| Flag | Environment variable | Required | Description |
| --- | --- | --- | --- |
| `--job-id` | `JOB_ID` | yes | Identifier (UUID) of the job whose transcript to edit. |

`edit` writes the transcript to a temporary `.md` file and opens it in your
editor — `$VISUAL`, then `$EDITOR`, falling back to `vi` — saving any edits back.
It does **not** wait: if transcription has not finished yet (no transcript), it
errors out — try again once the service has produced one, or `ingestion restart`
if it failed.

`ingestion validate`:

| Flag | Environment variable | Required | Description |
| --- | --- | --- | --- |
| `--job-id` | `JOB_ID` | yes | Identifier (UUID) of the job whose transcript to validate. |

`validate` opens the human gate: the service continues through `chunk` and
`embed`. There is no undo after validation; use `ingestion restart` to redo the
transcription instead.

`ingestion restart`:

| Flag | Environment variable | Required | Description |
| --- | --- | --- | --- |
| `--job-id` | `JOB_ID` | yes | Identifier (UUID) of the job whose transcription to restart. |

Re-runs the `extract` step. Only allowed while the job is still in the
transcription phase — once validated, the job is locked into `chunk` + `embed`.

## Run

From the workspace root, with `-p`. The CLI needs a database (start the compose
Postgres first: `docker compose up -d postgres`):

```bash
export DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck

# Start an ingestion job; prints the new job id. The service transcribes it,
# then stops to await validation.
cargo run -p zyndeck-cli -- ingestion start \
  --game-id 00000000-0000-0000-0000-000000000001 \
  --file path/to/rules.pdf \
  --language en

# Once transcription is done: edit the transcript in $EDITOR
cargo run -p zyndeck-cli -- ingestion edit \
  --job-id 00000000-0000-0000-0000-000000000001

# Happy with it? Validate to run chunk + embed
cargo run -p zyndeck-cli -- ingestion validate \
  --job-id 00000000-0000-0000-0000-000000000001

# Or re-run the transcription instead (before validating it)
cargo run -p zyndeck-cli -- ingestion restart \
  --job-id 00000000-0000-0000-0000-000000000001
```

## Test

```bash
cargo test -p zyndeck-cli
```
