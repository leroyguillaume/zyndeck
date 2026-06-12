//! Zyndeck command-line interface.
//!
//! The control surface for Zyndeck: it drives the system by writing directly to
//! the database (the services — like `zyndeck-ingester` — then act on what they
//! find there). This is the binary entry point; the compiled binary is named
//! `zyndeck`.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;
use zyndeck_core::{IngestionMode, LanguageCode};
use zyndeck_db::{Db, DbConfig, IngestionJobRepository, NewIngestionJob};

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
    /// Writes the job to the database; the ingestion service picks it up from
    /// there. The new job's id is printed to stdout.
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

        /// How the job advances between steps: `auto` runs straight through
        /// until the job completes or a step fails; `manual` stops after each
        /// step so its output can be reviewed and corrected.
        #[arg(long = "mode", env = "INGESTION_MODE", default_value_t = IngestionMode::Auto)]
        mode: IngestionMode,

        /// Id of the user starting the job, if any (CLI runs may be anonymous).
        #[arg(long = "created-by", env = "CREATED_BY")]
        created_by: Option<Uuid>,
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
                mode,
                created_by,
            } => ingestion_start(&db, game_id, file, language, mode, created_by).await,
        },
    }
}

/// Starts an ingestion job and prints its id.
async fn ingestion_start(
    db: &Db,
    game_id: Uuid,
    file: PathBuf,
    language: LanguageCode,
    mode: IngestionMode,
    created_by: Option<Uuid>,
) -> anyhow::Result<()> {
    let job = db
        .ingestion_jobs()
        .create(NewIngestionJob {
            game_id,
            source: file,
            language,
            mode,
            created_by,
        })
        .await?;

    tracing::info!(
        job_id = %job.id,
        step = job.step.as_str(),
        mode = job.mode.as_str(),
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
            "--mode",
            "manual",
            "--created-by",
            user,
        ]));
        let Command::Ingestion {
            action:
                IngestionAction::Start {
                    game_id,
                    file,
                    language,
                    mode,
                    created_by,
                },
        } = cli.command;
        assert_eq!(game_id, id.parse::<Uuid>().unwrap());
        assert_eq!(file, std::path::Path::new("rules.pdf"));
        assert_eq!(language, LanguageCode::new("fr").unwrap());
        assert_eq!(mode, IngestionMode::Manual);
        assert_eq!(created_by, Some(user.parse::<Uuid>().unwrap()));
    }

    #[test]
    fn ingestion_start_defaults_to_auto_mode_and_anonymous() {
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
            action: IngestionAction::Start {
                mode, created_by, ..
            },
        } = cli.command;
        assert_eq!(mode, IngestionMode::Auto);
        assert_eq!(created_by, None);
    }

    #[test]
    fn ingestion_start_rejects_invalid_mode() {
        let args = argv(&[
            "ingestion",
            "start",
            "--game-id",
            "00000000-0000-0000-0000-000000000001",
            "--file",
            "rules.pdf",
            "--language",
            "en",
            "--mode",
            "semi",
        ]);
        assert!(Cli::try_parse_from(args).is_err());
    }

    #[test]
    fn ingestion_start_requires_game_file_and_language() {
        assert!(Cli::try_parse_from(argv(&["ingestion", "start"])).is_err());
    }

    #[test]
    fn ingestion_requires_a_subcommand() {
        assert!(Cli::try_parse_from(argv(&["ingestion"])).is_err());
    }
}
