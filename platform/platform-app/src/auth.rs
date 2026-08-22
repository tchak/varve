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
//! Principal resolution is a set of request functions, per topcoat's
//! "functions, not middlewares" idiom: [`account`] resolves the
//! presented token to a live session row and loads the [`Account`],
//! [`principal`] derives the [`Principal`] from it, and
//! [`require_account`] fails closed with an `UnauthorizedError`.
//! Each is `#[memoize]`d, so however many layouts, pages, and
//! components ask during one request, the session lookup runs at
//! most once — and a request nothing asks on (a public page) never
//! touches session storage at all. Nothing is resolved up front and
//! nothing is stashed in `Cx`; a handler that needs the principal
//! asks for it where it needs it.
//!
//! API tokens are the second transport (P.7): [`bearer_principal`]
//! resolves `Authorization: Bearer` through
//! `platform-core::api_token` to the same [`Principal`], and is asked
//! by the `/graphql` route alone — see that module for why the two
//! transports never substitute for each other.
//!
//! Cross-origin protection is the router's default
//! [`topcoat::router::OriginPolicy`] (403 on state-changing
//! cross-origin browser requests), not anything session-specific;
//! this module only relies on every state change being a POST.

use std::sync::Arc;

use platform_core::{Account, DEFAULT_SESSION_TTL, Principal};
use topcoat::{
    context::{Cx, memoize},
    router::{error::RouterErrorExt, header, request},
    session::{self, SessionConfig, TokenHash},
};

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

/// A session-resolution failure as the memoized functions cache it.
/// `#[memoize]` hands every caller in the request the same stored
/// outcome, so the underlying error lives behind an `Arc` and each
/// caller receives a fresh `topcoat::Error` wrapping it (the
/// original stays reachable as the `source`).
#[derive(Debug, Clone)]
struct ResolutionError(Arc<topcoat::Error>);

impl std::fmt::Display for ResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&*self.0, f)
    }
}

impl std::error::Error for ResolutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        let inner: &(dyn std::error::Error + Send + Sync + 'static) = (*self.0).as_ref();
        Some(inner)
    }
}

/// Resolves the presented session token to its account: hash lookup
/// via [`platform_core::find_live_session`] (expired rows are absent
/// by construction), then the account row. A session pointing at a
/// deleted account resolves to `None`, not an error.
async fn resolve_account(cx: &Cx) -> topcoat::Result<Option<Account>> {
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

/// [`resolve_account`], once per request.
#[memoize]
async fn load_account(cx: &Cx) -> Result<Option<Account>, ResolutionError> {
    resolve_account(cx)
        .await
        .map_err(|error| ResolutionError(Arc::new(error)))
}

/// The principal derived from [`load_account`], once per request.
#[memoize]
async fn load_principal(cx: &Cx) -> Result<Option<Principal>, ResolutionError> {
    let account = load_account(cx).await.as_ref().map_err(Clone::clone)?;
    Ok(account.as_ref().map(Principal::from_account))
}

/// The authenticated account row of this request, if any. The row
/// was loaded to derive the principal; exposing it lets pages show
/// account data ([`Principal`] deliberately carries only the
/// identity core — e.g. no display name) without a second query.
pub async fn account(cx: &Cx) -> topcoat::Result<Option<&Account>> {
    match load_account(cx).await {
        Ok(account) => Ok(account.as_ref()),
        Err(error) => Err(error.clone().into()),
    }
}

/// The authenticated principal of this request, if any. This is the
/// one question pages ask (P.7: everything resolves to a `Principal`
/// before execution).
pub async fn principal(cx: &Cx) -> topcoat::Result<Option<&Principal>> {
    match load_principal(cx).await {
        Ok(principal) => Ok(principal.as_ref()),
        Err(error) => Err(error.clone().into()),
    }
}

/// The API token presented as `Authorization: Bearer <secret>`, when
/// the header is present, well-formed, and carries a `varve_`-shaped
/// secret. Anything else — absent, another scheme, empty, not our
/// format — is `None`; the caller's answer to `None` is the same
/// 401 whatever the cause, so the shape check only spares the
/// database a hash lookup for obviously foreign credentials.
fn bearer_secret(cx: &Cx) -> Option<&str> {
    let value = request::headers(cx)
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, secret) = value.trim().split_once(' ')?;
    let secret = secret.trim();
    (scheme.eq_ignore_ascii_case("bearer")
        && secret.starts_with(platform_core::api_token::SECRET_PREFIX))
    .then_some(secret)
}

/// Resolves the presented bearer token to its account:
/// [`platform_core::find_live_api_token`] (hash lookup; expired rows
/// are absent by construction), then the account row. A token whose
/// account is gone resolves to `None`, not an error.
async fn resolve_bearer_account(cx: &Cx) -> topcoat::Result<Option<Account>> {
    let Some(secret) = bearer_secret(cx) else {
        return Ok(None);
    };
    let mut db = crate::db(cx);
    let now = jiff::Timestamp::now();
    let Some(token) = platform_core::find_live_api_token(&mut db, secret, now).await? else {
        return Ok(None);
    };
    Ok(Account::filter_by_id(token.account_id)
        .first()
        .exec(&mut db)
        .await?)
}

/// [`resolve_bearer_account`] → [`Principal`], once per request.
#[memoize]
async fn load_bearer_principal(cx: &Cx) -> Result<Option<Principal>, ResolutionError> {
    resolve_bearer_account(cx)
        .await
        .map(|account| account.as_ref().map(Principal::from_account))
        .map_err(|error| ResolutionError(Arc::new(error)))
}

/// The principal of the request's **bearer token**, if a live one
/// was presented — the API transport's counterpart of [`principal`],
/// deliberately separate from it: the `/graphql` route asks this and
/// only this, so a session cookie never authenticates an API call
/// (no CSRF surface) and a bearer token never authenticates a page.
/// Both produce the same [`Principal`] (P.7); which transport it
/// came from is invisible below the route.
pub async fn bearer_principal(cx: &Cx) -> topcoat::Result<Option<&Principal>> {
    match load_bearer_principal(cx).await {
        Ok(principal) => Ok(principal.as_ref()),
        Err(error) => Err(error.clone().into()),
    }
}

/// The authenticated account row, or topcoat's `UnauthorizedError`
/// (401) when the request carries no live session. The guard for
/// signed-in handlers: calling it is what protects a page — a
/// subtree that wants a friendlier answer than 401 maps the error in
/// its layout (the `/settings` subtree does).
pub async fn require_account(cx: &Cx) -> topcoat::Result<&Account> {
    Ok(account(cx).await?.ok_or_unauthorized()?)
}

/// The request's `User-Agent`, when the client sent a valid-UTF-8
/// one. Untrusted display metadata for the session list;
/// `platform-core` truncates it on storage.
fn request_user_agent(cx: &Cx) -> Option<String> {
    request::headers(cx)
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The client IP, best-effort: the first `X-Forwarded-For` value
/// when a proxy supplied one, else `None`.
///
/// Topcoat 0.6.2 never surfaces the socket peer address to handlers
/// — `internal_serve` accepts `(stream, _remote)` and drops the
/// remote, and nothing puts it in the request extensions — so behind
/// no proxy there is nothing to record. Like the user agent this is
/// display metadata, not an authentication input: an unproxied
/// deployment simply shows the localized "unknown" fallback.
fn request_client_ip(cx: &Cx) -> Option<String> {
    request::headers(cx)
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Logs `account` in: mints a fresh session token (fixation-safe,
/// issued to the client by topcoat) and records its hash in
/// `platform-core`'s session storage, together with the request's
/// user agent and best-effort client IP (`request_client_ip` — the
/// session-list metadata `/settings/security` shows).
/// Call after [`platform_core::verify_credentials`] or a fresh
/// registration — this function performs no credential check itself.
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
        request_user_agent(cx).as_deref(),
        request_client_ip(cx).as_deref(),
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
