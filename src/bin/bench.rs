use anyhow::Result;
use tokio_postgres::NoTls;
use uuid::Uuid;

const ITERATIONS: usize = 1000;

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

    // Scenario 1: Write to primary, read from primary
    let primary_stale_reads =
        run_workload(&primary_client, &primary_client, &bench_table, ITERATIONS).await?;
    println!(
        "Scenario 1: Write to primary, read from primary - Stale reads: {primary_stale_reads} / {ITERATIONS}"
    );

    // Scenario 2: Write to primary, read from replica
    let replica_stale_reads =
        run_workload(&primary_client, &replica_client, &bench_table, ITERATIONS).await?;
    println!(
        "Scenario 2: Write to primary, read from replica - Stale reads: {replica_stale_reads} / {ITERATIONS}"
    );

    // Scenario 3: Write to proxy, read from proxy
    let proxy_stale_reads =
        run_workload(&proxy_client, &proxy_client, &bench_table, ITERATIONS).await?;
    println!(
        "Scenario 3: Write to proxy, read from proxy - Stale reads: {proxy_stale_reads} / {ITERATIONS}"
    );

    bench_table.drop(&primary_client).await?;

    Ok(())
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
