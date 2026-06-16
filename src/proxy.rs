use crate::BackendPool;
use crate::Config;
use crate::router;
use crate::session::SessionState;

use anyhow::Result;

use futures::stream;

use pgwire::api::query::SimpleQueryHandler;
use pgwire::api::results::{DataRowEncoder, FieldFormat, FieldInfo, QueryResponse, Response};
use pgwire::api::{ClientInfo, PgWireServerHandlers, Type};
use pgwire::error::ErrorInfo;
use pgwire::error::PgWireError;
use pgwire::error::PgWireResult;
use pgwire::tokio::process_socket;

use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio_postgres::SimpleQueryMessage;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

const SESSION_ID_KEY: &str = "session_id";

pub async fn run(config: Config, pool: BackendPool) -> Result<()> {
    let listener: TcpListener = TcpListener::bind(&config.listen_addr).await?;
    tracing::info!(
        addr = %config.listen_addr,
        "Proxy listening on {}",
        config.listen_addr
    );

    let pool = Arc::new(pool);
    let handler = Arc::new(Handler {
        query_handler: Arc::new(QueryHandler {
            pool: pool.clone(),
            session: Mutex::new(HashMap::new()),
            id_counter: AtomicU32::new(0),
        }),
    });

    loop {
        let (socket, addr) = listener.accept().await?;
        tracing::debug!(?addr, "Accepted connection from {}", addr);

        let handler = handler.clone();
        tokio::spawn(async move {
            if let Err(e) = process_socket(socket, None, handler).await {
                tracing::error!(error = %e, "Error processing connection from {}", addr);
            }
        });
    }
}

struct Handler {
    query_handler: Arc<QueryHandler>,
}

impl PgWireServerHandlers for Handler {
    fn simple_query_handler(&self) -> Arc<impl SimpleQueryHandler> {
        Arc::new(QueryHandler {
            pool: self.query_handler.pool.clone(),
            session: Mutex::new(HashMap::new()),
            id_counter: AtomicU32::new(0),
        })
    }
}

struct QueryHandler {
    pool: Arc<BackendPool>,
    session: Mutex<HashMap<String, SessionState>>,
    id_counter: AtomicU32,
}

fn to_pg_wire_error(e: anyhow::Error) -> PgWireError {
    let code = "XX000"; // Internal Error
    let message = e.to_string();
    let error_info = ErrorInfo::new("ERROR".to_string(), code.to_string(), message);
    PgWireError::UserError(Box::new(error_info))
}

#[async_trait::async_trait]
impl SimpleQueryHandler for QueryHandler {
    async fn do_query<C>(&self, _client: &mut C, query: &str) -> PgWireResult<Vec<Response>>
    where
        C: ClientInfo + Unpin + Send + Sync,
    {
        let session_id = _client
            .metadata_mut()
            .entry(SESSION_ID_KEY.to_string())
            .or_insert_with(|| {
                let id = self
                    .id_counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                format!("session-{}", id)
            });

        let result = {
            let mut session = self.session.lock().await;
            let session_state = session
                .entry(session_id.clone())
                .or_insert_with(SessionState::default);

            router::handle(query, session_state, self.pool.as_ref())
                .await
                .map_err(to_pg_wire_error)?
        };

        let mut responses = Vec::new();
        let mut fields = None;
        let mut row_encoder = None;
        let mut data_rows = Vec::new();

        for message in result {
            match message {
                SimpleQueryMessage::CommandComplete(_num) => {
                    if fields.is_some() {
                        // If we have seen Row messages, we need to finalize the QueryResponse with the collected fields and data rows
                        let fields_arc = fields.take().unwrap();
                        let row_stream = stream::iter(data_rows.into_iter());
                        responses.push(Response::Query(QueryResponse::new(fields_arc, row_stream)));

                        // Reset the row encoder and data rows for the next command
                        fields = None;
                        row_encoder = None;
                        data_rows = Vec::new();
                    } else {
                        // If we haven't seen any Row messages yet, we can just return a CommandComplete response without fields
                        // TODO: We might want to return a more specific tag based on the command type (e.g., "INSERT 0 1" for an insert that affected 1 row)
                        responses.push(Response::Execution(pgwire::api::results::Tag::new("OK")));
                    }
                }
                SimpleQueryMessage::Row(row) => {
                    if fields.is_none() {
                        // If we haven't seen a Row message before, we need to initialize the fields and row encoder
                        fields = Some(Arc::new(
                            row.columns()
                                .iter()
                                .map(|col| {
                                    FieldInfo::new(
                                        col.name().to_string(),
                                        None,
                                        None,
                                        Type::TEXT, // For simplicity, we treat all columns as TEXT
                                        FieldFormat::Text,
                                    )
                                })
                                .collect(),
                        ));
                        row_encoder = Some(DataRowEncoder::new(fields.clone().unwrap()));
                    }

                    if let Some(encoder) = &mut row_encoder {
                        // Iterate through all the columns in the current row and encode them using the DataRowEncoder
                        for col in row.columns() {
                            let value: Option<&str> = row.get(col.name());
                            encoder.encode_field(&value)?;
                        }
                        data_rows.push(Ok(encoder.take_row()));
                    }
                }
                _ => {
                    // Handle other message types if needed
                }
            }
        }
        Ok(responses)
    }
}
