# zyndeck-ingester

The ingestion service of [Zyndeck](../README.md). It ingests game rules so the
rest of Zyndeck can validate decks against them and let the LLM answer questions
about how a game's rules work.

Ingested rules are chunked and embedded into the pgvector store managed by
[`zyndeck-db`](../zyndeck-db), where the API's question-answering can retrieve
them. See the [root README](../README.md#models) for the embedding and
generation models this targets.

## PDF extraction

Rulebooks are PDFs that can contain anything: multi-column layouts, decorative
fonts, card-art captions overlapping the text, and icon glyphs. Extraction runs
text-only and fully local, in testable stages:

- [`pdf`](src/pdf.rs) — the [pdfium](https://pdfium.googlesource.com/pdfium/)
  I/O boundary, turning a PDF into raw text segments with their geometry and
  font (via [`pdfium-render`](https://crates.io/crates/pdfium-render)).
- [`document`](src/document.rs) — pure, library-free heuristics that turn those
  segments into ordered, classified, cleaned lines plus a quality report:
  reading order from column-aware line grouping, heading detection from the
  *relative* font family (not hard-coded font names), icon detection from
  Private Use Area glyphs, caption-bleed stripping, and dropping text mangled by
  broken font subsets (counted in the report rather than embedded as noise).

### pdfium native library

`pdfium-render` loads the pdfium shared library at runtime; it is platform
specific and **not** committed. Fetch it once per machine into `vendor/pdfium/`:

```bash
./scripts/fetch-pdfium.sh
```

The ingester loads it from `/usr/local/lib/pdfium` by default (its own
subdirectory under the standard Linux location, where the Docker image installs
it). For local development, the fetch script puts it under `vendor/pdfium/`, so
point `PDFIUM_LIB_PATH` (or `--pdfium-lib-dir`) at `vendor/pdfium/lib`.

You can eyeball extraction against a real PDF with the `explore` example, which
prints the structured lines (`##` heading, `<>` icons) and the quality report:

```bash
cargo run -p zyndeck-ingester --example explore -- path/to/rules.pdf [first-page] [last-page]
```

## Commands

The binary is a subcommand CLI:

| Command | Description |
| --- | --- |
| `run` | Run as a long-running service: applies migrations at startup, then (idle for now) sits until it receives `SIGINT`/`SIGTERM` and shuts down gracefully. |
| `ingest start` | Create a new ingestion job for a document and run its first step. |
| `ingest continue` | Continue an existing job by running its next step. |
| `ingest restart` | Re-run a job's most recently completed step without advancing it. |
| `ingest stop` | Stop a job's currently running step. |

Ingestion is modelled as a **job** (`IngestionJob`) that works through one step
at a time — `extract` → `chunk` → `embed`. Each command runs a single step and
returns, so the output of a step (notably the extracted transcript) can be
reviewed, and corrected if needed, before the next step is run.

- `start` creates a job and runs its first step (`extract`).
- `continue` advances to the next step and runs it — but only once the current
  step has **succeeded**; otherwise it tells you to `restart` first.
- `restart` re-runs the current step as a fresh attempt, to retry a failed step
  or redo one whose output was unsatisfactory.

Every execution is recorded in a **run history** (`ingestion_step_run` table):
one row per attempt, with its status (`pending` / `running` / `succeeded` /
`failed` / `aborted`), `started_at`, `completed_at`, `error` (on failure), and a
per-step `attempt` counter that increments on each `restart`.

`stop` only writes `aborted` to the database; the process running the step polls
its run and, seeing the change, abandons the step on its own (nothing is
overwritten). An aborted step behaves like a failed one: `continue` is blocked
until you `restart` it.

A job runs **at most one step at a time** and can never be run in parallel. Two
mechanisms enforce this, both in the database: a partial unique index allows only
one active (`pending`/`running`) run per job, and each transition (`start` /
`continue` / `restart`) takes a `SELECT … FOR UPDATE` lock on the job row so its
read-check-advance is atomic — the lock is held only for the brief transition,
never while the step itself runs.

Jobs and their history are **persisted** in Postgres, so a job created by `start`
can be advanced later by `continue` / `restart` in a separate run. A job stores
its inputs (game, source document, language) so the extract step can be re-run
without re-supplying them.

The **extract** step reads the source PDF (see [PDF extraction](#pdf-extraction)
above), structures it, and stores a Markdown **transcript** in the
`ingestion_transcript` table — the reviewable artifact the later steps consume.

> **Note:** only the `extract` step is implemented so far; `chunk` and `embed`
> still record a failed run.

## Configure

Configuration resolves in the order **CLI flags → environment variables →
defaults**. Every option is settable both ways.

Global (every subcommand, including `run`):

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--log-filter` | `RUST_LOG` | `info` | `tracing` filter directive (e.g. `info`, `zyndeck_ingester=debug`). |
| `--database-url` | `DATABASE_URL` | _(required)_ | PostgreSQL connection URL. |
| `--db-max-connections` | `DB_MAX_CONNECTIONS` | `10` | Connection pool size. |

The ingester connects to the database and **applies any outstanding migrations
at startup**, for every subcommand (including `run`).

`ingest start`:

| Flag | Environment variable | Required | Description |
| --- | --- | --- | --- |
| `--game-id` | `GAME_ID` | yes | Identifier (UUID) of the game the rules belong to. |
| `--file` | `RULES_FILE` | yes | Path to the file holding the rules to ingest. |
| `--language` | `RULES_LANGUAGE` | yes | ISO 639-1 language of the document (e.g. `fr`, `en`). |
| `--created-by` | `CREATED_BY` | no | Identifier (UUID) of the user starting the job; omitted for anonymous CLI runs. |

`ingest continue` / `ingest restart` / `ingest stop`:

| Flag | Environment variable | Required | Description |
| --- | --- | --- | --- |
| `--job-id` | `JOB_ID` | yes | Identifier (UUID) of the job to advance (`continue`), whose last step to re-run (`restart`), or whose running step to stop (`stop`). |

The `ingest` subcommands additionally take where to find the pdfium library:

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--pdfium-lib-dir` | `PDFIUM_LIB_PATH` | `/usr/local/lib/pdfium` | Directory holding the pdfium native library (used by the extract step). For local dev, point it at `vendor/pdfium/lib`. |

## Run

From the workspace root, with `-p`. Every subcommand needs a database (start the
compose Postgres first: `docker compose up -d postgres`):

```bash
export DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck

# Long-running service (applies migrations at startup, then idles)
cargo run -p zyndeck-ingester -- run

# Start a job and run its first step
cargo run -p zyndeck-ingester -- ingest start \
  --game-id 00000000-0000-0000-0000-000000000001 \
  --file path/to/rules.pdf \
  --language en

# Later, advance that job one step
cargo run -p zyndeck-ingester -- ingest continue \
  --job-id <job-id-from-start>

# Or redo the last step if its output was bad
cargo run -p zyndeck-ingester -- ingest restart \
  --job-id <job-id-from-start>

# Stop a step that is currently running (from another shell)
cargo run -p zyndeck-ingester -- ingest stop \
  --job-id <job-id-from-start>
```

## Test

```bash
cargo test -p zyndeck-ingester
```
