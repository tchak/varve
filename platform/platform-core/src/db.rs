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
/// Migration application is not guarded against concurrent callers:
/// toasty 0.10 runs each migration in its own transaction but takes
/// no lock around creating `__toasty_migrations` or checking what is
/// pending, so two processes migrating a fresh database at once can
/// collide. Call this once at boot before serving (the
/// `platform-server` pattern); anything needing multi-replica boot
/// safety must add its own advisory lock around it.
pub async fn connect(url: &str) -> toasty::Result<toasty::Db> {
    let db = toasty::Db::builder()
        .models(toasty::models!(crate::*))
        .connect(url)
        .await?;
    MIGRATIONS.apply(&db).await?;
    Ok(db)
}
