use clap::Args;

/// Database configuration.
///
/// Exposed as a `clap` argument group so binaries can flatten it into their own
/// CLI with `#[command(flatten)]`. Every field carries an `env` attribute, per
/// the workspace's CLI-flags → environment-variables → defaults rule.
#[derive(Debug, Clone, Args)]
pub struct DbConfig {
    /// PostgreSQL connection URL (e.g. `postgresql://user:pass@host:5432/db`).
    #[arg(long = "database-url", env = "DATABASE_URL")]
    pub url: String,

    /// Maximum number of connections kept in the pool.
    #[arg(
        long = "db-max-connections",
        env = "DB_MAX_CONNECTIONS",
        default_value_t = 10
    )]
    pub max_connections: u32,
}
