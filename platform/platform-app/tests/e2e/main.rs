//! # Browser tests
//!
//! Real-browser end-to-end tests: the app is served in-process on an
//! ephemeral port ([`topcoat::serve_until`]) and driven headless
//! through [`playwright-rs`](https://docs.rs/playwright-rs) on every
//! installed engine — chromium, firefox, and webkit. Each test runs
//! its scenario against each engine in turn (serially, with a fresh
//! browser context per engine) and reports failures per engine.
//!
//! **Gating** (the settled P.3 convention — `cargo test --workspace`
//! stays green with nothing installed): each test passes vacuously,
//! after printing why, unless **both**
//!
//! 1. `VARVE_TEST_DATABASE_URL` is set (same scratch database as
//!    `tests/app/`), and
//! 2. at least one Playwright engine matching the bundled driver is
//!    installed — a miss surfaces as the crate's
//!    [`playwright_rs::Error::BrowserNotInstalled`] at launch. Each
//!    missing engine prints its own `skipped:` line and is dropped
//!    from the run; the installed ones still run.
//!
//! **One-time setup.** The pinned `playwright-rs` dev-dependency
//! downloads its Playwright driver (with its own Node runtime) at
//! build time; browsers are installed once, through the crate so the
//! versions match, via the vendored example (no arguments installs
//! all three engines):
//!
//! ```text
//! cargo run -p platform-app --example install-browsers
//! ```
//!
//! Then run for real with e.g.:
//!
//! ```text
//! VARVE_TEST_DATABASE_URL=postgres://localhost/varve_platform_test \
//!   cargo test -p platform-app --test e2e
//! ```
//!
//! Tests share one database and run in parallel, so every test mints
//! unique emails (unique per engine too — engines within one test
//! share the database), uses a fresh browser context per engine, and
//! never asserts on global counts (same rules as `tests/app/`). On
//! failure a Playwright trace named after the scenario *and* engine
//! (e.g. `signup-roundtrip.webkit.trace.zip`) is written under
//! `CARGO_TARGET_TMPDIR` (the path is printed); open it at
//! <https://trace.playwright.dev>.
//!
//! **Layout.** One test binary, split by *subject* (journeys, not
//! pages — flows span pages, and subjects map to how the platform
//! grows): `harness` holds the shared machinery (gating, the
//! in-process app, the engine loop, traces); each other module is
//! one subject (`auth`, `i18n`, ...). Filter per subject with e.g.
//! `cargo test --test e2e auth::`. A new subject is a new module —
//! never grow one file past its subject.
//!
//! **Known engine divergence.** WebKit refuses the app's `Secure`
//! `__Host-session` cookie over plain-http loopback, so signed-in
//! flows cannot run there; the affected scenarios assert that
//! refusal explicitly instead — see
//! [`accepts_secure_cookie_on_loopback_http`].

mod auth;
mod harness;
mod i18n;
