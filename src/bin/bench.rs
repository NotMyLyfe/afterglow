use anyhow::Result;
use tokio_postgres::NoTls;
use uuid::Uuid;

use std::collections::HashMap;

const ITERATIONS: usize = 10000;
const LATENCIES: [u64; 10] = [0, 1, 5, 10, 25, 50, 100, 200, 500, 1000];
const METRICS_URL: &str = "http://localhost:9090/metrics";

struct BenchTable {
    name: String,
}

impl BenchTable {
    // Creates a new table in the database for benchmarking purposes
    // Waits for replica to catch up before returning
    async fn create(
        primary: &tokio_postgres::Client,
        replica: &tokio_postgres::Client,
    ) -> Result<Self> {
        let table_id = Uuid::now_v7();
        let table_name = format!("bench_{table_id}").replace('-', "_");

        let create_table_query =
            format!("CREATE TABLE IF NOT EXISTS {table_name} (id UUID PRIMARY KEY, marker TEXT)");
        primary.simple_query(&create_table_query).await?;

        // Wait for replica to catch up
        for _ in 0..100 {
            let check_query = format!(
                "SELECT 1 FROM information_schema.tables WHERE table_name = '{table_name}'"
            );
            let rows = replica.simple_query(&check_query).await?;
            let exists = rows
                .iter()
                .any(|msg| matches!(msg, tokio_postgres::SimpleQueryMessage::Row(_)));
            if exists {
                return Ok(Self { name: table_name });
            }

            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        anyhow::bail!("Replica did not catch up in time for table creation");
    }

    async fn drop(self, primary: &tokio_postgres::Client) -> Result<()> {
        let drop_table_query = format!("DROP TABLE IF EXISTS {}", self.name);
        primary.simple_query(&drop_table_query).await?;
        Ok(())
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let primary_url =
        std::env::var("PRIMARY_URL").unwrap_or("postgres://localhost:5432/postgres".to_string());
    let replica_url =
        std::env::var("REPLICA_URL").unwrap_or("postgres://localhost:5433/postgres".to_string());
    let proxy_url =
        std::env::var("PROXY_URL").unwrap_or("postgres://localhost:6432/postgres".to_string());

    let (primary_client, primary_connection) = tokio_postgres::connect(&primary_url, NoTls).await?;
    let (replica_client, replica_connection) = tokio_postgres::connect(&replica_url, NoTls).await?;
    let (proxy_client, proxy_connection) = tokio_postgres::connect(&proxy_url, NoTls).await?;

    tokio::spawn(async move {
        if let Err(e) = primary_connection.await {
            eprintln!("Primary connection error: {e}");
        }
    });

    tokio::spawn(async move {
        if let Err(e) = replica_connection.await {
            eprintln!("Replica connection error: {e}");
        }
    });

    tokio::spawn(async move {
        if let Err(e) = proxy_connection.await {
            eprintln!("Proxy connection error: {e}");
        }
    });

    let bench_table = BenchTable::create(&primary_client, &replica_client).await?;

    for &latency in &LATENCIES {
        println!("Running benchmark with latency: {latency} ms");
        let (results, lookup_counts) = run_latency_workload(
            &primary_client,
            &replica_client,
            &proxy_client,
            &bench_table,
            latency,
            ITERATIONS,
        )
        .await?;

        println!("Results for latency {latency} ms:");
        for result in results {
            println!(
                "Write Route: {}, Read Route: {}, Stale Reads: {}",
                result.write_route, result.read_route, result.stale_reads
            );
        }

        println!("Lookup counts for latency {latency} ms:");
        for (route, count) in lookup_counts {
            println!("Route: {route}, Count: {count}");
        }
    }

    bench_table.drop(&primary_client).await?;

    Ok(())
}

async fn get_current_latency(client: &tokio_postgres::Client) -> Result<String> {
    let rows = client.simple_query("SHOW recovery_min_apply_delay").await?;

    rows.iter()
        .find_map(|msg| match msg {
            tokio_postgres::SimpleQueryMessage::Row(row) => row
                .get("recovery_min_apply_delay")
                .map(std::string::ToString::to_string),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("Failed to retrieve current latency"))
}

async fn get_lookup_counts(metrics_url: &str) -> Result<HashMap<String, usize>> {
    let body = reqwest::get(metrics_url).await?.text().await?;

    let mut counts = HashMap::new();
    for line in body.lines() {
        if !line.starts_with("afterglow_reads_total{route=\"") {
            continue;
        }

        if let Some((route, count_str)) = line
            .strip_prefix("afterglow_reads_total{route=\"")
            .and_then(|s| s.split_once("\"} "))
            && let Ok(count) = count_str.parse::<usize>()
        {
            counts.insert(route.to_string(), count);
        }
    }
    Ok(counts)
}

struct LatencyWorkloadResult {
    write_route: String,
    read_route: String,
    stale_reads: usize,
}

async fn run_latency_workload(
    primary_client: &tokio_postgres::Client,
    replica_client: &tokio_postgres::Client,
    proxy_client: &tokio_postgres::Client,
    bench_table: &BenchTable,
    latency: u64,
    iterations: usize,
) -> Result<(Vec<LatencyWorkloadResult>, HashMap<String, usize>)> {
    struct WorkloadOrder<'a> {
        write_name: &'a str,
        read_name: &'a str,
        write_client: &'a tokio_postgres::Client,
        read_client: &'a tokio_postgres::Client,
    }

    let workload_order = [
        WorkloadOrder {
            write_name: "primary",
            read_name: "primary",
            write_client: primary_client,
            read_client: primary_client,
        },
        WorkloadOrder {
            write_name: "primary",
            read_name: "replica",
            write_client: primary_client,
            read_client: replica_client,
        },
        WorkloadOrder {
            write_name: "proxy",
            read_name: "proxy",
            write_client: proxy_client,
            read_client: proxy_client,
        },
    ];

    // Remember the original latency
    let original_latency = get_current_latency(replica_client).await?;

    // Set the desired latency
    let latency_str = latency.to_string();
    replica_client
        .simple_query(&format!(
            "ALTER SYSTEM SET recovery_min_apply_delay = '{latency_str}'"
        ))
        .await?;
    replica_client
        .simple_query("SELECT pg_reload_conf()")
        .await?;

    // Get current lookup counts before running the workload
    let prev_lookup_counts = get_lookup_counts(METRICS_URL).await?;

    // Run the workload for each combination of write and read clients
    let results = futures::future::join_all(workload_order.iter().map(|order| async move {
        let stale_reads = run_workload(
            order.write_client,
            order.read_client,
            bench_table,
            iterations,
        )
        .await?;

        Ok::<LatencyWorkloadResult, anyhow::Error>(LatencyWorkloadResult {
            write_route: order.write_name.to_string(),
            read_route: order.read_name.to_string(),
            stale_reads,
        })
    }))
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()?;

    let current_lookup_counts = get_lookup_counts(METRICS_URL).await?;

    // Guaranteed to be monotonically increasing, so we can safely subtract the previous counts from the current counts to get the difference
    let diff_lookup_counts: HashMap<String, usize> = current_lookup_counts
        .iter()
        .map(|(route, &current_count)| {
            let prev_count = prev_lookup_counts.get(route).copied().unwrap_or(0);
            (route.clone(), current_count.saturating_sub(prev_count))
        })
        .collect();

    // Restore the original latency
    replica_client
        .simple_query(&format!(
            "ALTER SYSTEM SET recovery_min_apply_delay = '{original_latency}'"
        ))
        .await?;
    replica_client
        .simple_query("SELECT pg_reload_conf()")
        .await?;

    Ok((results, diff_lookup_counts))
}

async fn run_workload(
    write_client: &tokio_postgres::Client,
    read_client: &tokio_postgres::Client,
    table: &BenchTable,
    n: usize,
) -> Result<usize> {
    // Loop through n iters, and keep track of stale reads
    let mut stale_reads = 0;

    for _ in 0..n {
        let id = Uuid::now_v7();
        let marker = format!("bench-{id}");

        let insert_query = format!(
            "INSERT INTO {} (id, marker) VALUES ('{id}', '{marker}')",
            table.name()
        );
        write_client.simple_query(&insert_query).await?;

        let select_query = format!("SELECT marker FROM {} WHERE id = '{id}'", table.name());
        let rows = read_client.simple_query(&select_query).await?;

        let found = rows.iter().find_map(|msg| match msg {
            tokio_postgres::SimpleQueryMessage::Row(row) => {
                row.get("marker").map(std::string::ToString::to_string)
            }
            _ => None,
        });

        let is_stale = found.as_deref() != Some(marker.as_str());

        if is_stale {
            stale_reads += 1;
        }
    }

    Ok(stale_reads)
}
