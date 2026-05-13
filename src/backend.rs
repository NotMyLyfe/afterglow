use std::str::FromStr;

use anyhow::Result;
use deadpool_postgres::{Client, Manager, ManagerConfig, Pool, RecyclingMethod};
use tokio_postgres::NoTls;

use crate::Config;

pub struct BackendPool {
    primary: Pool,
    replica: Pool,
}

impl BackendPool {
    fn make_pool(url: &str) -> Result<Pool> {
        let pg_config = tokio_postgres::Config::from_str(url)?;
        let mgr_config = ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        };
        let mgr = Manager::from_config(pg_config, NoTls, mgr_config);
        let pool = Pool::builder(mgr).max_size(16).build()?;
        Ok(pool)
    }

    pub fn new(config: &Config) -> Result<Self> {
        let primary = Self::make_pool(&config.primary_url)?;
        let replica = Self::make_pool(&config.replica_url)?;

        Ok(Self { primary, replica })
    }

    pub async fn primary(&self) -> Result<Client> {
        self.primary.get().await.map_err(Into::into)
    }

    pub async fn replica(&self) -> Result<Client> {
        self.replica.get().await.map_err(Into::into)
    }
}
