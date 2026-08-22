//! The Topcoat app shell (design/platform.md P.3): browser sessions adapted
//! onto `platform-core`'s session storage, principal resolution, locale
//! resolution, and the P0 pages — home, signin, signup, signout.
//!
//! **Current scope: the P0 walking-skeleton shell** (P.8, outside-in
//! ordering). Later phases add in-process document execution and the
//! components with colocated fragments; P.7's FranceConnect /
//! AgentConnect are session concerns that will land here too, invisible
//! below this crate.
//!
//! The seams (P.3):
//!
//! - `platform-core` owns storage and credentials; this crate adapts
//!   topcoat's token/hash session mechanics to it ([`auth`]) — raw
//!   tokens never reach storage, only stable hex encodings of the
//!   SHA-256 [`topcoat::session::TokenHash`].
//! - Locale is resolved **here** and only here ([`i18n`]): principal
//!   preference, then `Accept-Language`, then English. Everything below
//!   receives a typed [`platform_i18n::Locale`] as a plain value.
//! - Every user-visible string goes through
//!   [`platform_i18n::Catalogs::format`]; the provisional in-code
//!   catalogs live in [`strings`].
//!
//! - Pages ([`pages`]) are composed from [`components`]: topcoat-ui
//!   components vendored by `topcoat ui add` plus a few of our own in
//!   the same style, styled with Tailwind against the theme tokens in
//!   `styles.css` (the Tailwind input `build.rs` compiles).
//!
//! [`router`] assembles the app; `platform-server` serves it.

#![forbid(unsafe_code)]

pub mod auth;
pub mod components;
pub mod flash;
pub mod i18n;
pub mod pages;
pub mod strings;
pub mod ua;

use topcoat::{
    asset::{AssetBundle, RouterBuilderAssetExt},
    context::{Cx, app_context},
    cookie::{Key, RouterBuilderCookieExt},
    router::{Router, RouterBuilderDiscoverExt},
    session::RouterBuilderSessionExt,
};

/// Builds the platform router over a connected database (from
/// [`platform_core::connect`]), the cookie key, and, when one is
/// supplied, an asset bundle.
///
/// `cookie_key` seals the private cookies [`flash`] writes
/// (`topcoat::cookie::private_cookies` reads it from app context).
/// It must be **persisted** across restarts and shared by every
/// replica — `platform-server` loads it from `COOKIE_KEY`; tests
/// mint one per router with [`Key::generate`].
///
/// The route table is the [`pages`] module tree: `pages::builder`
/// calls `module_router!` in the route root, so every pathless
/// handler under [`pages`] registers at its module-derived path.
/// `.discover()` is still chained for explicit-path items collected
/// at link time (fonts, procedures, shards) — none yet; it costs
/// nothing and keeps the registration shape the docs show.
///
/// `assets` carries the Tailwind stylesheet (and any future static
/// files): pass the bundle `topcoat asset bundle` wrote next to the
/// binary (`platform-server` does — see its `main`). With `None` the
/// pages render without the stylesheet link — the shape router-level
/// tests use, since a test binary has no bundle of its own and
/// rendering an unbundled [`topcoat::asset::Asset`] panics by design
/// (bundle and binary must come from the same build).
///
/// The app registers no layers of its own: the principal and the
/// locale are request functions ([`auth`], [`i18n`]) that handlers
/// call on demand, so only topcoat's cookie and session layers wrap
/// the handlers (sessions outermost — among root layers the most
/// recently registered runs outermost — then cookies, which
/// [`auth`]'s session lookup reads through).
///
/// The router keeps topcoat's default [`topcoat::router::OriginPolicy`]:
/// state-changing cross-origin browser requests are rejected with 403,
/// and every state-changing route in [`pages`] is a POST, which is what
/// makes that check sufficient (GETs are deliberately unchecked).
pub fn router(db: toasty::Db, cookie_key: Key, assets: Option<AssetBundle>) -> Router {
    let builder = pages::builder()
        .discover()
        .cookies()
        .sessions(auth::session_config())
        .app_context(db)
        .app_context(cookie_key)
        .app_context(platform_graphql::schema())
        .app_context(strings::catalogs());
    match assets {
        Some(bundle) => builder.assets(bundle).build(),
        None => builder.build(),
    }
}

/// The app-context database handle, cloned per use ([`toasty::Db`] is a
/// cheap handle). Panics if the router was built without one — a
/// startup wiring bug, not a runtime condition.
pub fn db(cx: &Cx) -> toasty::Db {
    app_context::<toasty::Db>(cx).clone()
}
