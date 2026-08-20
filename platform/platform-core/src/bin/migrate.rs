//! Project-local migration CLI (toasty ships no standalone binary:
//! generating a migration diffs the *registered models* against the
//! last snapshot, so the CLI must link this crate's model types).
//!
//! Run from `platform/platform-core/` (where `Toasty.toml` lives):
//!
//! ```text
//! DATABASE_URL=postgres:///varve_platform \
//!   cargo run --features migrate-cli --bin migrate -- migration generate --name <change>
//! ```
//!
//! Subcommands: `migration generate | apply | snapshot | drop |
//! reset`. Generated SQL + snapshot + `history.toml` are committed
//! with the model change and embedded by `platform_core::MIGRATIONS`.

use toasty_cli::{Config, ToastyCli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    let url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL must be set (PostgreSQL URL)"))?;
    let db = toasty::Db::builder()
        .models(toasty::models!(platform_core::*))
        .connect(&url)
        .await?;
    ToastyCli::with_config(db, config).parse_and_run().await?;
    Ok(())
}
