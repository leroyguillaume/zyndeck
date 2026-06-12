//! Administrative tooling for the Zyndeck database.
//!
//! A thin operator binary that sits alongside the `zyndeck-db` library. Its one
//! job today is `recreate`: drop the target database, create it fresh, and apply
//! the embedded migrations — handy for resetting a local or CI database to a
//! clean, fully-migrated state. It writes to the maintenance database to do the
//! drop/create, so it must be pointed at a URL whose role may create databases.

use std::io::{self, Write};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use sqlx::Postgres;
use sqlx::migrate::MigrateDatabase;
use tracing_subscriber::EnvFilter;
use zyndeck_db::{Db, DbConfig};

/// Command-line configuration. Resolution order is CLI flags → environment
/// variables → defaults; every option carries an `env` so nothing is CLI-only.
#[derive(Debug, Parser)]
#[command(name = "zyndeck-db-tool", version, about)]
struct Cli {
    #[command(flatten)]
    db: DbConfig,

    /// `tracing` filter directive (e.g. `info`, `zyndeck=debug`).
    #[arg(long = "log-filter", env = "RUST_LOG", default_value = "info")]
    log_filter: String,

    #[command(subcommand)]
    command: Command,
}

/// Database maintenance commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Drop the database and recreate it from the migrations.
    ///
    /// Destroys everything in the target database, creates it anew, and applies
    /// every migration. This is irreversible, so it asks for confirmation unless
    /// `--yes` is given.
    Recreate {
        /// Skip the confirmation prompt (for scripts and CI).
        #[arg(long = "yes", short = 'y', env = "ASSUME_YES")]
        yes: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&cli.log_filter))
        .init();

    match cli.command {
        Command::Recreate { yes } => recreate(&cli.db, yes).await,
    }
}

/// Drops the database named in `config.url`, recreates it, and migrates it.
async fn recreate(config: &DbConfig, yes: bool) -> anyhow::Result<()> {
    if !yes && !confirm(&config.url)? {
        tracing::info!("aborted; database left untouched");
        return Ok(());
    }

    if Postgres::database_exists(&config.url)
        .await
        .context("failed to check whether the database exists")?
    {
        tracing::warn!("dropping existing database");
        Postgres::drop_database(&config.url)
            .await
            .context("failed to drop the database")?;
    }

    tracing::info!("creating database");
    Postgres::create_database(&config.url)
        .await
        .context("failed to create the database")?;

    let db = Db::connect(config).await?;
    tracing::info!("applying migrations");
    db.migrate().await?;

    tracing::info!("database recreated");
    Ok(())
}

/// Asks the operator to confirm the destructive recreate on the terminal.
///
/// Returns `Ok(true)` only when the answer is an explicit `y`/`yes`. The prompt
/// goes to stdout (not `tracing`) because it is interactive UI, not a diagnostic.
fn confirm(url: &str) -> anyhow::Result<bool> {
    print!(
        "This will DROP and recreate the database at {}.\nAll data will be lost. Continue? [y/N] ",
        redacted(url)
    );
    io::stdout().flush().context("failed to write the prompt")?;

    let mut answer = String::new();
    let read = io::stdin()
        .read_line(&mut answer)
        .context("failed to read the confirmation")?;
    if read == 0 {
        // EOF with no input (e.g. non-interactive stdin) — treat as "no".
        bail!("no confirmation provided; re-run with --yes to skip the prompt");
    }

    let answer = answer.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// Hides the password in a connection URL before showing it to the operator, so
/// the prompt does not leak the credential onto the terminal or into logs.
fn redacted(url: &str) -> String {
    match (url.find("://"), url.find('@')) {
        (Some(scheme_end), Some(at)) if scheme_end + 3 < at => {
            let creds = &url[scheme_end + 3..at];
            let user = creds.split(':').next().unwrap_or(creds);
            format!("{}://{}:***@{}", &url[..scheme_end], user, &url[at + 1..])
        }
        _ => url.to_string(),
    }
}
