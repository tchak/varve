//! Subject: authentication as header-and-markup contracts — the
//! signin page, failure re-renders, the signup/signout round trip,
//! session freshness, duplicate signup, and the cross-origin guard.

use platform_app::auth::encode_token_hash;
use topcoat::{
    router::{StatusCode, header},
    session::Token,
};

use crate::harness::{
    body_text, form_body, get, post, session_cookie, signup, test_app, unique_email,
};

#[tokio::test]
async fn signin_page_renders_the_form() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let response = router.handle(get("/signin", &[])).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains(r#"action="/signin""#), "{html}");
    assert!(html.contains(r#"name="email""#), "{html}");
    assert!(html.contains(r#"name="password""#), "{html}");
}

#[tokio::test]
async fn failed_signin_rerenders_with_one_generic_error() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("wrong-password");
    signup(&router, "Alice", &email, "s3cret-enough").await;

    // Wrong password and unknown email produce the same page — the
    // platform-core `None` collapse carried through to the view.
    let wrong_password = router
        .handle(post(
            "/signin",
            &[],
            form_body(&[("email", &email), ("password", "nope")]),
        ))
        .await;
    assert_eq!(wrong_password.status(), StatusCode::OK);
    let wrong_password = body_text(wrong_password).await;
    assert!(
        wrong_password.contains("Incorrect email address or password."),
        "{wrong_password}"
    );

    let unknown_email = router
        .handle(post(
            "/signin",
            &[],
            form_body(&[("email", &unique_email("ghost")), ("password", "nope")]),
        ))
        .await;
    let unknown_email = body_text(unknown_email).await;
    assert!(
        unknown_email.contains("Incorrect email address or password."),
        "{unknown_email}"
    );
    // No session was started on either path.
    assert!(!wrong_password.contains("Sign out"), "{wrong_password}");
}

#[tokio::test]
async fn signup_signs_in_greets_and_logs_out() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let email = unique_email("round-trip");
    let cookie = signup(&router, "Amélie", &email, "s3cret-enough").await;

    // The cookie carries the raw token; storage holds only the hex
    // hash — verify the adapter's split against platform-core. The
    // jar percent-encodes cookie values ('=' padding becomes %3D), so
    // undo that before decoding.
    let token_value = cookie.split_once('=').unwrap().1.replace("%3D", "=");
    let token = Token::decode(&token_value).expect("cookie value is an encoded session token");
    let hash = encode_token_hash(&token.hash());
    let mut db = db;
    let row = platform_core::find_live_session(&mut db, &hash, jiff::Timestamp::now())
        .await
        .expect("lookup")
        .expect("signup recorded the session hash");

    // Signed-in home greets by name, in the account's request locale.
    let response = router.handle(get("/", &[("cookie", &cookie)])).await;
    let html = body_text(response).await;
    assert!(html.contains("Hello, Amélie."), "{html}");
    assert!(html.contains(&format!("Signed in as {email}")), "{html}");
    assert!(html.contains(r#"action="/signout""#), "{html}");

    // Logout: 303 home, session row deleted, cookie unusable.
    let response = router
        .handle(post("/signout", &[("cookie", &cookie)], String::new()))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/");
    assert!(
        platform_core::find_live_session(&mut db, &hash, jiff::Timestamp::now())
            .await
            .expect("lookup")
            .is_none(),
        "signout deletes the session row"
    );
    let response = router.handle(get("/", &[("cookie", &cookie)])).await;
    let html = body_text(response).await;
    assert!(html.contains("Please sign in to continue."), "{html}");
    let _ = row;
}

#[tokio::test]
async fn signin_starts_a_fresh_session() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("signin");
    let first_cookie = signup(&router, "Benoît", &email, "s3cret-enough").await;

    let response = router
        .handle(post(
            "/signin",
            &[],
            form_body(&[("email", &email), ("password", "s3cret-enough")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/");
    let cookie = session_cookie(&response).expect("signin sets a session cookie");
    // Fixation safety: a fresh token, not the one signup issued.
    assert_ne!(cookie, first_cookie);

    let response = router.handle(get("/", &[("cookie", &cookie)])).await;
    let html = body_text(response).await;
    assert!(html.contains("Hello, Benoît."), "{html}");
}

#[tokio::test]
async fn duplicate_signup_rerenders_with_a_message() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("duplicate");
    signup(&router, "First", &email, "s3cret-enough").await;

    let response = router
        .handle(post(
            "/signup",
            &[],
            form_body(&[
                ("name", "Second"),
                ("email", &email),
                ("password", "other-secret"),
            ]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(
        html.contains("An account with this email address already exists."),
        "{html}"
    );
    // The refused submission started no session.
    assert!(!html.contains("Sign out"), "{html}");
}

#[tokio::test]
async fn cross_origin_post_is_rejected() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let response = router
        .handle(post(
            "/signin",
            &[("sec-fetch-site", "cross-site")],
            form_body(&[("email", "a@b.test"), ("password", "x")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// The stored locale of an account, read straight from platform-core.
async fn stored_locale(db: &mut toasty::Db, email: &str) -> Option<String> {
    platform_core::Account::filter_by_email(email)
        .first()
        .exec(db)
        .await
        .expect("lookup account")
        .expect("the account exists")
        .locale
}

#[tokio::test]
async fn signup_stores_the_resolved_browser_locale() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let email = unique_email("signup-locale-fr");
    let response = router
        .handle(post(
            "/signup",
            &[("accept-language", "fr-FR,fr;q=0.9,en;q=0.5")],
            form_body(&[
                ("name", "Française"),
                ("email", &email),
                ("password", "s3cret-enough"),
            ]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    // The *resolved* locale is stored, not the raw header: fr-FR
    // resolves to the supported base tag.
    assert_eq!(stored_locale(&mut db, &email).await.as_deref(), Some("fr"));

    // And it now pins the UI regardless of the browser language: an
    // English-headed follow-up still renders French.
    let cookie = session_cookie(&response).expect("signup sets a session cookie");
    let html = body_text(
        router
            .handle(get(
                "/settings/account",
                &[("cookie", &cookie), ("accept-language", "en")],
            ))
            .await,
    )
    .await;
    assert!(html.contains(r#"lang="fr""#), "{html}");
    assert!(html.contains("Sécurité"), "{html}");
}

#[tokio::test]
async fn signup_with_an_unsupported_language_stores_the_fallback() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let email = unique_email("signup-locale-de");
    let response = router
        .handle(post(
            "/signup",
            &[("accept-language", "de-DE,de;q=0.9")],
            form_body(&[
                ("name", "Deutsche"),
                ("email", &email),
                ("password", "s3cret-enough"),
            ]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    // resolve_locale already reduced the unsupported language to the
    // English fallback; that resolved value is what lands in storage
    // — never a tag the catalogs cannot serve.
    assert_eq!(stored_locale(&mut db, &email).await.as_deref(), Some("en"));
}
