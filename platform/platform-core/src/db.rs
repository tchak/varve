//! Database bootstrap: one connection entry point that registers the
//! platform models and brings the schema up to date.
//!
//! The schema is derived from the models (toasty has no external
//! schema DSL); its evolution is tracked as generated SQL migrations
//! under `toasty/` (`migrations/`, `snapshots/`, `history.toml`),
//! produced by the project-local `migrate` CLI (`src/bin/migrate.rs`,
//! feature `migrate-cli`) and compiled into the crate via
//! [`toasty::embed_migrations!`]. [`connect`] applies whatever is
//! pending on every call — application in `__toasty_migrations` is
//! idempotent, so a fully migrated database costs one read.
//!
//! Workflow when models change: edit the structs, then from this
//! crate's directory run
//! `DATABASE_URL=... cargo run --features migrate-cli --bin migrate -- migration generate --name <change>`,
//! review the SQL, and commit it together with the model change. The
//! embedded set picks the new file up at the next build (the macro
//! registers `history.toml` as a compile-time dependency).
//!
//! **Foreign keys are hand-written.** toasty derives none from
//! `#[belongs_to]`, so `0005_foreign_keys.sql` adds one constraint
//! per relation by hand (with its ON DELETE policy stated there), and
//! any migration adding a relation must add its constraint the same
//! way — the generator will not.

/// Every migration this build knows about, embedded at compile time
/// from `toasty/` (path relative to this crate's `Cargo.toml`).
///
/// `platform-server` will apply this same set at boot; tests apply it
/// through [`connect`].
pub static MIGRATIONS: toasty::migration::MigrationSet = toasty::embed_migrations!();

/// Connects to the platform database (PostgreSQL URL, e.g.
/// `postgres:///varve_platform`), registers every model in this
/// crate, and applies pending migrations.
///
/// This is the one bootstrap path: anything holding a
/// [`toasty::Db`] from here is guaranteed a schema matching the
/// models this build was compiled with. Pool sizing and the other
/// [`toasty::Db::builder`] knobs stay at their defaults for P0; they
/// become `platform-server` configuration when it exists (P.3).
/// # Concurrency
///
/// toasty 0.10 runs each migration in its own transaction but takes
/// no lock around creating `__toasty_migrations` or checking what is
/// pending, so two callers migrating a fresh database at once would
/// collide. `connect` therefore holds a session-level advisory lock
/// (`MIGRATION_LOCK`) on a pinned connection for the duration of
/// the apply: concurrent callers — multi-replica boot, test
/// processes sharing one database — queue on it and each finds the
/// schema complete. The lock is released before returning; it never
/// outlives a crashed caller either, since Postgres drops it with
/// the session.
pub async fn connect(url: &str) -> toasty::Result<toasty::Db> {
    connect_with(url, toasty::ModelSet::new(), &[]).await
}

/// [`connect`] with additional model and migration sets from crates
/// that own tables in the same database — today `platform-store`'s
/// kernel tables (P.3, *migrations and registration*): one database,
/// one `__toasty_migrations` table, each crate owning its files.
/// Extra sets apply after this crate's, inside the same advisory
/// lock.
pub async fn connect_with(
    url: &str,
    extra_models: toasty::ModelSet,
    extra_migrations: &[&toasty::migration::MigrationSet],
) -> toasty::Result<toasty::Db> {
    let mut models = toasty::models!(crate::*);
    for model in extra_models {
        models.add(model);
    }
    let db = toasty::Db::builder().models(models).connect(url).await?;
    // Advisory locks are per session, so the lock and unlock must
    // run on the same pinned connection; `apply` draws its own from
    // the pool, which the lock serializes across callers, not
    // across connections.
    let mut conn = db.connection().await?;
    toasty::sql::query("SELECT 1 FROM pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK)
        .exec(&mut conn)
        .await?;
    let mut applied = MIGRATIONS.apply(&db).await;
    if applied.is_ok() {
        for set in extra_migrations {
            applied = set.apply(&db).await;
            if applied.is_err() {
                break;
            }
        }
    }
    let unlocked = toasty::sql::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK)
        .exec(&mut conn)
        .await;
    applied?;
    unlocked?;
    Ok(db)
}

/// Key of the advisory lock [`connect`] holds while applying
/// migrations. Any fixed 64-bit value works as long as nothing else
/// on the same database uses it for another purpose.
const MIGRATION_LOCK: i64 = 0x7661_7276_6521_6d69; // "varve!mi"
