# afterglow

A PostgreSQL wire-protocol proxy for read-your-writes consistency across a primary database and its read replicas, using per-session WAL LSN tracking.

Clients connect to afterglow as they would to PostgreSQL. Writes are forwarded to the primary database, while reads are served from a read replica only if that replica has replayed past the session's last write; otherwise the read falls back to the primary.

## The Problem

Adding read replicas and pointing reads at them silently breaks read-your-writes consistency. After a write, the client may read stale data from the replica if it has not yet replayed that write.

Measured over 10,000 write-then-read cycles against a local Docker primary/replica setup:

| Artificial replication lag | Stale reads (read not reflecting the write) |
| -------------------------- | ------------------------------------------- |
| 0ms (physical lag only)    | 383/10000                                   |
| 1ms                        | 9820/10000                                  |
| 25ms                       | 9999/10000                                  |
| 50ms                       | 10000/10000                                 |

Even with no artificial delay, the physical replication pipeline is slow enough to lose 3.8% of read-after-write operations. Past a millisecond, it loses almost all of them.

## How it Works

Queries are parsed to determine whether they are reads or writes.

- **Writes** are routed to the primary. Afterwards afterglow reads `pg_current_wal_lsn()` and records that position against the session. The stored LSN only ever moves forward.
- **Reads** are served from a replica only if it has replayed past the session's recorded LSN. Afterglow polls `pg_last_wal_replay_lsn()` until the replica catches up or a wait budget expires.
- **Fallback** If the budget expires, the read goes to the primary, which is always current. Correctness never depends on the replica catching up.

PostgreSQL 17 has no blocking wait primitive, so the wait is a poll loop.

### Causal Tokens

The session's consistency floor typically lives inside afterglow and is tied to one connection. Causal tokens export it so consistency can cross sessions and service boundaries:

```sql
-- service A, after a write
SELECT afterglow_get_token();
--    token
-- --------------
--  AAAAAAMFGKA=
```

A passes that opaque token to service B by any means (HTTP header, message bus, etc.). Service B can then use that token to set its consistency floor:

```sql
-- service B, on a different connection
SELECT afterglow_set_token('AAAAAAMFGKA=');
SELECT * FROM orders WHERE id = 42;   -- guaranteed to see A's write
```

Both are proxy-level pseudo-commands, intercepted before they reach PostgreSQL — no extension is installed and no such function exists in the database.

## Results

Three scenarios, identical workload (write a unique marker, immediately read it back), 10,000 iterations each, at 1ms replication lag:

| Scenario              | Stale reads | Reads served by replica |
| --------------------- | ----------- | ----------------------- |
| Primary only          | 0/10000     | 0%                      |
| Replica direct        | 9820/10000  | 100%                    |
| **Through afterglow** | **0/10000** | **100%**                |

Reading only from the primary is correct but wastes the replica. Reading from the replica uses it but is wrong. afterglow is correct _and_ uses it.

Across a range of artificial replication lags, afterglow returns 0 stale reads throughout. What changes is how many reads the replica can serve before the wait budget expires:

| Replication lag | Served from replica | Fell back to primary | Stale reads |
| --------------- | ------------------- | -------------------- | ----------- |
| 0ms             | 10000               | 0                    | 0           |
| 1ms             | 10000               | 0                    | 0           |
| 25ms            | 9999                | 1                    | 0           |
| 50ms            | 9945                | 55                   | 0           |
| 100ms           | 6                   | 9994                 | 0           |
| 200ms           | 9                   | 9991                 | 0           |
| 1000ms          | 0                   | 10000                | 0           |

The transition is a cliff, and it sits at the wait budget. Below it the replica catches up in time and serves nearly every read; above it the wait expires and afterglow degrades to primary-only behaviour — still correct, no longer offloading.

## Quickstart

```bash
export PRIMARY_URL="postgres://postgres:password@localhost:5432/app"
export REPLICA_URL="postgres://postgres:password@localhost:5433/app"
export LISTEN_ADDR="0.0.0.0:6432"
export RUST_LOG="afterglow=info"
export PGPASSWORD="password"

docker compose up -d
cargo run --release
```

Connect as you would to PostgreSQL:

```bash
psql -h localhost -p 6432 -U postgres -d app
```

Read-your-writes applies within a session, so use one interactive session rather than separate `psql -c` invocations:

```sql
INSERT INTO test_table (data) VALUES ('ryw');
SELECT * FROM test_table;   -- the inserted row is visible
```

### Benchmark

```bash
cargo run --bin bench
```

### Metrics

Prometheus metrics are served on `:9090`, separate from the proxy's listening port.

```bash
curl -s localhost:9090/metrics | grep afterglow
```

## Limitations

- Transactions do not pin a connection. Statements within an explicit transaction may be routed to different pooled connections, so multi-statement transactions are not yet supported.
- Simple query protocol only. The extended protocol is not implemented, so clients using prepared statements or bound parameters cannot connect.
- The wait is wasted when lag exceeds the budget. Above the timeout, every read waits the full budget, fails, and goes to the primary anyway. This is slower than reading the primary directly.
- Session state is never evicted. The session map grows by one entry per connection for the lifetime of the process.
- Command tags are simplified (`OK` rather than `INSERT 0 1`).
- A single mutex guards all sessions, which becomes a contention point when many sessions are active.
- No TLS, no SCRAM, no in-proxy authentication.

## Roadmap

Full roadmap is available in [ROADMAP.md](ROADMAP.md).
