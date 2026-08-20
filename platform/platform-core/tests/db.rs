//! DB-backed integration tests, gated on `VARVE_TEST_DATABASE_URL`
//! (the settled P.3 convention: `cargo test --workspace` stays green
//! without Postgres; CI provides a service container). Run for real
//! with e.g.:
//!
//! ```text
//! VARVE_TEST_DATABASE_URL=postgres://localhost/varve_platform_test \
//!   cargo test -p platform-core
//! ```
//!
//! Tests share one database and run in parallel, so every test mints
//! unique emails/token hashes and never asserts on global counts.

use jiff::{SignedDuration, Timestamp};
use platform_core::{
    DEFAULT_SESSION_TTL, RegisterError, connect, create_session, delete_account_sessions,
    delete_session, find_live_session, register, sweep_expired, verify_credentials,
};

/// Connects to the test database, applying migrations; `None` (after
/// printing why) when `VARVE_TEST_DATABASE_URL` is unset so the test
/// passes vacuously.
///
/// Connects one test at a time: `connect` applies pending migrations,
/// and concurrent application is unguarded in toasty 0.10 (see the
/// `platform_core::db` docs) — on a fresh database, parallel tests
/// would race creating `__toasty_migrations`.
async fn test_db() -> Option<toasty::Db> {
    static CONNECT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    let url = match std::env::var("VARVE_TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("skipped: VARVE_TEST_DATABASE_URL not set");
            return None;
        }
    };
    let _guard = CONNECT_LOCK.lock().await;
    Some(connect(&url).await.expect("connect to test database"))
}

fn unique_email(tag: &str) -> String {
    format!("{tag}+{}@example.test", uuid::Uuid::new_v4())
}

fn unique_hash(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn register_then_verify() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let email = unique_email("verify");

    let account = register(&mut db, &email, "s3cret", "Alice")
        .await
        .expect("register");
    assert_eq!(account.email, email);
    assert_eq!(account.name, "Alice");
    assert!(account.password_hash.starts_with("$argon2id$"));
    assert_eq!(account.locale, None);

    // Right password — and a case/whitespace-variant email must
    // normalize to the same account.
    let found = verify_credentials(&mut db, &format!("  {}  ", email.to_uppercase()), "s3cret")
        .await
        .expect("verify");
    assert_eq!(found.map(|a| a.id), Some(account.id));

    // Wrong password and unknown email are the same `None`.
    assert!(
        verify_credentials(&mut db, &email, "wrong")
            .await
            .expect("verify")
            .is_none()
    );
    assert!(
        verify_credentials(&mut db, &unique_email("ghost"), "s3cret")
            .await
            .expect("verify")
            .is_none()
    );
}

#[tokio::test]
async fn duplicate_email_is_a_typed_error() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let email = unique_email("dup");

    register(&mut db, &email, "first", "First")
        .await
        .expect("register");
    // Same email modulo normalization: still taken.
    let err = register(&mut db, &email.to_uppercase(), "second", "Second")
        .await
        .expect_err("duplicate must fail");
    assert!(matches!(err, RegisterError::EmailTaken), "got: {err:?}");
}

#[tokio::test]
async fn session_lifecycle() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let email = unique_email("session");
    let account = register(&mut db, &email, "pw", "Sess")
        .await
        .expect("register");

    let now = Timestamp::now();
    let hash = unique_hash("lifecycle");
    let session = create_session(&mut db, account.id, &hash, now, DEFAULT_SESSION_TTL)
        .await
        .expect("create_session");
    assert_eq!(session.account_id, account.id);
    assert_eq!(session.created_at, now);
    assert_eq!(session.expires_at, now + DEFAULT_SESSION_TTL);

    // Live at `now`, live just before expiry, absent at expiry
    // (strict comparison) and after.
    for (probe, live) in [
        (now, true),
        (
            now + DEFAULT_SESSION_TTL - SignedDuration::from_secs(1),
            true,
        ),
        (now + DEFAULT_SESSION_TTL, false),
        (
            now + DEFAULT_SESSION_TTL + SignedDuration::from_secs(1),
            false,
        ),
    ] {
        let found = find_live_session(&mut db, &hash, probe)
            .await
            .expect("find");
        assert_eq!(found.is_some(), live, "probe at {probe}");
    }

    delete_session(&mut db, &hash).await.expect("delete");
    assert!(
        find_live_session(&mut db, &hash, now)
            .await
            .expect("find")
            .is_none()
    );
    // Idempotent.
    delete_session(&mut db, &hash).await.expect("delete twice");
}

#[tokio::test]
async fn sign_out_everywhere() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let account = register(&mut db, &unique_email("everywhere"), "pw", "Multi")
        .await
        .expect("register");

    let now = Timestamp::now();
    let hashes: Vec<String> = (0..3).map(|i| unique_hash(&format!("multi{i}"))).collect();
    for hash in &hashes {
        create_session(&mut db, account.id, hash, now, DEFAULT_SESSION_TTL)
            .await
            .expect("create");
    }

    delete_account_sessions(&mut db, account.id)
        .await
        .expect("delete all");
    for hash in &hashes {
        assert!(
            find_live_session(&mut db, hash, now)
                .await
                .expect("find")
                .is_none()
        );
    }
}

#[tokio::test]
async fn sweep_collects_only_expired() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let account = register(&mut db, &unique_email("sweep"), "pw", "Sweep")
        .await
        .expect("register");

    let now = Timestamp::now();
    let expired = unique_hash("expired");
    let live = unique_hash("live");
    create_session(
        &mut db,
        account.id,
        &expired,
        now - SignedDuration::from_hours(2),
        SignedDuration::from_hours(1),
    )
    .await
    .expect("create expired");
    create_session(&mut db, account.id, &live, now, DEFAULT_SESSION_TTL)
        .await
        .expect("create live");

    sweep_expired(&mut db, now).await.expect("sweep");

    // The expired row is gone outright (absent even for a probe
    // instant at which it was live), the live one untouched.
    assert!(
        find_live_session(&mut db, &expired, now - SignedDuration::from_mins(90))
            .await
            .expect("find")
            .is_none()
    );
    assert!(
        find_live_session(&mut db, &live, now)
            .await
            .expect("find")
            .is_some()
    );
}
