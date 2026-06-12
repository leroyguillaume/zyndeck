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

## Chunking and embedding

Once a transcript is validated, the remaining two steps turn it into searchable
vectors:

- [`chunk`](src/chunk.rs) — pure splitting of the (reviewed) Markdown transcript
  into retrieval chunks. It is heading-aware (a chunk never crosses a `##`
  section boundary) and caps a chunk at ~1200 characters, splitting a long
  section at line boundaries. Each chunk keeps its section heading and the source
  page it starts on, so retrieval can cite where a rule came from. Chunks are
  stored in the `ingestion_chunk` table, replaced wholesale on a re-run.
- [`embed`](src/embed.rs) — turns each chunk into a vector via a local
  [Ollama](https://ollama.com/) server (`BGE-M3` by default, see the
  [root README](../README.md#models)), reached over plain HTTP. The chunk's
  heading is prepended to its body so the vector captures the section context.
  Vectors are upserted into the `ingestion_chunk_embedding` pgvector column
  (dimension 1024, cosine HNSW index), keyed by chunk, ready for the API's
  nearest-neighbour retrieval.

## The service

The binary is a **long-running service**. It applies any outstanding database
migrations at startup, then runs until it receives `SIGINT`/`SIGTERM` and shuts
down gracefully.

Its role is to act on ingestion **jobs**, which are created out-of-band by the
[`zyndeck` CLI](../zyndeck-cli) writing directly to the database — the ingester
does not expose any job-management commands of its own. A job is modelled as an
`IngestionJob` that works through one step at a time (`extract` → `chunk` →
`embed`), in two phases with a human validation gate after `extract`; see the
[CLI README](../zyndeck-cli) for the job model (phases, validation, run history)
and how to create one.

### How it picks up work

The service reacts to work rather than polling for it. The unit of work it
executes is a **pending step run**: every action that needs the service —
creating a job, validating a transcript, restarting a transcription — enqueues a
`pending` run and fires one notification.

- **`LISTEN`/`NOTIFY`.** Both the job-creation trigger and the validate/restart
  transitions emit a notification on the `ingestion_job_ready` channel, carrying
  the job's id. The service `LISTEN`s on it and, for each id, **claims** the
  job's pending run — an atomic `pending → running` update, so duplicate
  notifications (or several service instances) cannot execute the same run twice.
  Because `NOTIFY` is transactional, the run is announced exactly when it becomes
  visible — and never if the enqueuing transaction rolls back.
- **Startup sweep.** A notification fired while no service is listening is lost,
  so on startup the service also processes any job left with a pending run (it
  subscribes *before* sweeping, so nothing slips through the gap).

After running `extract`, the service **stops** and leaves the job awaiting human
validation. Once validated, it claims the enqueued `chunk` run and chains
straight through `chunk → embed` to completion (or until a step fails).

> **Note:** a job left mid-flight by a crash (its run stuck `running`) is **not**
> yet recovered — a reaper for stale runs is still to come. A `chunk` or `embed`
> run that *fails* likewise has no retry path yet (`restart` only re-runs the
> transcription phase), so a failed phase-2 run currently needs a fresh job.

## Configure

Configuration resolves in the order **CLI flags → environment variables →
defaults**. Every option is settable both ways.

| Flag | Environment variable | Default | Description |
| --- | --- | --- | --- |
| `--log-filter` | `RUST_LOG` | `info` | `tracing` filter directive (e.g. `info`, `zyndeck_ingester=debug`). |
| `--database-url` | `DATABASE_URL` | _(required)_ | PostgreSQL connection URL. |
| `--db-max-connections` | `DB_MAX_CONNECTIONS` | `10` | Connection pool size. |
| `--pdfium-lib-dir` | `PDFIUM_LIB_PATH` | `/usr/local/lib/pdfium` | Directory holding the pdfium native library (extract step). |
| `--ollama-url` | `OLLAMA_URL` | `http://localhost:11434` | Base URL of the Ollama server (embed step). |
| `--embedding-model` | `EMBEDDING_MODEL` | `bge-m3:567m` | Ollama model tag used to embed chunks. |

## Run

From the workspace root, with `-p`. The service needs Postgres (always) and, for
the embed step, the Ollama server with the embedding model pulled — `docker
compose up -d` starts both and pulls the models:

```bash
export DATABASE_URL=postgresql://zyndeck:zyndeck@localhost:5432/zyndeck

# Long-running service: migrates, then listens for and drives ingestion jobs
cargo run -p zyndeck-ingester
```

## Test

```bash
cargo test -p zyndeck-ingester
```
