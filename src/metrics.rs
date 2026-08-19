use axum::{http::StatusCode, response::IntoResponse};
use prometheus::{
    Encoder, IntCounter, IntCounterVec, register_int_counter, register_int_counter_vec,
};
use std::sync::LazyLock;

pub static READS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!("afterglow_reads_total", "Total number of reads", &["route"]).unwrap()
});

pub static WRITES: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!("afterglow_writes_total", "Total number of writes").unwrap()
});

pub static GET_TOKEN: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "afterglow_get_token_total",
        "Total number of get token requests"
    )
    .unwrap()
});

pub static SET_TOKEN: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "afterglow_set_token_total",
        "Total number of set token requests"
    )
    .unwrap()
});

pub async fn metrics_handler() -> impl IntoResponse {
    let encoder = prometheus::TextEncoder::new();
    let metric_families = prometheus::gather();

    match encoder.encode_to_string(&metric_families) {
        Ok(result) => (
            StatusCode::OK,
            [("Content-Type", encoder.format_type().to_string())],
            result,
        ),
        Err(e) => {
            tracing::error!(error = %e, "Failed to encode metrics");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("Content-Type", "text/plain".to_string())],
                "Failed to encode metrics".to_string(),
            )
        }
    }
}
