//! Install the browsers matching the Playwright driver bundled with
//! the pinned `playwright-rs` dev-dependency (copied from that crate's
//! `examples/install-browsers.rs`, as its README recommends — the
//! browser version then rides `Cargo.lock`, so a crate bump moves
//! crate, driver, and browsers together with no script edit).
//!
//! The browser end-to-end tests (`tests/e2e.rs`) need only chromium:
//!
//! ```text
//! cargo run -p platform-app --example install-browsers -- chromium
//! ```
//!
//! With no arguments, all three browsers are installed. On Linux,
//! required system libraries install automatically alongside the
//! browsers; pass `--with-deps` to force that on other platforms.

use playwright_rs::{install_browsers, install_browsers_with_deps};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let with_deps = args.iter().any(|arg| arg == "--with-deps");
    let browsers: Vec<&str> = args
        .iter()
        .filter(|arg| *arg != "--with-deps")
        .map(String::as_str)
        .collect();
    let selection = (!browsers.is_empty()).then_some(browsers.as_slice());
    if with_deps {
        install_browsers_with_deps(selection).await?;
    } else {
        install_browsers(selection).await?;
    }
    Ok(())
}
