# Transactions, raw SQL, sqlx coexistence (toasty v0.10.0) — spikes Q10 & Q9

Sources: `docs/guide/src/{transactions,raw-sql,batch-operations,concurrency-control}.md`,
`crates/toasty/src/db/{tx,executor,connection,db}.rs` (as `db.rs`+`db/` module),
`examples/store-operations/src/main.rs`.

## Q10 — Transaction API (SQL backends only; DynamoDB has none)

```rust
let mut tx = db.transaction().await?;            // borrows &mut Db exclusively
toasty::create!(User { name: "Alice" }).exec(&mut tx).await?;
let u = User::get_by_id(&mut tx, &1).await?;     // reads see earlier tx writes
tx.commit().await?;                              // or tx.rollback().await?
// Drop without commit/rollback ⇒ automatic rollback (fire-and-forget in Drop).
```

Signatures (`crates/toasty/src/db/tx.rs`, `db/executor.rs`):

```rust
pub struct Transaction<'a> { /* owns or borrows a pooled Connection */ }
impl Db  { pub async fn transaction(&mut self) -> Result<Transaction<'_>>;
           pub fn transaction_builder(&mut self) -> TransactionBuilder<'_>; }
impl<'a> Transaction<'a> {
    pub async fn transaction(&mut self) -> Result<Transaction<'_>>;  // nested = SAVEPOINT
    pub async fn commit(self) -> Result<()>;      // nested: RELEASE SAVEPOINT
    pub async fn rollback(self) -> Result<()>;    // nested: ROLLBACK TO SAVEPOINT
}
#[async_trait] pub trait Executor: Send + Sync {
    async fn transaction(&mut self) -> Result<Transaction<'_>>; /* + doc-hidden exec fns */ }
// impl Executor for Db, Connection, Transaction<'a>; every builder's
// .exec(executor: &mut dyn Executor) accepts any of them.
```

- **No closure API.** Only explicit begin/commit/rollback + drop-rollback. There is
  no `db.transaction(|tx| ...)` retry helper; write your own retry loop on
  `err.is_serialization_failure()` / `is_condition_failed()`.
- **Threading through code:** `Transaction<'a>` implements `Executor`, so pass
  `&mut tx`, or generically `&mut dyn Executor` / `impl Executor` — the right shape
  for varve-store trait methods that must run inside a caller-supplied transaction.
- **Send/lifetime:** the `Executor` supertraits force `Transaction: Send + Sync`;
  holding one across `.await` in a tokio task is fine. But `'a` pins it to the
  `&mut Db` (or `&mut Connection`) borrow — it CANNOT be stored in a `'static`
  struct or returned as an owned handle detached from the `Db` borrow. A
  self-referential owned-transaction wrapper is the known workaround shape; toasty
  itself offers none. Clone the `Db` first if unrelated work needs a handle while a
  tx is open (clones share the pool).
- **Nested transactions** are savepoints, arbitrary depth, same drop-rollback.
- **Options** (`TransactionBuilder`): `.isolation(IsolationLevel::{ReadUncommitted,
  ReadCommitted,RepeatableRead,Serializable})` (PG/MySQL all four; SQLite/Turso only
  Serializable), `.read_only(true)`, `.mode(TransactionMode::{Default,Deferred,
  Immediate,Exclusive})` (Immediate/Exclusive are SQLite-only; PG/MySQL reject with
  `UnsupportedFeature`). **Gotcha:** `IsolationLevel` and `TransactionMode` are NOT
  re-exported from `toasty` — import `toasty_core::driver::operation::{IsolationLevel,
  TransactionMode}` (see `examples/store-operations`).
- **Atomic alternatives** that avoid interactive round-trips: `toasty::batch(...)`
  (tuple ≤8 / array / Vec of queries+creates, atomic), `create_many()`, query-based
  `update!`/`.delete()`, and atomic `field.add(n)`-style relative updates.
- **OCC:** `#[version]` conditions instance updates/deletes; conflict ⇒
  `Error::condition_failed`, deleted-underneath ⇒ `record_not_found`. PG bundles
  check+update in one statement; retryable serialization failures surface as
  `Error::SerializationFailure` (SQLSTATE 40001).

## Raw SQL (0.7+; SQL backends only, DynamoDB ⇒ `unsupported_feature`)

```rust
// Non-SELECT: returns affected-row count (u64).
let n = toasty::sql::statement("UPDATE users SET name = $1 WHERE id = $2")
    .bind("Alice").bind(1_i64)
    .exec(&mut db).await?;             // &mut db, &mut conn, or &mut tx

// SELECT: returns Vec<toasty::stmt::Value>; each row is Value::Record (column order).
let rows = toasty::sql::query("SELECT record, MAX(seq) FROM events GROUP BY record")
    .exec(&mut tx).await?;
for row in rows {
    let toasty::stmt::Value::Record(cols) = row else { unreachable!() };
    // cols[0], cols[1] are stmt::Value — no model hydration.
}
```

- Placeholders are NOT rewritten: PostgreSQL `$1`, MySQL `?`, SQLite/Turso `?1`
  (check `db.capability().sql_placeholder`).
- `.bind(value)` infers the DB type; `.bind_typed(Value::Null, db::Type::Timestamp(6))`
  for NULL/empty-list; `.column_types([stmt::Type::I64, stmt::Type::Bool])` to fix
  ambiguous result decoding (mainly SQLite).
- Raw SQL executes through the same `Executor` — inside a `Transaction` it commits/
  rolls back with the ORM statements, including savepoints. For session state
  (temp tables, `SET`), pin a `Connection`: `let mut conn = db.connection().await?`.

## Q9 — Coexisting with sqlx on one Postgres database

Connection model (verified in source):
- PG driver = **tokio-postgres** (`crates/toasty-driver-postgresql/Cargo.toml`) —
  sqlx appears only inside the *MySQL* driver. Pooling = toasty's own deadpool-based
  pool in the `toasty` crate; each pooled connection is a background task reached
  over channels (`db/connection_task.rs`).
- `Db::builder().connect(url)` or `.build(driver: impl Driver)`. The `Driver` trait
  (`toasty-core/src/driver.rs`) is toasty's Operation-based interface — implementing
  it over an existing sqlx pool would mean writing a whole driver, not wiring a pool.
- **ABSENT:** no API to inject an external pool, export toasty's pool, or reach the
  underlying `tokio_postgres::Client` from `Db`/`Connection`/`Transaction`
  (`Db::driver()` returns only `&dyn Driver`). ⇒ **toasty and sqlx cannot share a
  pool or a connection, hence cannot share one database transaction.**
- **SUPPORTED:** both libraries pointing at the same database/schema with separate
  pools is normal multi-client usage. Budget connections accordingly
  (`max_pool_size`, default `num_cpus * 2`). `application_name=<svc>` in the URL
  distinguishes them in `pg_stat_activity`; `options=-c search_path%3D...` pins a
  schema per pool; `table_name_prefix("...")` namespaces toasty's tables.
- Raw-SQL-within-toasty (above) covers most "need real SQL in the same tx" cases;
  anything needing sqlx *and* toasty writes atomically would have to go through one
  library only.
