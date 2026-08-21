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

    /// The `User-Agent` presented at sign-in, truncated to
    /// [`MAX_USER_AGENT_CHARS`] characters on the way in. `None` for
    /// sessions created before this column existed, or by clients
    /// sending no user agent. Display metadata only — never an
    /// authentication input.
    pub user_agent: Option<String>,

    /// The client IP observed at sign-in, best-effort (the caller
    /// decides what "client IP" means — e.g. the first
    /// `X-Forwarded-For` value behind a proxy). `None` when nothing
    /// trustworthy-enough was available. Display metadata only.
    pub ip: Option<String>,
}

/// The most characters of a presented `User-Agent` that
/// [`create_session`] stores; anything longer is truncated (on a
/// character boundary), never rejected — the header is untrusted
/// display metadata, not a protocol field.
pub const MAX_USER_AGENT_CHARS: usize = 512;

/// Truncates an untrusted user-agent string to
/// [`MAX_USER_AGENT_CHARS`] characters.
fn truncate_user_agent(user_agent: &str) -> String {
    user_agent.chars().take(MAX_USER_AGENT_CHARS).collect()
}

/// Records a new session for `account_id`: live from `now` for
/// `ttl`.
///
/// `token_hash` must already be a hash — see the module docs. The
/// expiry saturates at the timestamp range edge rather than failing,
/// which for any sane TTL is unreachable.
///
/// `user_agent` and `ip` are optional display metadata captured at
/// sign-in (shown on the account's session list); the user agent is
/// truncated to [`MAX_USER_AGENT_CHARS`] characters, since the header
/// is client-controlled and unbounded.
pub async fn create_session(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
    token_hash: &str,
    now: Timestamp,
    ttl: SignedDuration,
    user_agent: Option<&str>,
    ip: Option<&str>,
) -> toasty::Result<Session> {
    // Infallible for a `SignedDuration` argument: `saturating_add`
    // only errors for calendar-unit `Span`s, and saturation (not
    // overflow) handles the timestamp range edge.
    let expires_at = now
        .saturating_add(ttl)
        .expect("SignedDuration arithmetic saturates instead of failing");
    let mut builder = Session::create()
        .account_id(account_id)
        .token_hash(token_hash)
        .created_at(now)
        .expires_at(expires_at);
    if let Some(user_agent) = user_agent {
        builder = builder.user_agent(truncate_user_agent(user_agent));
    }
    if let Some(ip) = ip {
        builder = builder.ip(ip);
    }
    builder.exec(db).await
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

/// Lists the live sessions of an account, newest first (`created_at`
/// descending, id — UUID v7, time-ordered — as tie-breaker). Expired
/// rows are excluded by the same strict predicate as
/// [`find_live_session`]; a not-yet-swept stale row never shows up.
pub async fn list_live_sessions(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
    now: Timestamp,
) -> toasty::Result<Vec<Session>> {
    Session::filter_by_account_id(account_id)
        .filter(Session::fields().expires_at().gt(now))
        .order_by(Session::fields().created_at().desc())
        .order_by(Session::fields().id().desc())
        .exec(db)
        .await
}

/// Deletes one session of `account_id` by id, returning whether a
/// row was deleted.
///
/// **This is the authorization boundary for session revocation**: the
/// account id is part of the delete predicate, not a hint — a caller
/// acting for one account can never destroy another account's
/// session, whatever `session_id` it presents (the mismatch is a
/// quiet `false`, indistinguishable from an already-deleted session).
/// Callers must pass the *authenticated* account's id, never one
/// taken from the request.
pub async fn destroy_session(
    db: &mut toasty::Db,
    account_id: uuid::Uuid,
    session_id: uuid::Uuid,
) -> toasty::Result<bool> {
    // Toasty's delete terminal reports no affected-row count, so the
    // scoped read supplies the bool; the delete itself repeats the
    // full scoped predicate rather than trusting the read (no
    // decision rides on the gap between the two statements).
    let scoped = Session::fields()
        .id()
        .eq(session_id)
        .and(Session::fields().account_id().eq(account_id));
    let found = Session::filter(scoped.clone()).first().exec(db).await?;
    if found.is_none() {
        return Ok(false);
    }
    Session::filter(scoped).delete().exec(db).await?;
    Ok(true)
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
