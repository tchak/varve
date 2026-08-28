//! Where to send the browser after a successful sign-in, carried in
//! a private cookie (design/platform.md P.7, settled 2026-08-28).
//!
//! When a signed-in subtree's gate turns an anonymous GET's 401 into
//! the 303 to `/signin`, the location the user actually wanted is
//! about to be lost. [`remember`] parks it — path + query,
//! origin-relative — in a cookie with the same hardening as
//! [`crate::flash`] (encrypted and authenticated, `__Host-`,
//! `Secure`, `HttpOnly`, `SameSite=Lax`), and [`take`] consumes it
//! once when a sign-in or sign-up succeeds. The cookie being
//! server-minted is the security design: a crafted link cannot plant
//! a redirect target, so the open-redirect class that haunts
//! `?return_to=` parameters is closed structurally, not by
//! vigilance. Validation still runs at both ends (`is_valid`)
//! because the value crosses time — a rotated key or stale shape
//! reads as absent, never as an error.
//!
//! This is *not* a flash: a flash is consumed on the next request,
//! but this value must survive several — the `/signin` render, a
//! failed credentials POST, a detour through `/signup` — until a
//! *successful* authentication takes it. Hence its own module and
//! its own, longer `MAX_AGE`.
//!
//! Only GET navigations are recorded ([`remember`] checks the
//! method): a replayed POST cannot be reconstructed by a redirect,
//! so remembering its URL would land the user on a page they never
//! asked to see. The recording sites are the subtree gates alone,
//! which structurally never wrap `/signin`, `/signup`, or
//! `/signout` — those pages are public — so no path exclusion list
//! exists here to drift.
//!
//! Future flows that cross devices or time (magic links,
//! sign-in-by-email-confirmation) must copy this cookie's value into
//! their server-side token row at issuance and honor it —
//! revalidated — at redemption. The return location never rides in
//! an emailed URL (P.7: never client-writable, and mail
//! infrastructure rewrites and logs links).

use topcoat::{
    context::Cx,
    cookie::{Cookies, SameSite, cookie_store, private_cookies, time::Duration},
    router::{Method, request},
};

/// The cookie's base name; the jar prefixes it to `__Host-return-to`.
const NAME: &str = "return-to";

/// How long an unconsumed return location survives on the client:
/// long enough to type credentials or detour through `/signup`,
/// short enough that an abandoned attempt does not ambush a sign-in
/// days later.
const MAX_AGE: Duration = Duration::minutes(15);

/// The longest location worth remembering; anything beyond this is
/// dropped rather than truncated (a truncated URL is a different
/// URL).
const MAX_LEN: usize = 2048;

/// The jar every return location goes through, on write and on
/// read/removal alike — the [`crate::flash`] shape, so removal
/// reapplies the attributes the value was set with.
fn jar(cx: &Cx) -> impl Cookies + '_ {
    private_cookies(cx)
        .default_http_only(true)
        .default_same_site(SameSite::Lax)
        .default_max_age(MAX_AGE)
        .default_prefix_host()
}

/// An origin-relative location this module is willing to redirect
/// to: rooted at `/` but not protocol-relative (`//host` — the
/// browser would leave the origin), free of backslashes (some
/// browsers read `/\` as `//`) and control bytes, and short enough
/// to be a URL someone navigated to.
fn is_valid(location: &str) -> bool {
    location.len() <= MAX_LEN
        && location.starts_with('/')
        && !location.starts_with("//")
        && !location
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'\\')
}

/// Records the request's own location for [`take`] — the gates call
/// this at the moment they turn an `UnauthorizedError` into the 303
/// to `/signin`. A non-GET request, or a location `is_valid`
/// refuses, records nothing; the pre-rewrite client URL is what the
/// browser will be sent back to, so that is what is stored.
pub fn remember(cx: &Cx) -> topcoat::Result<()> {
    if request::method(cx) != Method::GET {
        return Ok(());
    }
    let uri = request::original_uri(cx);
    let location = uri
        .path_and_query()
        .map_or_else(|| uri.path(), |path_and_query| path_and_query.as_str());
    if !is_valid(location) {
        return Ok(());
    }
    cookie_store::<String, _>(jar(cx), NAME)
        .set(location.to_owned())
        .commit()?;
    Ok(())
}

/// Consumes the remembered location: returns it when present,
/// readable, and still `is_valid`, and queues the cookie's removal
/// on this response either way. Call only when a sign-in or sign-up
/// *succeeded* — a failed submission must leave the value for the
/// retry. `None` means "fall back to home".
pub fn take(cx: &Cx) -> Option<String> {
    let store = cookie_store::<String, _>(jar(cx), NAME);
    // Unreadable (undecryptable, stale shape) counts as absent; the
    // removal below still clears it.
    let parsed = store.parse().ok().flatten();
    let value = parsed.map(|store| {
        let value = store.get();
        store.remove();
        value
    });
    if value.is_none() {
        cookie_store::<String, _>(jar(cx), NAME).remove();
    }
    value.filter(|location| is_valid(location))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_relative_locations_pass() {
        assert!(is_valid("/"));
        assert!(is_valid("/settings/security"));
        assert!(is_valid("/organizations/acme?tab=teams"));
    }

    #[test]
    fn escapes_from_the_origin_are_refused() {
        // Protocol-relative and backslash variants a browser would
        // read as leaving the origin.
        assert!(!is_valid("//evil.example/"));
        assert!(!is_valid("/\\evil.example/"));
        assert!(!is_valid("https://evil.example/"));
        assert!(!is_valid("javascript:alert(1)"));
        assert!(!is_valid(""));
    }

    #[test]
    fn control_bytes_and_oversize_are_refused() {
        assert!(!is_valid("/a\r\nSet-Cookie: x=y"));
        assert!(!is_valid("/a\0b"));
        assert!(!is_valid(&format!("/{}", "a".repeat(MAX_LEN))));
        assert!(is_valid(&format!("/{}", "a".repeat(MAX_LEN - 1))));
    }
}
