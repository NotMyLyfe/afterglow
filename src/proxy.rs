use crate::Config;

use anyhow::Result;

use futures::stream;

use pgwire::api::query::SimpleQueryHandler;
use pgwire::api::results::{DataRowEncoder, FieldFormat, FieldInfo, QueryResponse, Response};
use pgwire::api::{ClientInfo, PgWireServerHandlers};
use pgwire::error::PgWireResult;

use tokio::net::TcpListener;
use tokio_postgres::types::Type;

use std::sync::Arc;

pub async fn run(config: Config) -> Result<()> {
    let listener: TcpListener = TcpListener::bind(&config.listen_addr).await?;
    tracing::info!(
        addr = %config.listen_addr,
        "Proxy listening on {}",
        config.listen_addr
    );

    loop {
        let (socket, addr) = listener.accept().await?;
        tracing::debug!(?addr, "Accepted connection from {}", addr);
    }
}

struct Handler {
    query_handler: Arc<EchoQueryHandler>,
}

impl PgWireServerHandlers for Handler {
    fn simple_query_handler(&self) -> Arc<impl SimpleQueryHandler> {
        self.query_handler.clone()
    }
}

struct EchoQueryHandler;

#[async_trait::async_trait]
impl SimpleQueryHandler for EchoQueryHandler {
    async fn do_query<C>(&self, _client: &mut C, query: &str) -> PgWireResult<Vec<Response>>
    where
        C: ClientInfo + Unpin + Send + Sync,
    {
        tracing::info!(query = %query, "Received query: {}", query);

        let fields = Arc::new(vec![FieldInfo::new(
            "echo".to_string(), // Column name
            None,               // Table OID (None for no specific table)
            None,               // Column index (None for no specific column)
            Type::TEXT,         // Data type (TEXT in this case)
            FieldFormat::Text,  // Format (Text or Binary)
        )]);

        let mut encoder = DataRowEncoder::new(fields.clone());
        encoder.encode_field(&query)?;
        let row = encoder.take_row();

        let row_stream = stream::iter(vec![Ok(row)]);

        Ok(vec![Response::Query(QueryResponse::new(
            fields, row_stream,
        ))])
    }
}
