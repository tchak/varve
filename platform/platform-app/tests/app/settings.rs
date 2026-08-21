//! Subject: the settings area — the landing redirect, the signed-in
//! gate, the account profile card, and the security tab's session
//! list with per-session revocation (metadata capture, the
//! current-session marker, account scoping, and the legacy-row
//! fallback).

use topcoat::{
    router::{StatusCode, header},
    session::Token,
};

use platform_app::auth::encode_token_hash;

use crate::harness::{body_text, form_body, get, post, session_cookie, test_app, unique_email};

/// Signs a fresh account up like `harness::signup`, but with extra
/// request headers — this subject's way of controlling the metadata
/// (`User-Agent`, `X-Forwarded-For`) captured at sign-in.
async fn signup_with_headers(
    router: &topcoat::router::Router,
    name: &str,
    email: &str,
    headers: &[(&str, &str)],
) -> String {
    let response = router
        .handle(post(
            "/signup",
            headers,
            form_body(&[
                ("name", name),
                ("email", email),
                ("password", "s3cret-enough"),
            ]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    session_cookie(&response).expect("signup sets a session cookie")
}

/// The id of the session row a cookie's token resolves to, straight
/// from platform-core storage.
async fn session_id_of(db: &mut toasty::Db, cookie: &str) -> uuid::Uuid {
    let token_value = cookie.split_once('=').unwrap().1.replace("%3D", "=");
    let token = Token::decode(&token_value).expect("cookie value is an encoded session token");
    platform_core::find_live_session(
        db,
        &encode_token_hash(&token.hash()),
        jiff::Timestamp::now(),
    )
    .await
    .expect("lookup")
    .expect("the cookie's session row exists")
    .id
}

#[tokio::test]
async fn settings_redirects_to_the_account_tab() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = crate::harness::signup(
        &router,
        "Landing",
        &unique_email("settings-landing"),
        "s3cret-enough",
    )
    .await;
    let response = router
        .handle(get("/settings", &[("cookie", &cookie)]))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/settings/account");
}

#[tokio::test]
async fn anonymous_settings_paths_redirect_to_signin() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    for path in ["/settings", "/settings/account", "/settings/security"] {
        let response = router.handle(get(path, &[])).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{path}");
        assert_eq!(response.headers()[header::LOCATION], "/signin", "{path}");
    }
    // The gate guards the state change too, not just the pages.
    let response = router
        .handle(post(
            "/settings/security",
            &[],
            form_body(&[("session_id", &uuid::Uuid::new_v4().to_string())]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signin");
}

#[tokio::test]
async fn account_tab_shows_the_profile_card() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("settings-profile");
    let cookie =
        crate::harness::signup(&router, "Profil Utilisateur", &email, "s3cret-enough").await;
    let response = router
        .handle(get("/settings/account", &[("cookie", &cookie)]))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("User profile"), "{html}");
    assert!(html.contains("Profil Utilisateur"), "{html}");
    assert!(html.contains(&email), "{html}");
    // The tab navigation marks the active tab.
    assert!(html.contains(r#"aria-current="page""#), "{html}");
}

#[tokio::test]
async fn security_tab_lists_sessions_with_captured_metadata() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("settings-metadata");
    let cookie = signup_with_headers(
        &router,
        "Meta",
        &email,
        &[
            ("user-agent", "SettingsTest/1.0 (router-level)"),
            ("x-forwarded-for", "203.0.113.9, 198.51.100.4"),
        ],
    )
    .await;

    let response = router
        .handle(get("/settings/security", &[("cookie", &cookie)]))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("Active sessions"), "{html}");
    // The metadata captured at sign-in: the user agent verbatim, and
    // only the *first* X-Forwarded-For value as the client IP.
    assert!(html.contains("SettingsTest/1.0 (router-level)"), "{html}");
    assert!(html.contains("203.0.113.9"), "{html}");
    assert!(!html.contains("198.51.100.4"), "{html}");
    // This request's session is the marked one.
    assert!(html.contains(r#"data-current="true""#), "{html}");
    assert!(html.contains("Current session"), "{html}");
    assert!(!html.contains(r#"data-current="false""#), "{html}");
}

#[tokio::test]
async fn revoking_another_session_removes_it() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let email = unique_email("settings-revoke-other");
    let first_cookie = crate::harness::signup(&router, "Revoker", &email, "s3cret-enough").await;

    // A second sign-in: another live session for the same account.
    let response = router
        .handle(post(
            "/signin",
            &[],
            form_body(&[("email", &email), ("password", "s3cret-enough")]),
        ))
        .await;
    let second_cookie = session_cookie(&response).expect("signin sets a session cookie");
    let second_id = session_id_of(&mut db, &second_cookie).await;

    let html = body_text(
        router
            .handle(get("/settings/security", &[("cookie", &first_cookie)]))
            .await,
    )
    .await;
    assert_eq!(html.matches("data-current=").count(), 2, "{html}");
    assert!(html.contains(r#"data-current="false""#), "{html}");

    // Revoke the other session: 303 back to the security tab, the
    // row is gone, and the revoked cookie no longer authenticates.
    let response = router
        .handle(post(
            "/settings/security",
            &[("cookie", &first_cookie)],
            form_body(&[("session_id", &second_id.to_string())]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/settings/security");

    let html = body_text(
        router
            .handle(get("/settings/security", &[("cookie", &first_cookie)]))
            .await,
    )
    .await;
    assert_eq!(html.matches("data-current=").count(), 1, "{html}");
    let response = router
        .handle(get("/settings/security", &[("cookie", &second_cookie)]))
        .await;
    assert_eq!(response.headers()[header::LOCATION], "/signin");
}

#[tokio::test]
async fn revoking_the_current_session_signs_out() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let email = unique_email("settings-revoke-current");
    let cookie = crate::harness::signup(&router, "Self", &email, "s3cret-enough").await;
    let current_id = session_id_of(&mut db, &cookie).await;

    let response = router
        .handle(post(
            "/settings/security",
            &[("cookie", &cookie)],
            form_body(&[("session_id", &current_id.to_string())]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/");

    // The cookie is dead: the follow-up request is anonymous.
    let html = body_text(router.handle(get("/", &[("cookie", &cookie)])).await).await;
    assert!(html.contains("Please sign in to continue."), "{html}");
}

#[tokio::test]
async fn revocation_is_scoped_to_the_account() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let victim_cookie = crate::harness::signup(
        &router,
        "Victim",
        &unique_email("settings-scope-victim"),
        "s3cret-enough",
    )
    .await;
    let victim_id = session_id_of(&mut db, &victim_cookie).await;
    let attacker_cookie = crate::harness::signup(
        &router,
        "Attacker",
        &unique_email("settings-scope-attacker"),
        "s3cret-enough",
    )
    .await;

    // Account B presents account A's session id: same 303 as an
    // already-revoked id (no oracle), and the row survives — the
    // scoped destroy is the authorization boundary.
    let response = router
        .handle(post(
            "/settings/security",
            &[("cookie", &attacker_cookie)],
            form_body(&[("session_id", &victim_id.to_string())]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/settings/security");

    // The victim's session still authenticates and still lists.
    let html = body_text(
        router
            .handle(get("/settings/security", &[("cookie", &victim_cookie)]))
            .await,
    )
    .await;
    assert!(html.contains(r#"data-current="true""#), "{html}");
}

#[tokio::test]
async fn sessions_without_metadata_render_the_unknown_fallback() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let email = unique_email("settings-legacy");
    let cookie = signup_with_headers(
        &router,
        "Legacy",
        &email,
        &[("user-agent", "ModernBrowser/2.0")],
    )
    .await;

    // A pre-migration-shaped row: no user agent, no IP — inserted
    // straight through platform-core with `None` metadata.
    let account = platform_core::Account::filter_by_email(&email)
        .first()
        .exec(&mut db)
        .await
        .expect("lookup account")
        .expect("the account exists");
    platform_core::create_session(
        &mut db,
        account.id,
        &format!("legacy-{}", uuid::Uuid::new_v4()),
        jiff::Timestamp::now(),
        platform_core::DEFAULT_SESSION_TTL,
        None,
        None,
    )
    .await
    .expect("insert a legacy session");

    let html = body_text(
        router
            .handle(get("/settings/security", &[("cookie", &cookie)]))
            .await,
    )
    .await;
    assert_eq!(html.matches("data-current=").count(), 2, "{html}");
    // The legacy row renders with the localized fallback; the modern
    // row still shows its captured agent.
    assert!(html.contains("Unknown"), "{html}");
    assert!(html.contains("ModernBrowser/2.0"), "{html}");
}
