//! The session adapter and principal resolution (PLATFORM.md P.7).
//!
//! Topcoat owns the session *mechanics* — token generation, cookie
//! transport, lifecycle — and hands this crate a SHA-256
//! [`TokenHash`] to persist; `platform-core::session` owns the
//! *storage*. The adapter is the pair [`sign_in`] / [`sign_out`]
//! plus [`encode_token_hash`], which fixes the stable encoding
//! (lowercase hex) of the hash that `platform-core` treats as an
//! opaque string. Raw tokens never appear on this side of the seam:
//! everything this module touches is already hashed.
//!
//! Principal resolution is the root request-state layer
//! (`resolve_request_state`): it resolves the presented token to a
//! live session row, loads the [`Account`], and scopes the derived
//! [`Principal`] — together with the resolved request locale
//! ([`crate::i18n`]) — into the request context via [`Cx::with_many`].
//! Pages read the result with [`principal`] / [`account`]; below the
//! layer, authentication is plain data in `Cx`, per the "functions,
//! not middlewares" idiom.
//!
//! Cross-origin protection is the router's default
//! [`topcoat::router::OriginPolicy`] (403 on state-changing
//! cross-origin browser requests), not anything session-specific;
//! this module only relies on every state change being a POST.

use platform_core::{Account, DEFAULT_SESSION_TTL, Principal};
use topcoat::{
    context::{Cx, try_request_context},
    router::{Body, Next, header, layer, request, response::Response},
    session::{self, SessionConfig, TokenHash},
};

use crate::i18n::{self, RequestLocale};

/// The topcoat session lifetime: the same 14 days as
/// [`DEFAULT_SESSION_TTL`], so the cookie's `Max-Age` and the stored
/// row's `expires_at` agree. One provisional platform decision, held
/// in one place (P.7 has not yet fixed lifetimes or sliding
/// expiration).
const SESSION_LIFETIME: std::time::Duration =
    std::time::Duration::from_secs(DEFAULT_SESSION_TTL.as_secs() as u64);

/// The topcoat session configuration for [`crate::router`]: default
/// hardened cookie transport (`__Host-` prefix, `Secure`, `HttpOnly`,
/// `SameSite=Lax`), `SESSION_LIFETIME` (14 days).
pub fn session_config() -> SessionConfig {
    SessionConfig::builder().lifetime(SESSION_LIFETIME).build()
}

/// Encodes a [`TokenHash`] as lowercase hex — the stable, opaque
/// `token_hash: String` that `platform-core::session` stores and
/// looks up by. This function is the *only* place the encoding is
/// chosen; changing it invalidates every stored session.
pub fn encode_token_hash(hash: &TokenHash) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(64);
    for byte in hash.iter() {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

/// The request's resolved principal, scoped into `Cx` by the
/// request-state layer. `None` inside means "no authenticated
/// session"; the wrapper being absent altogether means the layer did
/// not run (a test context, or a request the router never matched).
pub struct CurrentPrincipal(pub Option<Principal>);

/// The account row the principal was derived from, scoped alongside
/// [`CurrentPrincipal`]. The row was already loaded to build the
/// principal; keeping it lets pages show account data ([`Principal`]
/// deliberately carries only the identity core — e.g. no display
/// name) without a second query.
pub struct CurrentAccount(pub Option<Account>);

/// The authenticated principal of this request, if any. This is the
/// one question pages ask (P.7: everything resolves to a `Principal`
/// before execution).
pub fn principal(cx: &Cx) -> Option<&Principal> {
    try_request_context::<CurrentPrincipal>(cx).and_then(|current| current.0.as_ref())
}

/// The authenticated account row of this request, if any.
pub fn account(cx: &Cx) -> Option<&Account> {
    try_request_context::<CurrentAccount>(cx).and_then(|current| current.0.as_ref())
}

/// Resolves the presented session token to its account: hash lookup
/// via [`platform_core::find_live_session`] (expired rows are absent
/// by construction), then the account row. A session pointing at a
/// deleted account resolves to `None`, not an error.
async fn load_account(cx: &Cx) -> topcoat::Result<Option<Account>> {
    let Some(hash) = session::token_hash(cx).await? else {
        return Ok(None);
    };
    let mut db = crate::db(cx);
    let now = jiff::Timestamp::now();
    let Some(session_row) =
        platform_core::find_live_session(&mut db, &encode_token_hash(&hash), now).await?
    else {
        return Ok(None);
    };
    Ok(Account::filter_by_id(session_row.account_id)
        .first()
        .exec(&mut db)
        .await?)
}

/// The root request-state layer: resolves principal and locale once
/// per request and scopes them for everything below. Registered by
/// discovery; [`crate::router`] documents why it must nest inside the
/// cookie and session layers.
#[layer("/")]
async fn resolve_request_state(cx: &Cx, body: Body, next: Next<'_>) -> topcoat::Result<Response> {
    let account = load_account(cx).await?;
    let principal = account.as_ref().map(Principal::from_account);
    let locale = i18n::resolve_locale(
        principal.as_ref().and_then(|p| p.locale.as_deref()),
        request::headers(cx)
            .get(header::ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok()),
    );
    let cx = cx.with_many((
        CurrentPrincipal(principal),
        CurrentAccount(account),
        RequestLocale(locale),
    ));
    next.run(&cx, body).await
}

/// Logs `account` in: mints a fresh session token (fixation-safe,
/// issued to the client by topcoat) and records its hash in
/// `platform-core`'s session storage. Call after
/// [`platform_core::verify_credentials`] or a fresh registration —
/// this function performs no credential check itself.
///
/// If recording fails after the token was issued, the client holds a
/// cookie no storage row backs — indistinguishable from an expired
/// session, so the failure is safe to surface as an error.
pub async fn sign_in(cx: &Cx, account: &Account) -> topcoat::Result<()> {
    let session = session::start(cx).await?;
    let mut db = crate::db(cx);
    platform_core::create_session(
        &mut db,
        account.id,
        &encode_token_hash(&session.token_hash),
        jiff::Timestamp::now(),
        DEFAULT_SESSION_TTL,
    )
    .await?;
    Ok(())
}

/// Logs the current session out: instructs the client to discard its
/// token and deletes the storage row. Idempotent — no presented
/// session, or an already-deleted row, is a no-op.
pub async fn sign_out(cx: &Cx) -> topcoat::Result<()> {
    if let Some(hash) = session::stop(cx).await? {
        let mut db = crate::db(cx);
        platform_core::delete_session(&mut db, &encode_token_hash(&hash)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_encoding_is_stable_lowercase_hex() {
        assert_eq!(
            encode_token_hash(&TokenHash::new([0u8; 32])),
            "0".repeat(64)
        );

        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::try_from(i).unwrap() * 8 + 7;
        }
        let encoded = encode_token_hash(&TokenHash::new(bytes));
        assert_eq!(encoded.len(), 64);
        assert!(encoded.starts_with("070f171f"));
        assert_eq!(encoded, encoded.to_lowercase());
    }

    #[test]
    fn session_lifetime_matches_platform_core_ttl() {
        // The cookie Max-Age and the stored expiry must agree; both
        // derive from the same constant, pinned here.
        assert_eq!(
            i64::try_from(SESSION_LIFETIME.as_secs()).unwrap(),
            DEFAULT_SESSION_TTL.as_secs()
        );
    }
}
