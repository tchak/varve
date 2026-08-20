//! Server-side session storage (P.7 browser sessions).
//!
//! This module is deliberately shaped for `topcoat-session`'s
//! token/hash split without depending on topcoat: the client holds a
//! random token, the framework hands the app a SHA-256 **hash** of
//! it, and only that hash ever reaches storage — a database dump
//! reveals no usable credentials. `platform-app` adapts its
//! `TokenHash` to the `token_hash` strings stored here (any stable
//! encoding of the digest, e.g. lowercase hex; this crate treats it
//! as opaque). Raw tokens must never be passed to these functions.
//!
//! Time is an argument everywhere: `now` and the TTL come from the
//! caller, and expiry is enforced by comparison in the query
//! ([`find_live_session`] treats expired rows as absent) rather than
//! by a clock read inside this crate — the same "timestamps are
//! inputs" discipline the kernel tiers follow (DESIGN §7), which
//! also makes expiry directly testable. Expired rows are garbage,
//! not state: [`sweep_expired`] deletes them (a `platform-jobs`
//! sweep eventually, P.13), but correctness never depends on the
//! sweep having run.

use jiff::{SignedDuration, Timestamp};

/// Provisional default session lifetime: 14 days.
///
/// A platform decision pending P.7 detail (P.7 fixes the principal
/// model but not yet lifetimes, sliding expiration, or rotation
/// policy). Callers pass the TTL explicitly; this constant is the
/// suggested value, not a hidden default.
pub const DEFAULT_SESSION_TTL: SignedDuration = SignedDuration::from_hours(14 * 24);

/// One live browser session: a token hash bound to an account with
/// an expiry.
#[derive(Debug, toasty::Model)]
pub struct Session {
    /// UUID v7 (time-ordered), generated on insert.
    #[key]
    #[auto]
    pub id: uuid::Uuid,

    /// Hash of the client-held token (module docs). Unique: one row
    /// per issued token, and the login-path lookup key.
    #[unique]
    pub token_hash: String,

    /// The account this session authenticates. Indexed for
    /// [`delete_account_sessions`] ("sign out everywhere").
    #[index]
    pub account_id: uuid::Uuid,

    /// When the session was created — the caller's `now`, stored
    /// explicitly rather than `#[auto]` so a single clock reading
    /// dates the whole row (`expires_at` derives from the same
    /// instant).
    pub created_at: Timestamp,

    /// The session is live strictly before this instant.
    pub expires_at: Timestamp,
}

/// Records a new session for `account_id`: live from `now` for
/// `ttl`.
///
/// `token_hash` must already be a hash — see the module docs. The
/// expiry saturates at the timestamp range edge rather than failing,
/// which for any sane TTL is unreachable.
pub async fn create_session(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
    token_hash: &str,
    now: Timestamp,
    ttl: SignedDuration,
) -> toasty::Result<Session> {
    // Infallible for a `SignedDuration` argument: `saturating_add`
    // only errors for calendar-unit `Span`s, and saturation (not
    // overflow) handles the timestamp range edge.
    let expires_at = now
        .saturating_add(ttl)
        .expect("SignedDuration arithmetic saturates instead of failing");
    Session::create()
        .account_id(account_id)
        .token_hash(token_hash)
        .created_at(now)
        .expires_at(expires_at)
        .exec(db)
        .await
}

/// Looks up a live session by token hash. Expired sessions are
/// treated as absent — expiry is part of the query predicate
/// (`expires_at > now`), so a stale row that [`sweep_expired`] has
/// not yet collected can never authenticate.
pub async fn find_live_session(
    db: &mut toasty::Db,
    token_hash: &str,
    now: Timestamp,
) -> toasty::Result<Option<Session>> {
    Session::filter_by_token_hash(token_hash)
        .filter(Session::fields().expires_at().gt(now))
        .first()
        .exec(db)
        .await
}

/// Deletes the session with this token hash (logout). Deleting an
/// absent session is a no-op, not an error — logout must be
/// idempotent.
pub async fn delete_session(db: &mut toasty::Db, token_hash: &str) -> toasty::Result<()> {
    Session::filter_by_token_hash(token_hash)
        .delete()
        .exec(db)
        .await
}

/// Deletes every session of an account ("sign out everywhere" —
/// P.7; also the hook for password change, where `platform-app`
/// composes this with re-issuing the current session).
pub async fn delete_account_sessions(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
) -> toasty::Result<()> {
    Session::filter_by_account_id(account_id)
        .delete()
        .exec(db)
        .await
}

/// Deletes every session expired at `now`. Housekeeping only —
/// [`find_live_session`] already refuses expired rows — destined for
/// a `platform-jobs` sweep (P.13). Toasty's delete terminal returns
/// no affected-row count, so neither does this.
pub async fn sweep_expired(db: &mut toasty::Db, now: Timestamp) -> toasty::Result<()> {
    Session::filter(Session::fields().expires_at().le(now))
        .delete()
        .exec(db)
        .await
}
