use crate::BackendPool;
use crate::Config;
use crate::lsn::Lsn;
use crate::router;
use crate::session::SessionState;

use anyhow::Result;

use base64::Engine;
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

use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::AtomicU32;

const SESSION_ID_KEY: &str = "session_id";
static GET_TOKEN_QUERY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*SELECT\s+afterglow_get_token\s*\(\s*\)\s*;?\s*$").unwrap()
});
static SET_TOKEN_QUERY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*SELECT\s+afterglow_set_token\s*\(\s*([A-Za-z0-9+/=]*)\s*\)\s*;?\s*$")
        .unwrap()
});

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

        if GET_TOKEN_QUERY_REGEX.is_match(query) {
            // This could be written more cleanly, but for now, we just want to make sure the session state is locked while we get the token
            // Will be optimized later when we have a better idea of how to have finer-grained locking for session state
            let mut session = self.session.lock().await;
            let session_state = session
                .entry(session_id.clone())
                .or_insert_with(SessionState::default);

            // TODO: tokens shouldn't only be base64-encoded, add HMAC or something later to make sure tokens can't be forged by clients
            let last_lsn = session_state.last_write_lsn();

            let token = match last_lsn {
                Some(lsn) => {
                    base64::engine::general_purpose::STANDARD.encode(lsn.as_u64().to_be_bytes())
                }
                None => String::new(),
            };

            let fields = Arc::new(vec![FieldInfo::new(
                "token".to_string(),
                None,
                None,
                Type::TEXT,
                FieldFormat::Text,
            )]);

            let mut encoder = DataRowEncoder::new(fields.clone());
            encoder.encode_field(&Some(token.as_str()))?;

            let row = encoder.take_row();
            let response = Response::Query(QueryResponse::new(fields, stream::iter(vec![Ok(row)])));

            return Ok(vec![response]);
        }

        if SET_TOKEN_QUERY_REGEX.is_match(query) {
            let captures = SET_TOKEN_QUERY_REGEX.captures(query).unwrap();
            let token = captures.get(1).map(|m| m.as_str()).unwrap_or("");

            let mut session = self.session.lock().await;

            // If there isn't a token - do nothing
            if !token.is_empty() {
                let session_state = session
                    .entry(session_id.clone())
                    .or_insert_with(SessionState::default);
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(token)
                    .map_err(|e| to_pg_wire_error(e.into()))?;
                let bytes: [u8; 8] = decoded.as_slice().try_into().map_err(|e| {
                    to_pg_wire_error(anyhow::anyhow!("Invalid token length: {}", e))
                })?;
                let lsn_val = u64::from_be_bytes(bytes);
                let lsn = Lsn::from_u64(lsn_val);
                session_state.record_write(lsn);
            }

            return Ok(vec![Response::Execution(pgwire::api::results::Tag::new(
                "OK",
            ))]);
        }

        let result = {
            let mut session = self.session.lock().await;
            let session_state = session
                .entry(session_id.clone())
                .or_insert_with(SessionState::default);

            // TODO: we shouldn't pass the whole session state to the router; but we also need to make sure
            // the current session state is locked for the duration of the query execution
            router::handle(query, session_state, self.pool.as_ref())
                .await
                .map_err(to_pg_wire_error)?
        };

        let mut responses = Vec::new();
        let mut current: Option<(Arc<Vec<FieldInfo>>, DataRowEncoder)> = None;
        let mut data_rows = Vec::new();

        for message in result {
            match message {
                SimpleQueryMessage::CommandComplete(_num) => {
                    if let Some((fields, _encoder)) = current.take() {
                        // If we have seen Row messages, we need to finalize the QueryResponse with the collected fields and data rows
                        let row_stream = stream::iter(std::mem::take(&mut data_rows));
                        responses.push(Response::Query(QueryResponse::new(fields, row_stream)));
                    } else {
                        // If we haven't seen any Row messages yet, we can just return a CommandComplete response without fields
                        // TODO: We might want to return a more specific tag based on the command type (e.g., "INSERT 0 1" for an insert that affected 1 row)
                        responses.push(Response::Execution(pgwire::api::results::Tag::new("OK")));
                    }
                }
                SimpleQueryMessage::Row(row) => {
                    if current.is_none() {
                        // If we haven't seen a Row message before, we need to initialize the fields and row encoder
                        let fields: Arc<Vec<FieldInfo>> = Arc::new(
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
                        );
                        current = Some((fields.clone(), DataRowEncoder::new(fields.clone())));
                    }

                    if let Some((_fields, encoder)) = &mut current {
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
