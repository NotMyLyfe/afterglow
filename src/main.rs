mod backend;
mod classify;
mod lsn;
mod metrics;
mod proxy;
mod router;
mod session;

use anyhow::Result;
use backend::BackendPool;

#[derive(Debug, Clone)]
pub struct Config {
    pub primary_url: String,
    pub replica_url: String,
    pub listen_addr: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    tracing::info!("Starting Afterglow...");

    let config = Config::from_env()?;
    tracing::debug!(?config, "Loaded configuration: {:?}", config);
    let pool = BackendPool::new(&config)?;
    tracing::info!("Initialized backend connection pools");

    proxy::run(config, pool).await
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Self {
            listen_addr: std::env::var("LISTEN_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:6432".to_string()),
            primary_url: std::env::var("PRIMARY_URL")
                .unwrap_or_else(|_| "postgres://localhost:5432/postgres".to_string()),
            replica_url: std::env::var("REPLICA_URL")
                .unwrap_or_else(|_| "postgres://localhost:5432/postgres".to_string()),
        })
    }
}
