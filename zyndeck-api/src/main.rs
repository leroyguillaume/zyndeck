//! Zyndeck API server entry point.

use clap::Parser;
use tracing_subscriber::EnvFilter;
use zyndeck_api::{AppState, build_router, hash_password};
use zyndeck_core::Role;
use zyndeck_db::{Db, DbConfig, NewUser, UserRepository};

/// Command-line configuration. Resolution order is CLI flags → environment
/// variables → defaults; every option carries an `env`.
#[derive(Debug, Parser)]
#[command(name = "zyndeck-api", version, about)]
struct Cli {
    #[command(flatten)]
    db: DbConfig,

    /// Address the HTTP server binds to.
    #[arg(long, env = "BIND_ADDR", default_value = "0.0.0.0:8080")]
    bind_addr: String,

    /// Secret used to sign and verify HS256 JWTs.
    #[arg(long, env = "JWT_SECRET")]
    jwt_secret: String,

    /// Lifetime, in seconds, of tokens issued by the login endpoint.
    #[arg(long = "jwt-ttl", env = "JWT_TTL_SECONDS", default_value_t = 86_400)]
    jwt_ttl_seconds: u64,

    /// Username of the bootstrap super-admin created/updated at startup.
    #[arg(long, env = "ADMIN_USERNAME", default_value = "admin")]
    admin_username: String,

    /// Password of the bootstrap super-admin created/updated at startup.
    #[arg(long, env = "ADMIN_PASSWORD")]
    admin_password: String,

    /// `tracing` filter directive (e.g. `info`, `zyndeck_api=debug`).
    #[arg(long = "log-filter", env = "RUST_LOG", default_value = "info")]
    log_filter: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&cli.log_filter))
        .init();

    let db = Db::connect(&cli.db).await?;
    db.migrate().await?;

    // Bootstrap (or reset) the super-admin from the params.
    let password_hash = hash_password(&cli.admin_password)?;
    let admin = db
        .users()
        .upsert_by_username(NewUser {
            username: cli.admin_username,
            password_hash,
            role: Role::SuperAdmin,
        })
        .await?;
    tracing::info!(username = %admin.username, "bootstrap super-admin ensured");

    let state = AppState::new(db, &cli.jwt_secret, cli.jwt_ttl_seconds);
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&cli.bind_addr).await?;
    tracing::info!(bind_addr = %cli.bind_addr, "zyndeck-api listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("shutdown signal received, exiting");
    Ok(())
}

/// Completes on `SIGTERM` or `SIGINT` for a clean drain.
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");

    tokio::select! {
        _ = interrupt.recv() => tracing::debug!("received SIGINT"),
        _ = terminate.recv() => tracing::debug!("received SIGTERM"),
    }
}
