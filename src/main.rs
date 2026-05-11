use anyhow::Result;

#[derive(Debug, Clone)]
pub struct Config {
    pub primary_url: String,
    pub replica_url: String,
    pub listen_addr: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    Ok(())
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
