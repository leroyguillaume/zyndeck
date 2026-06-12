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

The extraction stages load it from `/usr/local/lib/pdfium` by default (its own
subdirectory under the standard Linux location, where the Docker image installs
it). For local development, the fetch script puts it under `vendor/pdfium/`, so
point `PDFIUM_LIB_PATH` at `vendor/pdfium/lib`.

You can eyeball extraction against a real PDF with the `explore` example, which
prints the structured lines (`##` heading, `<>` icons) and the quality report:

```bash
cargo run -p zyndeck-ingester --example explore -- path/to/rules.pdf [first-page] [last-page]
```

## The service

The binary is a **long-running service**. It applies any outstanding database
migrations at startup, then runs until it receives `SIGINT`/`SIGTERM` and shuts
down gracefully.

Its role is to act on ingestion **jobs**, which are created out-of-band by the
[`zyndeck` CLI](../zyndeck-cli) writing directly to the database — the ingester
does not expose any job-management commands of its own. A job is modelled as an
`IngestionJob` that works through one step at a time (`extract` → `chunk` →
`embed`); see the [CLI README](../zyndeck-cli) for the job model (steps, modes,
run history) and how to create one.

### How it picks up jobs

The service reacts to jobs rather than polling for them:

- **`LISTEN`/`NOTIFY`.** A database trigger (migration `0009`) emits a
  notification on the `ingestion_job_created` channel whenever a job row is
  inserted, carrying the job's id. The service `LISTEN`s on that channel and
  drives each job as it arrives. Because `NOTIFY` is transactional, a job is
  announced exactly when it becomes visible — and never if the insert rolls
  back.
- **Startup sweep.** A notification fired while no service is listening is lost,
  so on startup the service also processes any job that was created but never
  run (it subscribes *before* sweeping, so nothing slips through the gap).

Claiming a job is atomic: beginning its first run takes the same `FOR UPDATE`
lock as the other transitions and relies on the one-active-run-per-job index, so
two service instances reacting to the same notification can't both process it.
An **auto** job is driven straight through to completion (or until a step fails);
a **manual** job has its first step run, then stops for review.

> **Note:** only the `extract` step is implemented so far. A job left mid-flight
> by a crash (its run stuck `running`) is **not** yet recovered — a reaper for
> stale runs is still to come.

## Configure

Configuration resolves in the order **CLI flags → environment variables →
defaults**. Every option is settable both ways.

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--log-filter` | `RUST_LOG` | `info` | `tracing` filter directive (e.g. `info`, `zyndeck_ingester=debug`). |
| `--database-url` | `DATABASE_URL` | _(required)_ | PostgreSQL connection URL. |
| `--db-max-connections` | `DB_MAX_CONNECTIONS` | `10` | Connection pool size. |

## Run

From the workspace root, with `-p`. The service needs a database (start the
compose Postgres first: `docker compose up -d postgres`):

```bash
export DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck

# Long-running service: migrates, then listens for and drives ingestion jobs
cargo run -p zyndeck-ingester
```

## Test

```bash
cargo test -p zyndeck-ingester
```
