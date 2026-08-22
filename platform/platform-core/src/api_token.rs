//! API tokens (P.7: the integrator transport) — server-side storage
//! and the one-time secret.
//!
//! **Hashed, never encrypted.** A bearer credential has exactly one
//! legitimate reader, the client that holds it; the server only ever
//! needs to *recognize* it. So the row keeps `SHA-256(secret)` and
//! nothing recoverable — the same discipline as browser sessions
//! ([`crate::session`]), for the same reason: a database dump (or a
//! leaked encryption key) yields no usable credential. The secret
//! exists in plaintext exactly once, in the [`IssuedToken`] that
//! [`create_api_token`] returns, and the caller shows it then or
//! never. What the list can show afterwards is the non-secret
//! [`ApiToken::prefix`].
//!
//! Secret format: `varve_` + 43 URL-safe base64 characters (32 random
//! bytes from the OS). The fixed prefix makes a leaked token
//! recognizable to secret scanners and `grep` alike, at no cost to
//! entropy. Presented tokens are resolved by hashing and a unique
//! index lookup ([`find_live_api_token`]) — no secret is ever
//! compared in application code, so there is no timing surface.
//!
//! Time is an argument, as in [`crate::session`]: `now` comes from the
//! caller, expiry is [`api_token_lifetime`] from creation and is
//! enforced in the query predicate, so an expired row can never
//! authenticate whether or not [`sweep_expired_api_tokens`] has run.

use base64::Engine;
use jiff::{Span, Timestamp, tz::TimeZone};
use sha2::{Digest, Sha256};
use toasty::Deferred;

use crate::account::Account;

/// How long a token lives after creation, in calendar months
/// (computed in UTC by [`api_token_lifetime`]; P.7 has not yet fixed
/// lifetimes, so this is the one provisional platform decision, held
/// here).
pub const API_TOKEN_LIFETIME_MONTHS: i32 = 6;

/// [`API_TOKEN_LIFETIME_MONTHS`] as a [`Span`] (`Span` has no const
/// constructor).
pub fn api_token_lifetime() -> Span {
    Span::new().months(API_TOKEN_LIFETIME_MONTHS)
}

/// The fixed, recognizable head of every secret (module docs).
pub const SECRET_PREFIX: &str = "varve_";

/// Random bytes per secret, before encoding.
const SECRET_BYTES: usize = 32;

/// Characters of the secret kept as the displayable
/// [`ApiToken::prefix`]: `varve_` plus six — enough to tell tokens
/// apart in a list, far too little to guess the rest.
const DISPLAY_PREFIX_CHARS: usize = SECRET_PREFIX.len() + 6;

/// The most characters a token name may have; longer names are
/// rejected by [`create_api_token`] with
/// [`CreateApiTokenError::NameTooLong`] rather than truncated (a name
/// is user-chosen, unlike a user agent).
pub const MAX_API_TOKEN_NAME_CHARS: usize = 100;

/// One issued API token: a secret's hash bound to an account with a
/// name and an expiry.
#[derive(Debug, toasty::Model)]
pub struct ApiToken {
    /// UUID v7 (time-ordered), generated on insert.
    #[key]
    #[auto]
    pub id: uuid::Uuid,

    /// Lowercase-hex SHA-256 of the full secret string. Unique: one
    /// row per issued secret, and the authentication lookup key.
    #[unique]
    pub token_hash: String,

    /// The account the token authenticates as. Indexed for the
    /// account's token list.
    #[index]
    pub account_id: uuid::Uuid,

    /// The account the token authenticates as (relation).
    #[belongs_to]
    pub account: Deferred<Account>,

    /// The user-chosen name, trimmed, non-empty, at most
    /// [`MAX_API_TOKEN_NAME_CHARS`] characters. Display only.
    pub name: String,

    /// The first few (`DISPLAY_PREFIX_CHARS`) characters of the secret
    /// (`varve_ab12cd`) — the non-secret handle a token list shows.
    pub prefix: String,

    /// When the token was created — the caller's `now` (the same
    /// instant `expires_at` derives from).
    pub created_at: Timestamp,

    /// The token is live strictly before this instant.
    pub expires_at: Timestamp,
}

/// A freshly created token together with its secret — the only time
/// the secret exists in plaintext on the server side (module docs).
#[derive(Debug)]
pub struct IssuedToken {
    /// The stored row.
    pub token: ApiToken,
    /// The bearer secret, to show the user once. Never log it.
    pub secret: String,
}

/// Failure modes of [`create_api_token`].
#[derive(Debug, thiserror::Error)]
pub enum CreateApiTokenError {
    /// The trimmed name is empty.
    #[error("the token name must not be empty")]
    EmptyName,
    /// The trimmed name exceeds [`MAX_API_TOKEN_NAME_CHARS`].
    #[error("the token name must be at most {MAX_API_TOKEN_NAME_CHARS} characters")]
    NameTooLong,
    /// The OS random source failed — never silently a weak secret.
    #[error("random source unavailable: {0}")]
    Random(getrandom::Error),
    /// The underlying store failed.
    #[error("database error: {0}")]
    Db(#[from] toasty::Error),
}

/// Mints a new secret from the OS random source.
fn generate_secret() -> Result<String, getrandom::Error> {
    let mut bytes = [0u8; SECRET_BYTES];
    getrandom::fill(&mut bytes)?;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    Ok(format!("{SECRET_PREFIX}{encoded}"))
}

/// The stored form of a secret: lowercase-hex SHA-256 of the whole
/// string, prefix included.
pub fn hash_secret(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    out
}

/// The displayable head of a secret.
fn display_prefix(secret: &str) -> String {
    secret.chars().take(DISPLAY_PREFIX_CHARS).collect()
}

/// `now` plus [`api_token_lifetime`] in UTC, saturating at the
/// timestamp range edge (unreachable for any real `now`).
fn expiry_from(now: Timestamp) -> Timestamp {
    now.to_zoned(TimeZone::UTC)
        .saturating_add(api_token_lifetime())
        .timestamp()
}

/// Creates a token for `account_id`, named `name` (trimmed), live
/// from `now` for [`api_token_lifetime`]. Returns the row and the
/// secret; the secret is shown once and gone.
pub async fn create_api_token(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
    name: &str,
    now: Timestamp,
) -> Result<IssuedToken, CreateApiTokenError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CreateApiTokenError::EmptyName);
    }
    if name.chars().count() > MAX_API_TOKEN_NAME_CHARS {
        return Err(CreateApiTokenError::NameTooLong);
    }
    let secret = generate_secret().map_err(CreateApiTokenError::Random)?;
    let token = ApiToken::create()
        .account_id(account_id)
        .token_hash(hash_secret(&secret))
        .name(name)
        .prefix(display_prefix(&secret))
        .created_at(now)
        .expires_at(expiry_from(now))
        .exec(db)
        .await?;
    Ok(IssuedToken { token, secret })
}

/// Resolves a presented secret to its live token, or `None` for an
/// unknown or expired one (expiry is in the predicate, so a stale row
/// never authenticates). The caller loads the account and builds the
/// [`crate::Principal`] — this is the API transport's counterpart of
/// [`crate::find_live_session`].
pub async fn find_live_api_token(
    db: &mut toasty::Db,
    secret: &str,
    now: Timestamp,
) -> toasty::Result<Option<ApiToken>> {
    ApiToken::filter_by_token_hash(hash_secret(secret))
        .filter(ApiToken::fields().expires_at().gt(now))
        .first()
        .exec(db)
        .await
}

/// The live tokens of an account, newest first (`created_at`
/// descending, id — UUID v7 — as tie-breaker).
pub async fn list_live_api_tokens(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
    now: Timestamp,
) -> toasty::Result<Vec<ApiToken>> {
    ApiToken::filter_by_account_id(account_id)
        .filter(ApiToken::fields().expires_at().gt(now))
        .order_by(ApiToken::fields().created_at().desc())
        .order_by(ApiToken::fields().id().desc())
        .exec(db)
        .await
}

/// Revokes one token of `account_id` by id, returning whether a row
/// was deleted. **The authorization boundary for revocation**, shaped
/// exactly like [`crate::destroy_session`]: the account id is part
/// of the delete predicate, so another account's token id deletes
/// nothing and answers a quiet `false`. Pass the *authenticated*
/// account's id, never one from the request.
pub async fn destroy_api_token(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
    token_id: uuid::Uuid,
) -> toasty::Result<bool> {
    let scoped = ApiToken::fields()
        .id()
        .eq(token_id)
        .and(ApiToken::fields().account_id().eq(account_id));
    let found = ApiToken::filter(scoped.clone()).first().exec(db).await?;
    if found.is_none() {
        return Ok(false);
    }
    ApiToken::filter(scoped).delete().exec(db).await?;
    Ok(true)
}

/// Deletes every token expired at `now`. Housekeeping only (P.13);
/// [`find_live_api_token`] never needs it to have run.
pub async fn sweep_expired_api_tokens(db: &mut toasty::Db, now: Timestamp) -> toasty::Result<()> {
    ApiToken::filter(ApiToken::fields().expires_at().le(now))
        .delete()
        .exec(db)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_prefixed_distinct_and_url_safe() {
        let a = generate_secret().unwrap();
        let b = generate_secret().unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(SECRET_PREFIX));
        assert_eq!(a.len(), SECRET_PREFIX.len() + 43);
        assert!(
            a[SECRET_PREFIX.len()..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{a}"
        );
    }

    #[test]
    fn hash_is_sha256_hex_of_the_whole_secret() {
        // SHA-256("varve_") — prefix included, so a hash can never be
        // confused with one over the bare random part.
        assert_eq!(
            hash_secret("varve_"),
            "1ca275e046c4567a78c2f98d6914ae7858abaf4cbeb961b3224037477e595b8b"
        );
        assert_eq!(hash_secret("x").len(), 64);
    }

    #[test]
    fn display_prefix_is_the_recognizable_head() {
        assert_eq!(display_prefix("varve_abcdefGHIJKL"), "varve_abcdef");
    }

    #[test]
    fn expiry_is_six_calendar_months_utc() {
        let now: Timestamp = "2026-08-31T12:00:00Z".parse().unwrap();
        // August 31 + 6 months clamps to the last day of February.
        assert_eq!(
            expiry_from(now),
            "2027-02-28T12:00:00Z".parse::<Timestamp>().unwrap()
        );
    }
}
