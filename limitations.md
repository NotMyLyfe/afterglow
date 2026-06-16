# afterglow - Known Limitations

afterglow is a Postgres wire-protocol proxy providing read-your-writes consistency
via per-session WAL LSN tracking. The following are **deliberate scope decisions**
made during the initial build, not unknown bugs. Each notes the current behaviour and
the intended fix.

## Transactions do not pin a connection

Each statement within an explicit transaction may be routed to a different primary
connection from the pool. Because a transaction's state (locks, uncommitted changes,
snapshot) lives on a single backend connection, multi-statement transactions are not
yet correct, i.e. a `COMMIT` may land on a connection that never saw the matching
`BEGIN`.

The session tracks an `in_transaction` flag and routes transaction statements to the
primary, but the connection itself is not held across statements.

**Fix:** hold the checked-out primary connection in `SessionState` for the duration of
the transaction, releasing it on `COMMIT`/`ROLLBACK`.

## Session map is not evicted on disconnect

Per-connection `SessionState` is stored in a shared map keyed by a minted connection
ID. The proxy's query handler is not notified on disconnect, so entries are never
removed - the map grows by one entry per connection for the lifetime of the process.

This is fine at demo and benchmark scale (dozens to hundreds of connections) but is a
slow memory leak under sustained production traffic.

**Fix:** hook a connection-teardown callback to evict the session entry on disconnect.

## Command tags are simplified

The proxy emits a generic `OK` tag on command completion rather than the standard
Postgres tags (`INSERT 0 1`, `UPDATE 3`, `SELECT 5`, etc.). The affected-row count is
available but not yet threaded into the tag.

Clients accept the generic tag, but tools that parse the command tag will not see the
verb or row count.

**Fix:** construct the tag from the statement kind (already known via the classifier)
and the row count from `CommandComplete`.

## LSN capture is not atomic with the write

After a write executes on the primary, a separate `SELECT pg_current_wal_lsn()` query
captures the resulting WAL position. There is a small window where the write commits
but the LSN-capture query fails, leaving the write durable but unrecorded in the
session. A subsequent read could then be served from a not-yet-caught-up replica,
violating read-your-writes for that one write.

This is extremely unlikely against a healthy primary, and the client receives an error
in that case regardless.

**Fix:** capture the LSN inline with the write (via `RETURNING pg_current_wal_lsn()`
rewriting, or logical decoding), which also eliminates the extra round-trip.

## Mutating functions are misclassified as reads

The query classifier treats `SELECT pg_advisory_lock(...)`, `SELECT nextval(...)`,
`SELECT setval(...)` and similar as reads, because it does not walk the SELECT's
function-call nodes to detect state-mutating built-ins. These could be routed to a
replica, where they would fail or behave incorrectly.

**Fix:** maintain a blocklist of mutating built-in functions and inspect the SELECT's
target list / function calls; classify as `Write` if any are present.

## Empty-result SELECTs may emit an Execution tag

The Query-vs-Execution decision is currently keyed on whether any data rows were
collected. A SELECT that returns zero rows (e.g. `SELECT * FROM t WHERE false`) may be
emitted as an Execution completion rather than an empty Query response with column
headers.

**Fix:** key the decision on whether the statement is row-returning (i.e. whether field
descriptions were built) rather than on whether any rows were produced.

## No extended query protocol

Only the simple query protocol is implemented. Clients defaulting to the extended
protocol (prepared statements, parameter binding — e.g. many ORMs and language drivers)
are not supported. `psql` and simple-protocol clients work.

**Fix:** implement `ExtendedQueryHandler`. Note that prepared-statement support
interacts with connection pooling and would need care.

## No TLS, no SCRAM authentication

TLS is disabled; client credentials are passed through to the backend. There is no
in-proxy authentication.

**Fix:** add optional TLS termination and an auth layer if deployed beyond a trusted
network.

## Single mutex guards all sessions

All per-connection sessions are stored behind one `Mutex<HashMap<…>>`. Each query locks
it briefly to look up its session. Lock hold time is negligible (an in-memory lookup),
but at high connection counts the single global lock becomes a contention point.

**Fix:** shard the session store (e.g. a concurrent map such as `DashMap`, or
per-connection storage) to remove the global lock.

## Replica wait is poll-based, not blocking

Postgres 17 does not provide a blocking "wait until replayed to LSN" function
(`pg_wal_replay_wait` is Postgres 18+). The proxy therefore polls
`pg_last_wal_replay_lsn()` on a short interval until the replica catches up or a timeout
elapses. Each poll is a round-trip; under cross-region latency this adds chatter.

**Fix:** on Postgres 18+, use the blocking `pg_wal_replay_wait` to eliminate poll
round-trips. The wait function is already structured to be swapped without touching the
routing logic.
