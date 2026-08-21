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
//! - `DATABASE_URL` (**required**): the platform PostgreSQL URL.
//!   [`platform_core::connect`] applies pending migrations once at
//!   boot, before serving (its documented single-process pattern).
//! - `HOST` / `PORT` (optional): the listen address, default
//!   `127.0.0.1:3000` — read by [`topcoat::start`].
//!
//! Shutdown is graceful on Ctrl+C / `SIGTERM` (topcoat gives
//! in-flight requests its shutdown timeout).

#![forbid(unsafe_code)]

use std::process::ExitCode;

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
    #[error("serving failed: {0}")]
    Serve(#[from] std::io::Error),
}

#[tokio::main]
async fn main() -> ExitCode {
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
    let router = platform_app::router(db);
    topcoat::start(router).await?;
    Ok(())
}
