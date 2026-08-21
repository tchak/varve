//! The platform binary (PLATFORM.md P.3): connects the database,
//! builds the `platform-app` router, and serves it. Later phases
//! mount `/graphql` and the upload/download `#[route]` handlers and
//! start the `platform-jobs` runners here — one process to start
//! with; the seams are already crates, so this stays thin.
//!
//! # Running
//!
//! ```text
//! DATABASE_URL=postgres://localhost/varve_platform cargo run -p platform-server
//! ```
//!
//! or put the variables in a `.env` file (loaded from the working
//! directory or any parent; real environment variables win; see
//! `.env.example` at the repo root) and just `cargo run -p
//! platform-server`.
//!
//! - `DATABASE_URL` (**required**): the platform PostgreSQL URL.
//!   [`platform_core::connect`] applies pending migrations once at
//!   boot, before serving (its documented single-process pattern).
//! - `HOST` / `PORT` (optional): the listen address, default
//!   `127.0.0.1:3000` — read by [`topcoat::start`].
//!
//! # Static assets (the Tailwind stylesheet)
//!
//! The pages' stylesheet is a topcoat asset: `topcoat asset bundle
//! --package platform-server` (or `topcoat dev`) writes an `assets/`
//! directory next to this binary, and [`AssetBundle::load`] picks it
//! up at boot. Without a bundle (plain `cargo run` with no bundling
//! step) the server comes up and serves every page *unstyled* — the
//! layout omits the stylesheet link — with a warning on stderr;
//! bundle and binary must come from the same build (asset IDs embed
//! `OUT_DIR` paths), so a stale directory fails at render, not
//! silently.
//!
//! Shutdown is graceful on Ctrl+C / `SIGTERM` (topcoat gives
//! in-flight requests its shutdown timeout).

#![forbid(unsafe_code)]

use std::io;
use std::process::ExitCode;

use topcoat::asset::AssetBundle;

/// Everything that can stop the server from coming up (or bring it
/// down), each with an actionable message — a missing variable must
/// not surface as a panic backtrace.
#[derive(Debug, thiserror::Error)]
enum ServerError {
    #[error(
        "DATABASE_URL is not set — export the platform PostgreSQL URL, \
         e.g. DATABASE_URL=postgres://localhost/varve_platform"
    )]
    MissingDatabaseUrl,
    #[error("connecting to the database (or applying migrations) failed: {0}")]
    Database(#[from] toasty::Error),
    #[error("loading the asset bundle next to the executable failed: {0}")]
    Assets(io::Error),
    #[error("serving failed: {0}")]
    Serve(#[from] io::Error),
}

#[tokio::main]
async fn main() -> ExitCode {
    // A `.env` in the working directory (or any parent) fills in
    // missing variables before anything reads the environment; real
    // environment variables always win, and no file is fine —
    // `.env` is a dev convenience (see `.env.example`), never a
    // deployment mechanism.
    let _ = dotenvy::dotenv();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("platform-server: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), ServerError> {
    let database_url =
        std::env::var("DATABASE_URL").map_err(|_| ServerError::MissingDatabaseUrl)?;
    let db = platform_core::connect(&database_url).await?;
    let assets = match AssetBundle::load() {
        Ok(bundle) => Some(bundle),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            eprintln!(
                "platform-server: no asset bundle next to the executable — serving without \
                 the stylesheet; bundle with `topcoat asset bundle --package platform-server`"
            );
            None
        }
        Err(error) => return Err(ServerError::Assets(error)),
    };
    let router = platform_app::router(db, assets);
    topcoat::start(router).await?;
    Ok(())
}
