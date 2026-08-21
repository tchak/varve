//! # Router-level tests
//!
//! Level 2 of the platform test policy (CLAUDE.md § Platform test
//! policy): everything goes through `Router::handle` — no listener,
//! no browser. These tests own route/status/redirect contracts, form
//! handling, session mechanics as headers, page markup and strings,
//! and locale resolution from headers. Fast — the **default home for
//! new behavior**; reach for `tests/e2e/` only when a real browser
//! proves something this level cannot.
//!
//! **Gating.** Tests run over the real database, gated on
//! `VARVE_TEST_DATABASE_URL` (the settled P.3 convention, same as
//! `platform-core/tests/db.rs`): `cargo test --workspace` stays
//! green without Postgres. Run for real with e.g.:
//!
//! ```text
//! VARVE_TEST_DATABASE_URL=postgres://localhost/varve_platform_test \
//!   cargo test -p platform-app --test app
//! ```
//!
//! Tests share one database and run in parallel, so every test mints
//! unique emails and never asserts on global counts.
//!
//! **Layout.** One test binary, split by *subject* (journeys and
//! features, not pages — flows span pages, and subjects map to how
//! the platform grows): `harness` holds the shared machinery (gating,
//! request builders, form encoding, the signup helper); each other
//! module is one subject (`auth`, `i18n`, `shell`, ...). Filter per
//! subject with e.g. `cargo test --test app auth::`. A new subject is
//! a new module — never grow one file past its subject.

mod auth;
mod harness;
mod i18n;
mod shell;
