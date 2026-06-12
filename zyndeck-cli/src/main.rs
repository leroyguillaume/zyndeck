//! Zyndeck command-line interface.
//!
//! The control surface for Zyndeck: it drives the system by writing directly to
//! the database (the services — like `zyndeck-ingester` — then act on what they
//! find there). This is the binary entry point; the compiled binary is named
//! `zyndeck`.

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;
use zyndeck_core::{IngestionStep, LanguageCode};
use zyndeck_db::{
    Db, DbConfig, IngestionJobRepository, IngestionStepRunRepository,
    IngestionTranscriptRepository, NewIngestionJob,
};

/// Command-line configuration. Resolution order is CLI flags → environment
/// variables → defaults; every option carries an `env` so nothing is
/// CLI-only.
#[derive(Debug, Parser)]
#[command(name = "zyndeck", version, about)]
struct Cli {
    #[command(flatten)]
    db: DbConfig,

    /// `tracing` filter directive (e.g. `info`, `zyndeck=debug`).
    #[arg(long = "log-filter", env = "RUST_LOG", default_value = "info")]
    log_filter: String,

    #[command(subcommand)]
    command: Command,
}

/// Top-level command groups.
#[derive(Debug, Subcommand)]
enum Command {
    /// Manage rule-ingestion jobs.
    Ingestion {
        #[command(subcommand)]
        action: IngestionAction,
    },
}

/// Operations on ingestion jobs.
#[derive(Debug, Subcommand)]
enum IngestionAction {
    /// Start a new ingestion job for a document.
    ///
    /// Writes the job to the database; the ingestion service picks it up, runs
    /// the transcription, then stops to await validation. The new job's id is
    /// printed to stdout — pass it to `ingestion edit` / `ingestion validate`
    /// once transcription is done.
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

    /// Edit a job's transcript in `$EDITOR`.
    ///
    /// Opens the transcript in `$EDITOR` (`$VISUAL`, then `$EDITOR`, falling back
    /// to `vi`) and saves any edits back. Errors if there is no transcript yet
    /// (transcription has not finished). This only edits — run `ingestion
    /// validate` when the transcript is good to continue the pipeline.
    Edit {
        /// Id of the job whose transcript to edit.
        #[arg(long = "job-id", env = "JOB_ID")]
        job_id: Uuid,
    },

    /// Validate a job's transcript and continue the pipeline (chunk + embed).
    ///
    /// Opens the human gate: the service picks the job back up and runs straight
    /// through chunking and embedding. There is no going back after this; use
    /// `ingestion restart` to redo the transcription instead.
    Validate {
        /// Id of the job whose transcript to validate.
        #[arg(long = "job-id", env = "JOB_ID")]
        job_id: Uuid,
    },

    /// Restart a job's transcription (re-run extraction).
    ///
    /// Only allowed before the transcript has been validated; once validated the
    /// job is locked into chunk + embed and cannot be restarted.
    Restart {
        /// Id of the job whose transcription to restart.
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

    // Connect and apply migrations at startup, so the CLI works against a fresh
    // database without a service having run first.
    let db = Db::connect(&cli.db).await?;
    db.migrate().await?;

    match cli.command {
        Command::Ingestion { action } => match action {
            IngestionAction::Start {
                game_id,
                file,
                language,
                created_by,
            } => ingestion_start(&db, game_id, file, language, created_by).await,
            IngestionAction::Edit { job_id } => ingestion_edit(&db, job_id).await,
            IngestionAction::Validate { job_id } => ingestion_validate(&db, job_id).await,
            IngestionAction::Restart { job_id } => ingestion_restart(&db, job_id).await,
        },
    }
}

/// Starts an ingestion job and prints its id.
async fn ingestion_start(
    db: &Db,
    game_id: Uuid,
    file: PathBuf,
    language: LanguageCode,
    created_by: Option<Uuid>,
) -> anyhow::Result<()> {
    let (job, _run) = db
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
        "ingestion job started",
    );

    // The id is the command's result: print it to stdout so it can be captured.
    println!("{}", job.id);
    Ok(())
}

/// Opens a job's transcript in the editor and saves any edits back. Errors right
/// away if the transcript does not exist yet (transcription is still running, or
/// it failed).
async fn ingestion_edit(db: &Db, job_id: Uuid) -> anyhow::Result<()> {
    let job = db
        .ingestion_jobs()
        .find_by_id(job_id)
        .await?
        .with_context(|| format!("no ingestion job with id {job_id}"))?;
    if job.step != IngestionStep::Extract {
        anyhow::bail!(
            "job {job_id} is past the transcription phase (step {}); its transcript is final",
            job.step.as_str(),
        );
    }

    let Some(original) = db.transcripts().find(job_id).await? else {
        return Err(no_transcript_error(db, job_id).await);
    };

    let edited = edit_in_editor(job_id, &original)?;
    if edited == original {
        tracing::info!(%job_id, "transcript unchanged");
    } else {
        db.transcripts().upsert(job_id, edited).await?;
        tracing::info!(%job_id, "transcript updated");
    }
    Ok(())
}

/// Validates a job's transcript, letting the service continue with chunk + embed.
async fn ingestion_validate(db: &Db, job_id: Uuid) -> anyhow::Result<()> {
    db.validate_job(job_id).await?;
    tracing::info!(%job_id, "transcript validated; chunking and embedding will follow");
    Ok(())
}

/// Restarts a job's transcription. The service re-runs extraction and stops
/// again to await validation.
async fn ingestion_restart(db: &Db, job_id: Uuid) -> anyhow::Result<()> {
    let run = db.restart_job(job_id).await?;
    tracing::info!(%job_id, attempt = run.attempt, "transcription restart requested");
    Ok(())
}

/// Builds the error for a job whose transcript is missing, tailoring the message
/// to the latest extract run: a failed run points at `ingestion restart`, an
/// in-flight one says it has not finished yet. A DB error while inspecting the
/// run is surfaced as-is.
async fn no_transcript_error(db: &Db, job_id: Uuid) -> anyhow::Error {
    match db.step_runs().find_latest(job_id).await {
        Ok(Some(run)) if !run.status.is_active() => match run.status.error() {
            Some(error) => anyhow::anyhow!(
                "transcription failed: {error}\n\
                 run `zyndeck ingestion restart --job-id {job_id}` to retry",
            ),
            None => anyhow::anyhow!(
                "job {job_id} has no transcript (transcription did not complete); \
                 run `zyndeck ingestion restart --job-id {job_id}` to retry",
            ),
        },
        Ok(_) => anyhow::anyhow!(
            "job {job_id} has no transcript yet — its transcription has not finished",
        ),
        Err(e) => e.into(),
    }
}

/// Round-trips `original` through a temp file the editor can open, returning the
/// edited content. A `.md` suffix gets the right syntax highlighting —
/// transcripts are Markdown. The temp file is cleaned up either way, so a failed
/// edit doesn't litter the temp dir.
fn edit_in_editor(job_id: Uuid, original: &str) -> anyhow::Result<String> {
    let path = std::env::temp_dir().join(format!("zyndeck-transcript-{job_id}.md"));
    std::fs::write(&path, original)
        .with_context(|| format!("writing transcript to {}", path.display()))?;

    let edited = launch_editor(&path).and_then(|()| {
        std::fs::read_to_string(&path)
            .with_context(|| format!("reading edited transcript from {}", path.display()))
    });
    let _ = std::fs::remove_file(&path);
    edited
}

/// Launches the user's editor (`$VISUAL`, then `$EDITOR`, falling back to `vi`)
/// on `path` and returns once it exits.
fn launch_editor(path: &std::path::Path) -> anyhow::Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    tracing::debug!(%editor, path = %path.display(), "opening transcript in editor");

    // Go through `sh -c` so an editor configured with arguments (e.g.
    // `code --wait`) works; the path is passed as `$1` so spaces in it survive.
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(path)
        .status()
        .with_context(|| format!("launching editor `{editor}`"))?;

    if !status.success() {
        anyhow::bail!("editor `{editor}` exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// `--database-url` is required (a top-level flattened `DbConfig`), so every
    /// parse needs one; the value is never connected to. Prefixes it before the
    /// given subcommand arguments.
    fn argv(rest: &[&'static str]) -> Vec<&'static str> {
        let mut args = vec!["zyndeck", "--database-url", "postgresql://test"];
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
        let cli = Cli::parse_from(argv(&[
            "ingestion",
            "start",
            "--game-id",
            "00000000-0000-0000-0000-000000000001",
            "--file",
            "rules.pdf",
            "--language",
            "en",
        ]));
        assert_eq!(cli.log_filter, "info");
    }

    #[test]
    fn ingestion_start_parses_its_arguments() {
        let id = "00000000-0000-0000-0000-000000000001";
        let user = "00000000-0000-0000-0000-0000000000ff";
        let cli = Cli::parse_from(argv(&[
            "ingestion",
            "start",
            "--game-id",
            id,
            "--file",
            "rules.pdf",
            "--language",
            "fr",
            "--created-by",
            user,
        ]));
        let Command::Ingestion {
            action:
                IngestionAction::Start {
                    game_id,
                    file,
                    language,
                    created_by,
                },
        } = cli.command
        else {
            panic!("expected an ingestion start command");
        };
        assert_eq!(game_id, id.parse::<Uuid>().unwrap());
        assert_eq!(file, std::path::Path::new("rules.pdf"));
        assert_eq!(language, LanguageCode::new("fr").unwrap());
        assert_eq!(created_by, Some(user.parse::<Uuid>().unwrap()));
    }

    #[test]
    fn ingestion_start_defaults_to_anonymous() {
        let cli = Cli::parse_from(argv(&[
            "ingestion",
            "start",
            "--game-id",
            "00000000-0000-0000-0000-000000000001",
            "--file",
            "rules.pdf",
            "--language",
            "en",
        ]));
        let Command::Ingestion {
            action: IngestionAction::Start { created_by, .. },
        } = cli.command
        else {
            panic!("expected an ingestion start command");
        };
        assert_eq!(created_by, None);
    }

    #[test]
    fn ingestion_start_requires_game_file_and_language() {
        assert!(Cli::try_parse_from(argv(&["ingestion", "start"])).is_err());
    }

    #[test]
    fn ingestion_edit_validate_restart_parse_their_job_id() {
        let id = "00000000-0000-0000-0000-000000000001";
        let want = id.parse::<Uuid>().unwrap();

        let cli = Cli::parse_from(argv(&["ingestion", "edit", "--job-id", id]));
        assert!(matches!(
            cli.command,
            Command::Ingestion {
                action: IngestionAction::Edit { job_id },
            } if job_id == want
        ));

        let cli = Cli::parse_from(argv(&["ingestion", "validate", "--job-id", id]));
        assert!(matches!(
            cli.command,
            Command::Ingestion {
                action: IngestionAction::Validate { job_id },
            } if job_id == want
        ));

        let cli = Cli::parse_from(argv(&["ingestion", "restart", "--job-id", id]));
        assert!(matches!(
            cli.command,
            Command::Ingestion {
                action: IngestionAction::Restart { job_id },
            } if job_id == want
        ));
    }

    #[test]
    fn ingestion_edit_validate_restart_require_a_job_id() {
        assert!(Cli::try_parse_from(argv(&["ingestion", "edit"])).is_err());
        assert!(Cli::try_parse_from(argv(&["ingestion", "validate"])).is_err());
        assert!(Cli::try_parse_from(argv(&["ingestion", "restart"])).is_err());
    }

    #[test]
    fn ingestion_requires_a_subcommand() {
        assert!(Cli::try_parse_from(argv(&["ingestion"])).is_err());
    }
}
