//! One-shot flash values across a redirect, carried in a private
//! cookie.
//!
//! The post/redirect/get pattern needs somewhere to park a value
//! between the POST that produces it and the GET that shows it. This
//! module parks it on the client, in a cookie that is **encrypted and
//! authenticated** (AES-256-GCM via [`private_cookies`] and the
//! app-context [`topcoat::cookie::Key`]): the client can neither read nor forge it,
//! which is what lets a freshly minted API secret ride in it. Every
//! flash is `__Host-`-prefixed, `Secure`, `HttpOnly`, `SameSite=Lax`
//! (the redirect that follows is a same-site navigation), and
//! short-lived (`MAX_AGE`, five minutes) so a flash whose GET never comes does not
//! linger.
//!
//! [`take`] is the whole consumption protocol: read once, then queue
//! the cookie's removal on the same response, so a refresh of the
//! landing page sees nothing — the value was *shown once* in the
//! strongest sense the web allows. A malformed or undecryptable
//! cookie (a key rotation, a stale browser) reads as absent.
//!
//! Limit, stated plainly: one-shot is enforced by the *client*
//! honouring the removal. The server keeps no record of consumed
//! flashes, so a captured `Set-Cookie` value replays within
//! `MAX_AGE` — an attacker in that position also holds the session
//! cookie from the same response, so the flash adds no exposure the
//! session did not already have.

use serde::{Serialize, de::DeserializeOwned};
use topcoat::{
    context::Cx,
    cookie::{Cookies, SameSite, cookie_store, private_cookies, time::Duration},
};

/// How long an unconsumed flash survives on the client.
const MAX_AGE: Duration = Duration::minutes(5);

/// The jar every flash goes through, on write and on read/removal
/// alike — so the attributes a flash was set with are reapplied when
/// it is removed, and the browser matches the deletion.
fn jar(cx: &Cx) -> impl Cookies + '_ {
    private_cookies(cx)
        .default_http_only(true)
        .default_same_site(SameSite::Lax)
        .default_max_age(MAX_AGE)
        .default_prefix_host()
}

/// Queues `value` under `name` for the next request.
pub fn set<T: Serialize + DeserializeOwned>(
    cx: &Cx,
    name: &'static str,
    value: T,
) -> topcoat::Result<()> {
    cookie_store::<T, _>(jar(cx), name).set(value).commit()?;
    Ok(())
}

/// Consumes the flash under `name`: returns it when present and
/// readable, and queues its removal on this response either way.
pub fn take<T: Serialize + DeserializeOwned + Clone>(cx: &Cx, name: &'static str) -> Option<T> {
    let store = cookie_store::<T, _>(jar(cx), name);
    // Unreadable (undecryptable, stale shape) counts as absent; the
    // removal below still clears it.
    let parsed = store.parse().ok().flatten();
    let value = parsed.map(|store| {
        let value = store.get();
        store.remove();
        value
    });
    if value.is_none() {
        cookie_store::<T, _>(jar(cx), name).remove();
    }
    value
}
