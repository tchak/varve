//! The Topcoat app shell (PLATFORM.md P.3): browser sessions adapted
//! onto `platform-core`'s session storage, principal resolution, locale
//! resolution, and the P0 pages — home, login, signup, logout.
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
//! [`router`] assembles the app; `platform-server` serves it.

#![forbid(unsafe_code)]

pub mod auth;
pub mod i18n;
pub mod pages;
pub mod strings;

use topcoat::{
    context::{Cx, app_context},
    cookie::RouterBuilderCookieExt,
    router::{Router, RouterBuilderDiscoverExt},
    session::RouterBuilderSessionExt,
};

/// Builds the platform router over a connected database (from
/// [`platform_core::connect`]).
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
pub fn router(db: toasty::Db) -> Router {
    Router::builder()
        .discover()
        .cookies()
        .sessions(auth::session_config())
        .app_context(db)
        .app_context(strings::catalogs())
        .build()
}

/// The app-context database handle, cloned per use ([`toasty::Db`] is a
/// cheap handle). Panics if the router was built without one — a
/// startup wiring bug, not a runtime condition.
pub fn db(cx: &Cx) -> toasty::Db {
    app_context::<toasty::Db>(cx).clone()
}
