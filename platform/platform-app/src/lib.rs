//! The Topcoat app shell (PLATFORM.md P.3): browser sessions adapted
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
pub mod i18n;
pub mod pages;
pub mod strings;
pub mod ua;

use topcoat::{
    asset::{AssetBundle, RouterBuilderAssetExt},
    context::{Cx, app_context},
    cookie::RouterBuilderCookieExt,
    router::{Router, RouterBuilderDiscoverExt},
    session::RouterBuilderSessionExt,
};

/// Builds the platform router over a connected database (from
/// [`platform_core::connect`]) and, when one is supplied, an asset
/// bundle.
///
/// The route table is the [`pages`] module tree: `pages::builder`
/// calls `module_router!` in the route root, so every pathless
/// handler under [`pages`] registers at its module-derived path.
/// `.discover()` is still needed on top for the explicit-path items
/// collected at link time — [`auth`]'s request-state layer at `/`.
///
/// `assets` carries the Tailwind stylesheet (and any future static
/// files): pass the bundle `topcoat asset bundle` wrote next to the
/// binary (`platform-server` does — see its `main`). With `None` the
/// pages render without the stylesheet link — the shape router-level
/// tests use, since a test binary has no bundle of its own and
/// rendering an unbundled [`topcoat::asset::Asset`] panics by design
/// (bundle and binary must come from the same build).
///
/// Layer nesting is load-bearing: among same-path (root) layers the
/// most recently registered runs outermost, so the chain below runs
/// sessions → cookies → [`auth`]'s request-state layer (discovered
/// first, hence innermost) → handlers. The request-state layer needs
/// both the session cell and the cookie jar in scope to resolve the
/// presented token, which this ordering guarantees.
///
/// The router keeps topcoat's default [`topcoat::router::OriginPolicy`]:
/// state-changing cross-origin browser requests are rejected with 403,
/// and every state-changing route in [`pages`] is a POST, which is what
/// makes that check sufficient (GETs are deliberately unchecked).
pub fn router(db: toasty::Db, assets: Option<AssetBundle>) -> Router {
    let builder = pages::builder()
        .discover()
        .cookies()
        .sessions(auth::session_config())
        .app_context(db)
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
