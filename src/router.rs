use crate::backend::BackendPool;
use crate::classify::{QueryKind, TxAction, classify};
use crate::lsn::{self, Lsn};
use crate::session::SessionState;

use anyhow::Result;
use deadpool_postgres::Client;
use tokio_postgres::SimpleQueryMessage;

use std::time::{Duration, Instant};
use std::vec::Vec;

const REPLICA_WAIT_TIMEOUT: Duration = Duration::from_millis(50);
const POLL_INTERVAL: Duration = Duration::from_millis(2);

async fn execute_write(
    query: &str,
    session: &mut SessionState,
    pool: &BackendPool,
) -> Result<Vec<SimpleQueryMessage>> {
    let connection = pool.primary().await?;

    let query_rows = connection.simple_query(query).await?;

    let lsn_row = connection
        .query_one("SELECT pg_current_wal_lsn()::text", &[])
        .await?;

    let lsn_str: String = lsn_row.get(0);
    let lsn = Lsn::parse(&lsn_str)?;

    session.record_write(lsn);

    Ok(query_rows)
}

pub async fn handle(
    query: &str,
    session: &mut SessionState,
    pool: &BackendPool,
) -> Result<Vec<SimpleQueryMessage>> {
    let kind = classify(query);

    if session.in_transaction() {
        // Query may be a write, but assumption is that it's all in primary regardless
        if kind == QueryKind::Transaction(TxAction::Commit) {
            let rows = execute_write(query, session, pool).await?;
            session.exit_transaction();
            return Ok(rows);
        }

        let primary_connection = pool.primary().await?;

        let query_rows = primary_connection.simple_query(query).await?;

        if kind == QueryKind::Transaction(TxAction::Rollback) {
            session.exit_transaction();
        }

        Ok(query_rows)
    } else {
        match kind {
            QueryKind::Write => execute_write(query, session, pool).await,
            QueryKind::Read => route_read(query, session, pool).await,
            QueryKind::Transaction(tx) => {
                let primary_connection = pool.primary().await?;

                match tx {
                    TxAction::Begin => {
                        let query_rows = primary_connection.simple_query(query).await?;
                        session.enter_transaction();
                        Ok(query_rows)
                    }
                    // Session not in transaction - so everything just runs through for now
                    _ => Ok(primary_connection.simple_query(query).await?),
                }
            }
            _ => Ok(pool.primary().await?.simple_query(query).await?),
        }
    }
}

async fn route_read(
    query: &str,
    session: &SessionState,
    pool: &BackendPool,
) -> Result<Vec<SimpleQueryMessage>> {
    let replica = pool.replica().await?;
    match session.last_write_lsn() {
        None => Ok(replica.simple_query(query).await?),
        Some(lsn) => match wait_for_replica(&replica, lsn, REPLICA_WAIT_TIMEOUT).await {
            Ok(true) => {
                tracing::debug!("served read from replica");
                Ok(replica.simple_query(query).await?)
            }
            Ok(false) => {
                tracing::debug!("replica lagging, falling back to primary");
                let primary = pool.primary().await?;
                Ok(primary.simple_query(query).await?)
            }
            Err(e) => {
                tracing::warn!(error = %e, "replica poll error, falling back to primary");
                let primary = pool.primary().await?;
                Ok(primary.simple_query(query).await?)
            }
        },
    }
}

async fn wait_for_replica(client: &Client, target: Lsn, timeout: Duration) -> Result<bool> {
    let start_time = Instant::now();
    loop {
        let row = client
            .query_one("SELECT pg_last_wal_replay_lsn()::text", &[])
            .await?;
        let current_str: String = row.get(0);
        let current = Lsn::parse(&current_str)?;

        if current >= target {
            return Ok(true);
        }
        if start_time.elapsed() >= timeout {
            return Ok(false);
        }
        // POLL_INTERVAL can go beyond timeout - TODO: fix later
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
