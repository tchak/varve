# Setup, drivers, pool, testing, errors (toasty v0.10.0)

Sources: `docs/guide/src/{getting-started,database-setup,postgresql,sqlite,tracing}.md`,
`crates/toasty/Cargo.toml`, `crates/toasty/src/db/builder.rs`,
`crates/toasty-core/src/error*`, `examples/service-ops/`.

## Cargo

```toml
[dependencies]
toasty = { version = "0.10.0", features = ["postgresql", "jiff", "serde"] }
# complete feature list (crates/toasty/Cargo.toml at the tag):
#   drivers: sqlite | postgresql | mysql | turso | dynamodb
#   jiff (timestamps — required for #[auto] created_at/updated_at),
#   serde (Json<T> / serde_json::Value fields), rust_decimal, bigdecimal,
#   net (IpCidr/IpInet/MacAddr), migration (embed_migrations!),
#   rustls (default) / native-tls (MySQL TLS backend)
tokio = { version = "1", features = ["full"] }
uuid  = "1"        # uuid::Uuid fields need no toasty feature flag
jiff  = "0.2"      # when using jiff types in your own code
toasty-cli = "0.10.0"   # only in the crate that hosts the migrate binary
```

PG driver TLS is on by default (`toasty-driver-postgresql` feature `tls`, rustls);
opt out by depending on the driver crate directly with `default-features = false`.

## Opening a Db

```rust
let mut db = toasty::Db::builder()
    .models(toasty::models!(crate::*))     // or models!(User, other::Post, dep_crate::*)
    .max_pool_size(32)                     // default num_cpus * 2
    .pool_wait_timeout(Some(Duration::from_secs(5)))     // default None (wait forever)
    .pool_create_timeout(Some(Duration::from_secs(10)))
    .pool_health_check_interval(Some(Duration::from_secs(60)))  // default 60s sweep
    .pool_pre_ping(true)                   // default false; +1 RTT per checkout
    .pool_max_connection_lifetime(None)
    .pool_max_connection_idle_time(None)
    .table_name_prefix("plat_")            // namespace tables in a shared database
    .slow_statement_threshold(Some(Duration::from_millis(200)))
    .connect("postgresql://user:pass@localhost:5432/mydb")
    .await?;
```

- `models!` also pulls in every model reachable through relation/embed fields.
- URL schemes: `sqlite::memory:`, `sqlite:./file.db`, `turso:...`,
  `postgresql://` / `postgres://`, `mysql://`, `dynamodb://region`.
- Direct driver: `.build(toasty_driver_sqlite::Sqlite::in_memory())`.
- `Db` is `Clone` (shares the pool) — clone before opening a transaction if another
  handle must stay usable. Most call sites take `&mut db` because `exec` needs
  `&mut dyn Executor`.
- Pool recovery: background sweep pings idle connections; a `ConnectionLost` error
  triggers an eager sweep, so a backend restart costs ~1 failed query.

## PostgreSQL URL parameters

`application_name=`, `options=` (form-encoded startup opts — the way to set
`search_path`: `?options=-c%20search_path%3Dtenant_a`; a `SET` through pooled `Db`
only hits one connection), `sslmode=disable|prefer|require|verify-ca|verify-full`,
`sslrootcert=`, `sslcert=`+`sslkey=`, `channel_binding=`, `sslnegotiation=`.
One `Db` = one search path; per-test schemas need one `Db` each.

## Postgres-only niceties

`.ilike()`, native arrays for `Vec<scalar>` (predicates lower to `= ANY`, `@>`, `&&`,
`cardinality`), `^@` prefix match for `.starts_with()`, named enum types, full upsert
(`ON CONFLICT`), row locking (`SELECT ... FOR UPDATE` inside transactions), backward
pagination, all four isolation levels, `jsonb`.

## Testing patterns

- Unit/integration tests: `sqlite::memory:` + `db.push_schema()` — the pattern every
  upstream example and the toasty test-suite use. In-memory SQLite pins the pool to
  ONE connection (fresh DB per connect; `reset_db` is a no-op) — fine in tests, never
  in prod. SQLite lacks `varchar`/`ilike`/RETURNING-dependent bits, so anything
  PG-specific (jsonb, native arrays, `.ilike()`, isolation levels below Serializable)
  needs a real Postgres (upstream runs `cargo test -p tests --features postgresql`
  against `compose.yaml` services).
- Per-test Postgres isolation: schema-per-test + `search_path` URL option, or
  `table_name_prefix`.

## Tracing

Toasty emits one `toasty::query` tracing event per statement (`db.system`,
`db.statement`) and propagates caller spans; silent until a subscriber is installed:
`RUST_LOG=toasty=debug` with `tracing_subscriber::fmt()`. Parameter values are never
logged unless `.log_statement_params(true)`. `slow_statement_threshold` flags slow
statements.

## Errors

`toasty::Error` (= `toasty_core::Error`) is a single opaque, `Clone`, cause-chained
type — **no public enum to match on**. Classify with predicate methods
(`toasty-core/src/error/*.rs`):
`is_condition_failed()` (OCC/version conflict), `is_record_not_found()`,
`is_serialization_failure()` (PG 40001 — retry), `is_connection_lost()`,
`is_unsupported_feature()`, `is_invalid_statement()`, `is_driver_operation_failed()`
(generic DB error — **unique-constraint violations land here**, no dedicated
`is_unique_violation()` predicate exists), `is_read_only_transaction()`, etc.
Constructors: `Error::from_args(format_args!(...))` (there is no `Error::msg`),
`Error::condition_failed(ctx)`, `.context(other)`. Duplicate-key detection for a
conditional-append therefore means: attempt the insert, treat
`is_driver_operation_failed()` on a `#[unique]`/PK conflict as "already exists" —
or use `upsert_by_*(...).or_ignore()` (returns `None` on conflict) for a race-free
insert-if-absent on PG/SQLite.


## Field notes (verified in production use, 2026-08-21)

- **Connection URLs must carry a host.** toasty rejects host-less
  forms: `postgres:///db` fails with `invalid connection URL: missing
  host`. Unix-socket / implicit-host URLs are unsupported — write
  `postgres://localhost/db`.
- jiff aside (shows up next to `#[auto]` timestamps):
  `jiff::Timestamp::saturating_add` returns `Result` — it errors only
  for calendar-unit `Span`s and is infallible for `SignedDuration`;
  prefer `SignedDuration` for const-constructible TTLs.
