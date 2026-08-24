//! Project-local migration CLI for the kernel tables (the
//! `platform-core` pattern — toasty ships no standalone binary:
//! generating a migration diffs the *registered models* against the
//! last snapshot, so the CLI must link this crate's model types).
//!
//! Run from `platform/platform-store/` (where `Toasty.toml` lives):
//!
//! ```text
//! DATABASE_URL=postgres:///varve_platform \
//!   cargo run --features migrate-cli --bin migrate -- migration generate --name <change>
//! ```

use toasty_cli::{Config, ToastyCli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL must be set (PostgreSQL URL)"))?;
    let db = toasty::Db::builder()
        .models(toasty::models!(platform_store::*))
        .connect(&url)
        .await?;
    ToastyCli::with_config(db, config).parse_and_run().await?;
    Ok(())
}
