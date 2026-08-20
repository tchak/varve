---
name: toasty
description: Distilled reference for the toasty async ORM (tokio-rs), pinned at v0.10.0. ALWAYS load this before writing, reviewing, or discussing toasty models, queries, transactions, migrations, or raw SQL — toasty breaks its public API every few weeks (0.6→0.10 in May–Aug 2026 alone) and anything produced from prior knowledge WILL be stale. Also load for the platform-store spikes (Q1 dynamic predicates, Q9 sqlx coexistence, Q10 transactions).
---

# Toasty v0.10.0 (pinned)

- **Pinned:** `toasty = "0.10.0"`, distilled from the upstream tag `toasty-v0.10.0`
  (github.com/tokio-rs/toasty) on **2026-08-20**.
- **Regenerate this skill whenever the dependency is bumped.** Every claim below is
  traceable to that tag; a version bump invalidates the skill until re-distilled.
- **Hard rule:** when unsure of a signature or attribute, read the real source in
  `~/.cargo/registry/src/*/toasty-0.10.0/` (or a vendored checkout of the tag) rather
  than guessing. Blog posts and LLM memory describe pre-release APIs
  (`toasty::schema!`, `.sql` schema files, `find_by_*`, generated `db::links`) that
  **no longer exist**.

## Concept map

Toasty is an *application-level query engine*, not a SQL string builder. Models are
plain structs with `#[derive(toasty::Model)]`; the derive generates typed builders
(`create()`, `all()`, `filter()`, `get_by_*`, `filter_by_*`, `update_by_*`,
`upsert_by_*`, `delete_by_*`) plus a `fields()` path accessor per model. Queries are
values: a `Query<T>` wraps an untyped statement AST (`toasty_core::stmt`) that a
multi-phase engine (simplify → lower → plan → execute) compiles per backend — SQL
(SQLite, Turso, PostgreSQL, MySQL) or key-value (DynamoDB). The schema has two layers
(app schema = models, db schema = tables) bridged by a mapping; `Db::builder()`
registers models (`toasty::models!(crate::*)`) and derives the full DB schema from
them. Everything executes through the `Executor` trait, implemented by `Db` (pooled),
`Connection` (pinned), and `Transaction` (savepoint-nested). Toasty deliberately does
NOT hide backend differences: `.ilike()` is PostgreSQL-only, MySQL has no upsert, etc.

## Routing

| Topic | File |
|---|---|
| Model derive, every attribute, relations, type mapping, JSON, enums, embeds | [references/models.md](references/models.md) |
| CRUD, filters, **runtime/dynamic query construction (spike Q1)**, pagination, select/count, includes | [references/queries.md](references/queries.md) |
| **Transactions (spike Q10), raw SQL + sqlx coexistence (spike Q9)**, batches, optimistic concurrency | [references/transactions-and-raw-sql.md](references/transactions-and-raw-sql.md) |
| `push_schema`, migration CLI, `embed_migrations!` | [references/schema-and-migrations.md](references/schema-and-migrations.md) |
| Cargo features, `Db::builder` + pool knobs, PostgreSQL driver, testing, tracing, errors | [references/setup-and-drivers.md](references/setup-and-drivers.md) |

## Spike verdicts (varve platform-store)

- **Q1 — runtime-built nested and/or trees: SUPPORTED.** `Expr<bool>` is a plain
  `Clone` value; `.and()/.or()/.not()/Expr::and_all(iter)` compose at runtime, and
  `Path::any()/.all()` on a *declared* relation emit `IN (SELECT …)` subqueries.
  **Arbitrary EXISTS against an unrelated table: ABSENT** from the typed API (the AST
  has `Expr::Exists` but nothing public builds it). Details in queries.md §Dynamic.
- **Q10 — transactions: SUPPORTED** (interactive, savepoints, isolation levels,
  auto-rollback on drop). `Transaction<'a>` borrows `&mut Db` for its lifetime — it
  can be threaded as `&mut Transaction` / `&mut dyn Executor` but not stored `'static`.
  Details in transactions-and-raw-sql.md.
- **Q9 — sharing with sqlx: same *database* yes, same *pool/connection* NO.** The PG
  driver is tokio-postgres behind toasty's own deadpool; no pool injection, no access
  to the underlying client. Raw SQL escape hatch (`toasty::sql::{statement,query}`)
  runs inside toasty transactions. sqlx must keep its own pool; cross-library
  transactions are impossible. Details in transactions-and-raw-sql.md.

## 0.6 → 0.10 breaking-changes digest (why May-2026 material is stale)

From `crates/toasty/CHANGELOG.md` at the tag:

- **0.7** (2026-05): `#[serialize(json)]` → `toasty::Json<T>` wrapper; `#[deferred]`
  attribute removed → wrap the type in `Deferred<T>`; raw SQL API added
  (`toasty::sql`); `update!` macro added; `.ilike()` restricted to PostgreSQL;
  increment/decrement/add/subtract update ops added (breaking); Turso driver;
  `Model::PrimaryKey`; multi-step `via` relations.
- **0.8** (2026-07): per-model query structs **unified into generic `Query<T>`**;
  `Register` trait removed; `RelationManyField/RelationOneField` assoc type renamed
  `Target`; `UpdateByKey` returning-columns explicit; `#[version]` OCC lands on SQL
  drivers; `#[belongs_to]` `key`/`references` now inferred; `between` added.
- **0.9** (2026-07): **JSON fields now require an explicit `#[column(type = ...)]`**
  (breaking); native `json`/`jsonb` columns; `serde_json::Value` fields; upsert
  (`upsert_by_*`) added; `#[document]` storage; enum-level `#[index]`/`#[unique]`;
  integer enum discriminants; relation link/unlink became builders (breaking).
- **0.10** (2026-08): `Capability::sql` names the SQL `Dialect` (breaking); unused
  schema/statement APIs removed (breaking); MySQL TLS features made additive with
  SQLx (breaking); `embed_migrations!`; network address types; asc/desc on newtype
  embeds; many cursor-pagination fixes.
- Older prerelease-era names — `find_by_*`, schema files, codegen CLI — are long gone:
  lookups are `get_by_*` (immediate) and `filter_by_*` (builder).
