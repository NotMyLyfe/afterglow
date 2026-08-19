use prometheus::{IntCounter, IntCounterVec, register_int_counter, register_int_counter_vec};
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
