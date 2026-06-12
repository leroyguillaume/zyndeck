//! Zyndeck ingestion service.
//!
//! Ingests game rules so the rest of Zyndeck can validate decks against them
//! and let the LLM answer questions about them. This is the binary entry
//! point: it wires up configuration, tracing, and graceful shutdown, then runs
//! the ingestion service.
//!
//! Jobs are *created* out-of-band by the `zyndeck` CLI writing to the database;
//! this service listens for them (Postgres `LISTEN`/`NOTIFY`) and drives each
//! one through the pipeline. The reusable pipeline stages live in the crate's
//! library ([`zyndeck_ingester::document`] / [`zyndeck_ingester::pdf`]).

use clap::Parser;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;
use zyndeck_core::{IngestionJob, IngestionStep, IngestionStepRun};
use zyndeck_db::{
    Advanced, Chunk, Db, DbConfig, IngestionChunkRepository, IngestionJobRepository,
    IngestionStepRunRepository, IngestionTranscriptRepository, NewChunk, StepOutcome,
};
use zyndeck_ingester::document::{self, ExtractedDocument};
use zyndeck_ingester::embed::{Embedder, OllamaEmbedder};
use zyndeck_ingester::{chunk, pdf};

/// Default Ollama server URL; overridable via `--ollama-url` / `OLLAMA_URL`.
const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Default embedding model tag (see the root README's models table);
/// overridable via `--embedding-model` / `EMBEDDING_MODEL`.
const DEFAULT_EMBEDDING_MODEL: &str = "bge-m3:567m";

/// How many chunks are embedded per request to Ollama. Batching keeps each
/// request (and its memory) bounded and gives incremental progress, rather than
/// embedding a whole document in one shot on a CPU-only host.
const EMBED_BATCH: usize = 16;

/// Command-line configuration. Resolution order is CLI flags → environment
/// variables → defaults; every option carries an `env` so nothing is
/// CLI-only.
#[derive(Debug, Parser)]
#[command(name = "zyndeck-ingester", version, about)]
struct Cli {
    #[command(flatten)]
    db: DbConfig,

    /// `tracing` filter directive (e.g. `info`, `zyndeck_ingester=debug`).
    #[arg(long = "log-filter", env = "RUST_LOG", default_value = "info")]
    log_filter: String,

    /// Directory holding the pdfium native library (used by the extract step).
    /// Fetch it with `scripts/fetch-pdfium.sh`.
    #[arg(
        long = "pdfium-lib-dir",
        env = "PDFIUM_LIB_PATH",
        default_value_t = pdf::DEFAULT_LIB_DIR.to_owned(),
    )]
    pdfium_lib_dir: String,

    /// Base URL of the Ollama server the embed step calls.
    #[arg(long = "ollama-url", env = "OLLAMA_URL", default_value = DEFAULT_OLLAMA_URL)]
    ollama_url: String,

    /// Ollama model tag used to embed chunks.
    #[arg(
        long = "embedding-model",
        env = "EMBEDDING_MODEL",
        default_value = DEFAULT_EMBEDDING_MODEL
    )]
    embedding_model: String,
}

/// Shared dependencies for processing jobs: the database, where to find the
/// pdfium library, and the embedder the embed step uses.
struct Ctx {
    db: Db,
    pdfium_lib_dir: String,
    embedder: OllamaEmbedder,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&cli.log_filter))
        .init();

    tracing::info!(log_filter = %cli.log_filter, "starting zyndeck-ingester");

    // Connect and apply migrations at startup.
    let db = Db::connect(&cli.db).await?;
    db.migrate().await?;

    let embedder = OllamaEmbedder::new(cli.ollama_url, cli.embedding_model);

    run(Ctx {
        db,
        pdfium_lib_dir: cli.pdfium_lib_dir,
        embedder,
    })
    .await
}

/// Runs as a long-running service: listens for created jobs and drives each
/// one, until a `SIGINT`/`SIGTERM` asks it to stop.
async fn run(ctx: Ctx) -> anyhow::Result<()> {
    // Subscribe before sweeping, so a job created during the sweep is still seen
    // (as a queued notification — a redundant one is a harmless no-op).
    let mut listener = ctx.db.listen_ingestion_jobs().await?;
    sweep(&ctx).await?;

    tracing::info!("waiting for ingestion jobs");
    loop {
        tokio::select! {
            // Finish the job in flight first (we are not inside `process` here),
            // then stop accepting new ones and exit cleanly.
            _ = shutdown_signal() => {
                tracing::info!("shutdown signal received, exiting");
                return Ok(());
            }
            job_id = listener.recv() => {
                let job_id = job_id?;
                if let Err(e) = process(&ctx, job_id).await {
                    tracing::error!(%job_id, error = %e, "failed to process ingestion job");
                }
            }
        }
    }
}

/// Processes any jobs with work enqueued while the service was down (their
/// `NOTIFY` was dropped, since no one was listening).
async fn sweep(ctx: &Ctx) -> anyhow::Result<()> {
    let job_ids = ctx.db.pending_run_job_ids().await?;
    if !job_ids.is_empty() {
        tracing::info!(
            count = job_ids.len(),
            "sweeping jobs enqueued while offline"
        );
    }
    for job_id in job_ids {
        if let Err(e) = process(ctx, job_id).await {
            tracing::error!(%job_id, error = %e, "failed to process ingestion job during sweep");
        }
    }
    Ok(())
}

/// Claims a job's pending run and, if there was one, drives it through the
/// pipeline.
async fn process(ctx: &Ctx, job_id: Uuid) -> anyhow::Result<()> {
    match ctx.db.claim_pending_run(job_id).await? {
        None => {
            tracing::debug!(%job_id, "ingestion job has no pending run to claim");
            Ok(())
        }
        Some(run) => {
            let job = load(ctx, job_id).await?;
            tracing::info!(%job_id, step = run.step.as_str(), "processing ingestion job");
            drive(ctx, job, run).await
        }
    }
}

/// Runs the claimed step. After `extract`, stops and leaves the job awaiting
/// human validation of the transcript; for the phase-2 steps it keeps advancing
/// (`chunk → embed`) until the job completes or a step fails.
async fn drive(ctx: &Ctx, mut job: IngestionJob, mut run: IngestionStepRun) -> anyhow::Result<()> {
    loop {
        run_and_finish(ctx, &job, &run).await?;

        if run.step == IngestionStep::Extract {
            tracing::info!(
                job_id = %job.id,
                "transcription complete, awaiting human validation",
            );
            return Ok(());
        }

        match ctx.db.continue_job(job.id).await? {
            Advanced::Completed => {
                tracing::info!(job_id = %job.id, "ingestion job completed");
                return Ok(());
            }
            Advanced::Running(next) => {
                tracing::info!(
                    job_id = %job.id,
                    step = next.step.as_str(),
                    "continuing ingestion job",
                );
                job = load(ctx, job.id).await?;
                run = next;
            }
        }
    }
}

/// Loads a job by id, failing if it has vanished (it was just operated on).
async fn load(ctx: &Ctx, job_id: Uuid) -> anyhow::Result<IngestionJob> {
    ctx.db
        .ingestion_jobs()
        .find_by_id(job_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("ingestion job {job_id} disappeared"))
}

/// Runs an already-begun step's body, then records the run as succeeded or
/// failed.
async fn run_and_finish(
    ctx: &Ctx,
    job: &IngestionJob,
    run: &IngestionStepRun,
) -> anyhow::Result<()> {
    tracing::info!(
        job_id = %run.job_id,
        run_id = %run.id,
        step = run.step.as_str(),
        attempt = run.attempt,
        "step run started",
    );

    let outcome = run_step(ctx, job).await;

    let recorded = match &outcome {
        Ok(()) => StepOutcome::Succeeded,
        Err(e) => StepOutcome::Failed {
            error: e.to_string(),
        },
    };

    // `finish` only updates a still-active run; if it returns `None` the run was
    // ended out from under us (e.g. externally aborted), so don't claim success.
    if ctx.db.step_runs().finish(run.id, recorded).await?.is_none() {
        anyhow::bail!("step run was ended externally");
    }

    match &outcome {
        Ok(()) => tracing::info!(run_id = %run.id, attempt = run.attempt, "step run succeeded"),
        Err(e) => {
            tracing::warn!(run_id = %run.id, attempt = run.attempt, error = %e, "step run failed")
        }
    }

    outcome
}

/// Runs the job's current [`IngestionStep`].
async fn run_step(ctx: &Ctx, job: &IngestionJob) -> anyhow::Result<()> {
    match job.step {
        IngestionStep::Extract => extract(ctx, job).await,
        IngestionStep::Chunk => chunk_step(ctx, job).await,
        IngestionStep::Embed => embed_step(ctx, job).await,
        IngestionStep::Completed => anyhow::bail!("job is already completed"),
    }
}

/// The extract step: read the source PDF, structure it, and store the resulting
/// reviewable transcript. The PDF parsing is blocking and pdfium is not `Send`,
/// so it runs on a blocking thread (binding pdfium there).
async fn extract(ctx: &Ctx, job: &IngestionJob) -> anyhow::Result<()> {
    let lib_dir = ctx.pdfium_lib_dir.clone();
    let source = job.source.clone();

    let document = tokio::task::spawn_blocking(move || -> anyhow::Result<ExtractedDocument> {
        let pdfium = pdf::bind(&lib_dir)?;
        let pages = pdf::read_pages(&pdfium, &source)?;
        Ok(document::structure(&pages))
    })
    .await??;

    tracing::info!(
        job_id = %job.id,
        kept = document.report.kept,
        dropped = document.report.dropped_garbled,
        "extracted transcript",
    );

    ctx.db
        .transcripts()
        .upsert(job.id, document.to_markdown())
        .await?;
    Ok(())
}

/// The chunk step: split the (reviewed) transcript into retrieval chunks and
/// store them, replacing any from a previous run.
async fn chunk_step(ctx: &Ctx, job: &IngestionJob) -> anyhow::Result<()> {
    let transcript = ctx
        .db
        .transcripts()
        .find(job.id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("job {} has no transcript to chunk", job.id))?;

    let chunks = chunk::split(&transcript);
    if chunks.is_empty() {
        anyhow::bail!("chunking produced no chunks (is the transcript empty?)");
    }

    // Positions and pages are small in practice, but convert fallibly rather
    // than truncating silently into the `integer` columns.
    let new_chunks = chunks
        .into_iter()
        .enumerate()
        .map(|(index, c)| {
            Ok(NewChunk {
                position: i32::try_from(index)?,
                heading: c.heading,
                page: i32::try_from(c.page)?,
                content: c.content,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let count = new_chunks.len();
    ctx.db.chunks().replace(job.id, new_chunks).await?;
    tracing::info!(job_id = %job.id, count, "chunked transcript");
    Ok(())
}

/// The embed step: embed each stored chunk and persist its vector. Idempotent —
/// embeddings are upserted, so a re-run overwrites rather than duplicates.
async fn embed_step(ctx: &Ctx, job: &IngestionJob) -> anyhow::Result<()> {
    let chunks = ctx.db.chunks().find_by_job(job.id).await?;
    if chunks.is_empty() {
        anyhow::bail!("job {} has no chunks to embed", job.id);
    }
    let total = chunks.len();

    let mut embedded = 0;
    for batch in chunks.chunks(EMBED_BATCH) {
        let inputs = batch.iter().map(embedding_input).collect();
        let vectors = ctx.embedder.embed(inputs).await?;
        for (chunk, vector) in batch.iter().zip(vectors) {
            ctx.db.chunks().store_embedding(chunk.id, vector).await?;
        }
        embedded += batch.len();
        tracing::debug!(job_id = %job.id, embedded, total, "embedding progress");
    }

    tracing::info!(job_id = %job.id, count = total, "embedded chunks");
    Ok(())
}

/// The text embedded for a chunk: the section heading (for retrieval context)
/// followed by the body, or just the body when the chunk has no heading.
fn embedding_input(chunk: &Chunk) -> String {
    if chunk.heading.is_empty() {
        chunk.content.clone()
    } else {
        format!("{}\n\n{}", chunk.heading, chunk.content)
    }
}

/// Completes on `SIGTERM` or `SIGINT`, so the service drains cleanly whether an
/// orchestrator stops it or a developer hits Ctrl-C.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");

    tokio::select! {
        _ = interrupt.recv() => tracing::debug!("received SIGINT"),
        _ = terminate.recv() => tracing::debug!("received SIGTERM"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// `--database-url` is required (a top-level flattened `DbConfig`), so every
    /// parse needs one; the value is never connected to.
    fn argv(rest: &[&'static str]) -> Vec<&'static str> {
        let mut args = vec!["zyndeck-ingester", "--database-url", "postgresql://test"];
        args.extend_from_slice(rest);
        args
    }

    #[test]
    fn cli_definition_is_valid() {
        // Catches malformed `clap` derive wiring (duplicate args, bad defaults).
        Cli::command().debug_assert();
    }

    #[test]
    fn log_filter_defaults_to_info() {
        let cli = Cli::parse_from(argv(&[]));
        assert_eq!(cli.log_filter, "info");
    }

    #[test]
    fn log_filter_overridable_from_flag() {
        let cli = Cli::parse_from(argv(&["--log-filter", "debug"]));
        assert_eq!(cli.log_filter, "debug");
    }
}
