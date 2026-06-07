//! Zyndeck ingestion service.
//!
//! Ingests game rules so the rest of Zyndeck can validate decks against them
//! and let the LLM answer questions about them. This is the binary entry
//! point: it wires up configuration, tracing, and graceful shutdown, then runs
//! the ingestion loop.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;
use zyndeck_core::{IngestionJob, IngestionStep, IngestionStepRun, LanguageCode};
use zyndeck_db::{
    Advanced, Db, DbConfig, IngestionJobRepository, IngestionStepRunRepository,
    IngestionTranscriptRepository, NewIngestionJob, StepOutcome,
};
use zyndeck_ingester::document::{self, ExtractedDocument};
use zyndeck_ingester::pdf;

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

    #[command(subcommand)]
    command: Command,
}

/// What the ingester should do this invocation.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run as a long-running service, ingesting game rules as they arrive.
    Run,

    /// Manage rule-ingestion jobs.
    Ingest {
        /// Directory holding the pdfium native library (used by the extract
        /// step). Fetch it with `scripts/fetch-pdfium.sh`.
        #[arg(
            long = "pdfium-lib-dir",
            env = "PDFIUM_LIB_PATH",
            default_value_t = pdf::DEFAULT_LIB_DIR.to_owned(),
        )]
        pdfium_lib_dir: String,

        #[command(subcommand)]
        action: IngestAction,
    },
}

/// Shared dependencies for the ingest subcommands: the database and where to
/// find the pdfium library.
struct Ctx {
    db: Db,
    pdfium_lib_dir: String,
}

/// Drive an [`IngestionJob`] one step at a time. Each invocation runs a single
/// step and returns, so the output (notably the extracted transcript) can be
/// reviewed before the next step is run.
#[derive(Debug, Subcommand)]
enum IngestAction {
    /// Create a new ingestion job for a document and run its first step.
    Start {
        /// Identifier of the game the rules belong to.
        #[arg(long = "game-id", env = "GAME_ID")]
        game_id: Uuid,

        /// Path to the file holding the rules to ingest.
        #[arg(long = "file", env = "RULES_FILE")]
        file: PathBuf,

        /// ISO 639-1 language of the document (e.g. `fr`, `en`).
        #[arg(long = "language", env = "RULES_LANGUAGE")]
        language: LanguageCode,

        /// Id of the user starting the job, if any (CLI runs may be anonymous).
        #[arg(long = "created-by", env = "CREATED_BY")]
        created_by: Option<Uuid>,
    },

    /// Continue an existing job by running its next step.
    Continue {
        /// Identifier of the job to advance.
        #[arg(long = "job-id", env = "JOB_ID")]
        job_id: Uuid,
    },

    /// Re-run a job's most recently completed step (e.g. redo a bad extraction)
    /// without advancing it.
    Restart {
        /// Identifier of the job whose last step to re-run.
        #[arg(long = "job-id", env = "JOB_ID")]
        job_id: Uuid,
    },

    /// Stop a job's currently running step. Only writes to the database; the
    /// process running the step notices and stops on its own.
    Stop {
        /// Identifier of the job whose running step to stop.
        #[arg(long = "job-id", env = "JOB_ID")]
        job_id: Uuid,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&cli.log_filter))
        .init();

    tracing::info!(log_filter = %cli.log_filter, "starting zyndeck-ingester");

    // Connect and apply migrations at startup, for every subcommand.
    let db = Db::connect(&cli.db).await?;
    db.migrate().await?;

    match cli.command {
        Command::Run => run().await,
        Command::Ingest {
            pdfium_lib_dir,
            action,
        } => {
            let ctx = Ctx { db, pdfium_lib_dir };
            match action {
                IngestAction::Start {
                    game_id,
                    file,
                    language,
                    created_by,
                } => ingest_start(&ctx, game_id, file, language, created_by).await,
                IngestAction::Continue { job_id } => ingest_continue(&ctx, job_id).await,
                IngestAction::Restart { job_id } => ingest_restart(&ctx, job_id).await,
                IngestAction::Stop { job_id } => ingest_stop(&ctx, job_id).await,
            }
        }
    }
}

/// Runs as a long-running service.
async fn run() -> anyhow::Result<()> {
    // No ingestion work yet — sit idle until asked to stop so the binary
    // already behaves like a well-mannered long-running service.
    shutdown_signal().await;

    tracing::info!("shutdown signal received, exiting");
    Ok(())
}

/// How often a running step polls the database to see whether it was stopped.
const ABORT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Creates a job (atomically beginning its first step's run) and runs that step.
async fn ingest_start(
    ctx: &Ctx,
    game_id: Uuid,
    file: PathBuf,
    language: LanguageCode,
    created_by: Option<Uuid>,
) -> anyhow::Result<()> {
    let (job, run) = ctx
        .db
        .start_job(NewIngestionJob {
            game_id,
            source: file,
            language,
            created_by,
        })
        .await?;

    tracing::info!(
        job_id = %job.id,
        step = job.step.as_str(),
        game_id = %job.game_id,
        source = %job.source.display(),
        language = job.language.as_str(),
        created_by = created_by.map(|id| id.to_string()).as_deref().unwrap_or("anonymous"),
        "ingestion job created",
    );

    run_and_finish(ctx, job, run).await
}

/// Advances a job to its next step (atomically, behind a row lock) and runs it.
async fn ingest_continue(ctx: &Ctx, job_id: Uuid) -> anyhow::Result<()> {
    match ctx.db.continue_job(job_id).await? {
        Advanced::Completed => {
            tracing::info!(%job_id, "ingestion job completed");
            Ok(())
        }
        Advanced::Running(run) => {
            tracing::info!(%job_id, step = run.step.as_str(), "continuing ingestion job");
            let job = load(ctx, job_id).await?;
            run_and_finish(ctx, job, run).await
        }
    }
}

/// Re-runs a job's current step as a fresh attempt (atomically, behind a row
/// lock), to retry a failed step or redo a bad one.
async fn ingest_restart(ctx: &Ctx, job_id: Uuid) -> anyhow::Result<()> {
    let run = ctx.db.restart_job(job_id).await?;
    tracing::info!(%job_id, step = run.step.as_str(), "restarting ingestion job step");
    let job = load(ctx, job_id).await?;
    run_and_finish(ctx, job, run).await
}

/// Stops a job's running step by writing `aborted` to the database. Write-only:
/// the process executing the step polls and stops itself.
async fn ingest_stop(ctx: &Ctx, job_id: Uuid) -> anyhow::Result<()> {
    match ctx.db.step_runs().abort(job_id).await? {
        Some(run) => {
            tracing::info!(
                %job_id,
                run_id = %run.id,
                step = run.step.as_str(),
                attempt = run.attempt,
                "stopped running step",
            );
            Ok(())
        }
        None => anyhow::bail!("ingestion job {job_id} has no running step to stop"),
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
/// failed — unless it is stopped meanwhile. While the body runs, the run's
/// status is polled; if it becomes `aborted` (via `stop`), the body is cancelled
/// and nothing is overwritten.
async fn run_and_finish(ctx: &Ctx, job: IngestionJob, run: IngestionStepRun) -> anyhow::Result<()> {
    tracing::info!(
        job_id = %run.job_id,
        run_id = %run.id,
        step = run.step.as_str(),
        attempt = run.attempt,
        "step run started",
    );

    let outcome = tokio::select! {
        outcome = run_step(ctx, &job) => outcome,
        result = watch_for_abort(&ctx.db, run.id) => return result,
    };

    let recorded = match &outcome {
        Ok(()) => StepOutcome::Succeeded,
        Err(e) => StepOutcome::Failed {
            error: e.to_string(),
        },
    };

    // `finish` only updates a still-running run, so if `stop` won the race the
    // run stays `aborted` and we report that rather than a bogus success.
    if ctx.db.step_runs().finish(run.id, recorded).await?.is_none() {
        anyhow::bail!("step run was stopped");
    }

    match &outcome {
        Ok(()) => tracing::info!(run_id = %run.id, attempt = run.attempt, "step run succeeded"),
        Err(e) => {
            tracing::warn!(run_id = %run.id, attempt = run.attempt, error = %e, "step run failed")
        }
    }

    outcome
}

/// Polls the run until it is no longer active — i.e. it was stopped — then
/// returns an error so the caller abandons the step. Never returns `Ok`.
async fn watch_for_abort(db: &Db, run_id: Uuid) -> anyhow::Result<()> {
    loop {
        tokio::time::sleep(ABORT_POLL_INTERVAL).await;
        match db.step_runs().find(run_id).await? {
            Some(run) if run.status.is_active() => continue,
            _ => {
                tracing::warn!(%run_id, "step run was stopped; abandoning");
                anyhow::bail!("step run was stopped");
            }
        }
    }
}

/// Runs the job's current [`IngestionStep`].
async fn run_step(ctx: &Ctx, job: &IngestionJob) -> anyhow::Result<()> {
    match job.step {
        IngestionStep::Extract => extract(ctx, job).await,
        // TODO: chunk the reviewed transcript, then embed the chunks.
        IngestionStep::Chunk => anyhow::bail!("chunking step is not implemented yet"),
        IngestionStep::Embed => anyhow::bail!("embedding step is not implemented yet"),
        IngestionStep::Completed => anyhow::bail!("job is already completed"),
    }
}

/// The extract step: read the source PDF, structure it, and store the resulting
/// reviewable transcript. The PDF parsing is blocking and pdfium is not `Send`,
/// so it runs on a blocking thread (binding pdfium there) while the abort poll
/// stays responsive on the async runtime.
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
    /// parse needs one; the value is never connected to. Prefixes it before the
    /// given subcommand arguments.
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
        let cli = Cli::parse_from(argv(&["run"]));
        assert_eq!(cli.log_filter, "info");
    }

    #[test]
    fn log_filter_overridable_from_flag() {
        let cli = Cli::parse_from(argv(&["--log-filter", "debug", "run"]));
        assert_eq!(cli.log_filter, "debug");
    }

    #[test]
    fn run_subcommand_parses() {
        let cli = Cli::parse_from(argv(&["run"]));
        assert!(matches!(cli.command, Command::Run));
    }

    #[test]
    fn ingest_start_parses_its_arguments() {
        let id = "00000000-0000-0000-0000-000000000001";
        let user = "00000000-0000-0000-0000-0000000000ff";
        let cli = Cli::parse_from(argv(&[
            "ingest",
            "start",
            "--game-id",
            id,
            "--file",
            "rules.txt",
            "--language",
            "fr",
            "--created-by",
            user,
        ]));
        match cli.command {
            Command::Ingest {
                action:
                    IngestAction::Start {
                        game_id,
                        file,
                        language,
                        created_by,
                    },
                ..
            } => {
                assert_eq!(game_id, id.parse::<Uuid>().unwrap());
                assert_eq!(file, std::path::Path::new("rules.txt"));
                assert_eq!(language, LanguageCode::new("fr").unwrap());
                assert_eq!(created_by, Some(user.parse::<Uuid>().unwrap()));
            }
            other => panic!("expected ingest start, got {other:?}"),
        }
    }

    #[test]
    fn ingest_start_created_by_is_optional() {
        let cli = Cli::parse_from(argv(&[
            "ingest",
            "start",
            "--game-id",
            "00000000-0000-0000-0000-000000000001",
            "--file",
            "rules.txt",
            "--language",
            "en",
        ]));
        match cli.command {
            Command::Ingest {
                action: IngestAction::Start { created_by, .. },
                ..
            } => assert_eq!(created_by, None),
            other => panic!("expected ingest start, got {other:?}"),
        }
    }

    #[test]
    fn ingest_continue_parses_job_id() {
        let id = "00000000-0000-0000-0000-00000000000a";
        let cli = Cli::parse_from(argv(&["ingest", "continue", "--job-id", id]));
        match cli.command {
            Command::Ingest {
                action: IngestAction::Continue { job_id },
                ..
            } => assert_eq!(job_id, id.parse::<Uuid>().unwrap()),
            other => panic!("expected ingest continue, got {other:?}"),
        }
    }

    #[test]
    fn ingest_restart_parses_job_id() {
        let id = "00000000-0000-0000-0000-00000000000b";
        let cli = Cli::parse_from(argv(&["ingest", "restart", "--job-id", id]));
        match cli.command {
            Command::Ingest {
                action: IngestAction::Restart { job_id },
                ..
            } => assert_eq!(job_id, id.parse::<Uuid>().unwrap()),
            other => panic!("expected ingest restart, got {other:?}"),
        }
    }

    #[test]
    fn ingest_stop_parses_job_id() {
        let id = "00000000-0000-0000-0000-00000000000c";
        let cli = Cli::parse_from(argv(&["ingest", "stop", "--job-id", id]));
        match cli.command {
            Command::Ingest {
                action: IngestAction::Stop { job_id },
                ..
            } => assert_eq!(job_id, id.parse::<Uuid>().unwrap()),
            other => panic!("expected ingest stop, got {other:?}"),
        }
    }

    #[test]
    fn ingest_start_rejects_invalid_language() {
        let args = argv(&[
            "ingest",
            "start",
            "--game-id",
            "00000000-0000-0000-0000-000000000001",
            "--file",
            "rules.txt",
            "--language",
            "french",
        ]);
        assert!(Cli::try_parse_from(args).is_err());
    }

    #[test]
    fn ingest_start_requires_game_file_and_language() {
        assert!(Cli::try_parse_from(argv(&["ingest", "start"])).is_err());
    }

    #[test]
    fn ingest_continue_requires_job_id() {
        assert!(Cli::try_parse_from(argv(&["ingest", "continue"])).is_err());
    }

    #[test]
    fn ingest_restart_requires_job_id() {
        assert!(Cli::try_parse_from(argv(&["ingest", "restart"])).is_err());
    }

    #[test]
    fn ingest_stop_requires_job_id() {
        assert!(Cli::try_parse_from(argv(&["ingest", "stop"])).is_err());
    }

    #[test]
    fn ingest_requires_a_subcommand() {
        assert!(Cli::try_parse_from(argv(&[])).is_err());
    }
}
