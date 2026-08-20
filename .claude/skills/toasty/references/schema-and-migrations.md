# Schema management & migrations (toasty v0.10.0)

Sources: `docs/guide/src/schema-management.md`, `docs/guide/src/postgresql.md`
(§Migrations), `examples/service-ops/` at tag `toasty-v0.10.0`.

There is **no external schema DSL and no standalone CLI binary**. The schema is
derived from the registered models; migrations are diff-generated SQL managed by a
project-local CLI you build from the `toasty-cli` *library* crate (it needs your
model types to compute the schema — a prebuilt binary can't).

## Dev/test path: `push_schema`

```rust
let mut db = toasty::Db::builder()
    .models(toasty::models!(crate::*))
    .connect(url).await?;
db.push_schema().await?;   // CREATE TABLE/INDEX for everything, every time; no diffing
```

Fine for prototypes and tests; use migrations once real data exists.
`db.reset_db()` drops and recreates the database (no-op on in-memory SQLite).

## Production path: generated SQL migrations

Project-local CLI binary (`src/bin/migrate.rs`):

```rust
use toasty_cli::{Config, ToastyCli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load()?;                       // reads Toasty.toml
    let db = toasty::Db::builder()
        .models(toasty::models!(my_app::*))             // SAME models the app runs
        .connect(&std::env::var("DATABASE_URL")?).await?;
    ToastyCli::with_config(db, config).parse_and_run().await?;
    Ok(())
}
```

`Toasty.toml`:

```toml
[migration]
path = "toasty"                 # migrations/, snapshots/, history.toml live here
prefix_style = "Sequential"     # or "Timestamp"
checksums = false               # true: MD5-detect edited migration files
statement_breakpoints = true    # -- #[toasty::breakpoint] separators in the SQL
```

Subcommands: `migration generate [--name x]` (diffs models vs last snapshot →
`NNNN_x.sql` + TOML snapshot + history entry; interactive rename detection turns
drop+add into `ALTER ... RENAME`), `migration apply` (runs pending, each inside a
transaction, tracked in the auto-created `__toasty_migrations` table),
`migration snapshot` (print schema TOML), `migration drop [--latest|--name]`,
`migration reset [--skip-migrations]`.

Generated files are plain backend-specific DDL — review and commit them (SQL,
snapshot, history.toml) with the model change.

## Embedded migrations (0.10, feature `migration`)

```rust
static MIGRATIONS: toasty::migration::MigrationSet = toasty::embed_migrations!();
// or toasty::embed_migrations!("toasty/primary") — path relative to Cargo.toml

let report = MIGRATIONS.apply(&db).await?;   // skips ids already in __toasty_migrations
println!("applied {}", report.applied());
```

Compile-time checks: invalid history, duplicate id/name, missing SQL file all fail
the build. One `MigrationSet` per database; the app decides which set applies to
which `Db`.

## PostgreSQL specifics

- Each migration applies inside `BEGIN`/`COMMIT`; failure rolls back cleanly.
- Enum `CREATE TYPE`/`ALTER TYPE ... ADD VALUE` statements are emitted before the
  tables that use them (embedded enums are real PG enum types).
- Column changes emit one statement per property (type, name, NOT NULL, default).
- **ABSENT:** no zero-downtime tooling (no `CREATE INDEX CONCURRENTLY`, dual-write);
  migrations assume exclusive schema access.

## Workflow

1. Edit model structs → 2. `migration generate --name change` → 3. review SQL →
4. `migration apply` → 5. commit SQL + snapshot + history with the code.
Keep models in a library crate consumed by both the app binary and the migrate
binary so the diff sees exactly what the server runs (`examples/service-ops`).
