//! Zyndeck ingestion service.
//!
//! Ingests game rules so the rest of Zyndeck can validate decks against them
//! and let the LLM answer questions about them. This is the binary entry
//! point: it wires up configuration, tracing, and graceful shutdown, then runs
//! the ingestion loop.

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// Command-line configuration. Resolution order is CLI flags → environment
/// variables → defaults; every option carries an `env` so nothing is
/// CLI-only.
#[derive(Debug, Parser)]
#[command(name = "zyndeck-ingester", version, about)]
struct Cli {
    /// `tracing` filter directive (e.g. `info`, `zyndeck_ingester=debug`).
    #[arg(long = "log-filter", env = "RUST_LOG", default_value = "info")]
    log_filter: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&cli.log_filter))
        .init();

    tracing::info!(log_filter = %cli.log_filter, "starting zyndeck-ingester");

    // No ingestion work yet — sit idle until asked to stop so the binary
    // already behaves like a well-mannered long-running service.
    shutdown_signal().await;

    tracing::info!("shutdown signal received, exiting");
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

    #[test]
    fn cli_definition_is_valid() {
        // Catches malformed `clap` derive wiring (duplicate args, bad defaults).
        Cli::command().debug_assert();
    }

    #[test]
    fn log_filter_defaults_to_info() {
        let cli = Cli::parse_from(["zyndeck-ingester"]);
        assert_eq!(cli.log_filter, "info");
    }

    #[test]
    fn log_filter_overridable_from_flag() {
        let cli = Cli::parse_from(["zyndeck-ingester", "--log-filter", "debug"]);
        assert_eq!(cli.log_filter, "debug");
    }
}
