//! Subject: the settings area — the landing redirect, the signed-in
//! gate, the account profile card, and the security tab's session
//! list with per-session revocation (metadata capture and the parsed
//! browser title, the current-session marker, account scoping, and
//! the unknown-browser fallback for junk or absent user agents) and
//! API tokens card (one-time secret, listing, validation, scoped
//! revocation).

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

/// A current, real Chrome-on-macOS user agent — what the parsed row
/// title is asserted against.
const CHROME_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                         AppleWebKit/537.36 (KHTML, like Gecko) \
                         Chrome/129.0.0.0 Safari/537.36";

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
            ("user-agent", CHROME_UA),
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
    // The row title is the *parsed* browser, composed as
    // family + major + OS; the raw stored string survives as the
    // title attribute's tooltip.
    assert!(html.contains("Chrome 129 · Mac OS X"), "{html}");
    assert!(html.contains(&format!(r#"title="{CHROME_UA}""#)), "{html}");
    // The captured IP: only the *first* X-Forwarded-For value.
    assert!(html.contains("203.0.113.9"), "{html}");
    assert!(!html.contains("198.51.100.4"), "{html}");
    // The meta line's dates are medium-style ("Aug 21, 2026"), both
    // of them today's UTC date at signup: created now, and expires
    // rendered as a calendar date too.
    let today = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date();
    let medium = today.strftime("%b %-d, %Y").to_string();
    assert!(html.contains(&format!("Signed in on {medium}")), "{html}");
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
async fn junk_or_absent_user_agents_render_the_unknown_browser_fallback() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let email = unique_email("settings-legacy");
    let cookie = signup_with_headers(&router, "Legacy", &email, &[("user-agent", CHROME_UA)]).await;

    // Two degenerate rows, inserted straight through platform-core:
    // a pre-migration-shaped one (no user agent, no IP) and one whose
    // stored user agent identifies no browser.
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
    platform_core::create_session(
        &mut db,
        account.id,
        &format!("junk-{}", uuid::Uuid::new_v4()),
        jiff::Timestamp::now(),
        platform_core::DEFAULT_SESSION_TTL,
        Some("definitely not a browser"),
        None,
    )
    .await
    .expect("insert a junk-agent session");

    let html = body_text(
        router
            .handle(get("/settings/security", &[("cookie", &cookie)]))
            .await,
    )
    .await;
    assert_eq!(html.matches("data-current=").count(), 3, "{html}");
    // Both degenerate rows render the localized unknown-browser
    // title — the junk one keeping its raw string as the tooltip,
    // the legacy one with no tooltip to offer (and the localized
    // "Unknown" standing in for its missing IP); the real row still
    // shows its parsed browser.
    assert_eq!(html.matches("Unknown browser").count(), 2, "{html}");
    assert!(
        html.contains(r#"title="definitely not a browser""#),
        "{html}"
    );
    assert!(html.contains("Unknown"), "{html}");
    assert!(html.contains("Chrome 129 · Mac OS X"), "{html}");
}

#[tokio::test]
async fn profile_form_shows_the_stored_name_and_selected_locale() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("settings-form");
    let cookie = crate::harness::signup(&router, "Marguerite", &email, "s3cret-enough").await;
    let response = router
        .handle(get("/settings/account", &[("cookie", &cookie)]))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    // The profile card is a form posting back to this module's path,
    // with the stored name as the input's value and the save button.
    assert!(
        html.contains(r#"<form method="post" action="/settings/account""#),
        "{html}"
    );
    assert!(html.contains(r#"value="Marguerite""#), "{html}");
    assert!(html.contains(r#"name="name""#), "{html}");
    assert!(html.contains("Save changes"), "{html}");
    // The language select offers exactly the supported locales as
    // endonyms; the account signed up without an Accept-Language, so
    // its stored preference is the English fallback — selected.
    assert!(html.contains(r#"name="locale""#), "{html}");
    assert!(html.contains("Language"), "{html}");
    assert!(
        html.contains(r#"<option value="en" selected="">English</option>"#),
        "{html}"
    );
    assert!(
        html.contains(r#"<option value="fr">Français</option>"#),
        "{html}"
    );
}

#[tokio::test]
async fn email_card_shows_the_address_as_read_only_prose() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("settings-email-card");
    let cookie = crate::harness::signup(&router, "Lectrice", &email, "s3cret-enough").await;
    let html = body_text(
        router
            .handle(get("/settings/account", &[("cookie", &cookie)]))
            .await,
    )
    .await;
    assert!(html.contains("Email address"), "{html}");
    // Prose, not a control: the address renders in a paragraph and
    // no input carries it.
    assert!(
        html.contains(&format!(r#"<p class="text-sm">{email}</p>"#)),
        "{html}"
    );
    assert!(!html.contains(r#"name="email""#), "{html}");
}

#[tokio::test]
async fn saving_the_profile_redirects_and_renders_in_the_new_locale() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("settings-save");
    let cookie = crate::harness::signup(&router, "Avant", &email, "s3cret-enough").await;

    let response = router
        .handle(post(
            "/settings/account",
            &[("cookie", &cookie)],
            form_body(&[("name", "  Après  "), ("locale", "fr")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/settings/account");

    // The follow-up GET renders in the new locale purely through the
    // existing resolution order (stored preference wins): French tab
    // labels, French lang attribute — and the trimmed name.
    let html = body_text(
        router
            .handle(get("/settings/account", &[("cookie", &cookie)]))
            .await,
    )
    .await;
    assert!(html.contains(r#"lang="fr""#), "{html}");
    assert!(html.contains("Paramètres"), "{html}");
    assert!(html.contains("Sécurité"), "{html}");
    assert!(html.contains(r#"value="Après""#), "{html}");
    assert!(
        html.contains(r#"<option value="fr" selected="">Français</option>"#),
        "{html}"
    );
}

#[tokio::test]
async fn empty_name_rerenders_with_an_error_and_saves_nothing() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("settings-empty-name");
    let cookie = crate::harness::signup(&router, "Intacte", &email, "s3cret-enough").await;

    let response = router
        .handle(post(
            "/settings/account",
            &[("cookie", &cookie)],
            form_body(&[("name", "   "), ("locale", "fr")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    // The localized message sits in the name field's error slot,
    // wired to the input via aria.
    assert!(html.contains("Please enter a name."), "{html}");
    assert!(html.contains(r#"<p id="account-name-error""#), "{html}");
    assert!(
        html.contains(r#"aria-describedby="account-name-error""#),
        "{html}"
    );
    // The submitted locale stays selected on the re-render.
    assert!(
        html.contains(r#"<option value="fr" selected="">Français</option>"#),
        "{html}"
    );

    // Nothing was saved: name and locale are as they were.
    let html = body_text(
        router
            .handle(get("/settings/account", &[("cookie", &cookie)]))
            .await,
    )
    .await;
    assert!(html.contains(r#"value="Intacte""#), "{html}");
    assert!(
        html.contains(r#"<option value="en" selected="">English</option>"#),
        "{html}"
    );
}

#[tokio::test]
async fn forged_locale_is_a_bad_request() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("settings-forged-locale");
    let cookie = crate::harness::signup(&router, "Forgeur", &email, "s3cret-enough").await;

    // The select never offers an unsupported locale, so this value
    // can only come from a forged form — a 400, not a re-render.
    let response = router
        .handle(post(
            "/settings/account",
            &[("cookie", &cookie)],
            form_body(&[("name", "Forgeur"), ("locale", "de")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// The one-shot flash cookie a successful creation sets (the
/// `__Host-` prefixed, encrypted carrier of the secret), as a
/// `name=value` pair to send back.
fn flash_cookie(response: &topcoat::router::response::Response) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("__Host-api-token-issued=") && !value.contains("Max-Age=0"))
        .map(|value| value.split(';').next().unwrap().to_owned())
}

/// Creates a token through the form — asserting the
/// post/redirect/get contract: 303 to the security tab carrying the
/// flash — then follows the redirect with the flash and returns
/// (the landing page, which shows the secret once, and that
/// response's flash removal as a bool).
async fn create_token(
    router: &topcoat::router::Router,
    cookie: &str,
    name: &str,
) -> (String, bool) {
    let response = router
        .handle(post(
            "/settings/security/tokens",
            &[("cookie", cookie)],
            form_body(&[("name", name)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/settings/security");
    let flash = flash_cookie(&response).expect("creation sets the flash cookie");
    // The secret never travels in the clear: the cookie is sealed.
    assert!(!flash.contains("varve_"), "{flash}");
    let response = router
        .handle(get(
            "/settings/security",
            &[("cookie", &format!("{cookie}; {flash}"))],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let removed = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.starts_with("__Host-api-token-issued=") && value.contains("Max-Age=0"));
    (body_text(response).await, removed)
}

/// The secret out of a creation response.
fn secret_in(html: &str) -> String {
    let start = html
        .find(r#"id="api-token-secret""#)
        .expect("the secret code block");
    let rest = &html[start..];
    let open = rest.find('>').unwrap() + 1;
    let close = rest.find("</code>").unwrap();
    rest[open..close].trim().to_owned()
}

#[tokio::test]
async fn api_token_is_created_shown_once_and_listed() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let cookie = crate::harness::signup(
        &router,
        "Tokens",
        &unique_email("settings-tokens"),
        "s3cret-enough",
    )
    .await;

    // Empty state.
    let html = body_text(
        router
            .handle(get("/settings/security", &[("cookie", &cookie)]))
            .await,
    )
    .await;
    assert!(html.contains("API tokens"), "{html}");
    assert!(html.contains("No API tokens."), "{html}");
    assert!(html.contains("6 months"), "{html}");

    // Creation redirects; the landing page shows the secret once, in
    // a status notice, and clears the flash on that same response.
    let (html, flash_removed) = create_token(&router, &cookie, "  CI deploy ").await;
    assert!(flash_removed, "{html}");
    assert!(html.contains(r#"role="status""#), "{html}");
    assert!(html.contains("Token “CI deploy” created"), "{html}");
    assert!(html.contains("it will not be shown again"), "{html}");
    let secret = secret_in(&html);
    assert!(secret.starts_with("varve_"), "{secret}");
    assert_eq!(secret.len(), 49, "{secret}");
    // The same response already lists the token, by name and by its
    // non-secret prefix, with the revoke button named after it.
    assert!(html.contains("data-token-id="), "{html}");
    assert!(html.contains(&format!("{}…", &secret[..12])), "{html}");
    assert!(
        html.contains(r#"aria-label="Revoke token CI deploy""#),
        "{html}"
    );
    let today = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date();
    let medium = today.strftime("%b %-d, %Y").to_string();
    assert!(html.contains(&format!("Created on {medium}")), "{html}");

    // The secret resolves in storage — and the storage holds a hash,
    // never the secret.
    let now = jiff::Timestamp::now();
    let token = platform_core::find_live_api_token(&mut db, &secret, now)
        .await
        .expect("lookup")
        .expect("the shown secret authenticates");
    assert_eq!(token.name, "CI deploy");
    assert_ne!(token.token_hash, secret);
    assert!(
        !token.token_hash.contains(&secret[6..]),
        "hash must not embed the secret"
    );

    // The next GET — a refresh of the landing page — lists the token
    // but never the secret again.
    let html = body_text(
        router
            .handle(get("/settings/security", &[("cookie", &cookie)]))
            .await,
    )
    .await;
    assert!(html.contains("CI deploy"), "{html}");
    assert!(!html.contains(&secret), "{html}");
    assert!(!html.contains("api-token-secret"), "{html}");
    assert!(!html.contains("No API tokens."), "{html}");
}

#[tokio::test]
async fn blank_token_name_rerenders_with_an_error_and_creates_nothing() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let email = unique_email("settings-tokens-blank");
    let cookie = crate::harness::signup(&router, "Blank", &email, "s3cret-enough").await;

    let response = router
        .handle(post(
            "/settings/security/tokens",
            &[("cookie", &cookie)],
            form_body(&[("name", "   ")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(flash_cookie(&response).is_none());
    let html = body_text(response).await;
    assert!(html.contains("Please enter a token name."), "{html}");
    assert!(
        html.contains(r#"aria-describedby="api-token-name-error""#),
        "{html}"
    );
    assert!(!html.contains("api-token-secret"), "{html}");

    let account = platform_core::verify_credentials(&mut db, &email, "s3cret-enough")
        .await
        .expect("verify")
        .expect("account");
    assert!(
        platform_core::list_live_api_tokens(&mut db, account.id, jiff::Timestamp::now())
            .await
            .expect("list")
            .is_empty()
    );

    // Over the length limit: a forged request (the input has
    // `maxlength`), answered 400.
    let response = router
        .handle(post(
            "/settings/security/tokens",
            &[("cookie", &cookie)],
            form_body(&[("name", &"x".repeat(101))]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn token_revocation_is_scoped_to_the_account() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let owner_cookie = crate::harness::signup(
        &router,
        "Owner",
        &unique_email("settings-tokens-owner"),
        "s3cret-enough",
    )
    .await;
    let other_cookie = crate::harness::signup(
        &router,
        "Other",
        &unique_email("settings-tokens-other"),
        "s3cret-enough",
    )
    .await;

    let (html, _) = create_token(&router, &owner_cookie, "Mine").await;
    let secret = secret_in(&html);
    let start = html.find(r#"data-token-id=""#).unwrap() + r#"data-token-id=""#.len();
    let token_id = &html[start..start + 36];

    // Another account's revocation: same 303, nothing deleted.
    let response = router
        .handle(post(
            "/settings/security/tokens/revoke",
            &[("cookie", &other_cookie)],
            form_body(&[("token_id", token_id)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/settings/security");
    assert!(
        platform_core::find_live_api_token(&mut db, &secret, jiff::Timestamp::now())
            .await
            .expect("lookup")
            .is_some()
    );

    // The owner's revocation: 303 back to the tab, the token is gone
    // and no longer authenticates.
    let response = router
        .handle(post(
            "/settings/security/tokens/revoke",
            &[("cookie", &owner_cookie)],
            form_body(&[("token_id", token_id)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/settings/security");
    assert!(
        platform_core::find_live_api_token(&mut db, &secret, jiff::Timestamp::now())
            .await
            .expect("lookup")
            .is_none()
    );
    let html = body_text(
        router
            .handle(get("/settings/security", &[("cookie", &owner_cookie)]))
            .await,
    )
    .await;
    assert!(!html.contains("data-token-id="), "{html}");

    // A junk id is a 400; an anonymous revocation is the signin
    // redirect, like every other settings POST.
    let response = router
        .handle(post(
            "/settings/security/tokens/revoke",
            &[("cookie", &owner_cookie)],
            form_body(&[("token_id", "nope")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = router
        .handle(post(
            "/settings/security/tokens/revoke",
            &[],
            form_body(&[("token_id", token_id)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signin");
}

#[tokio::test]
async fn tampered_flash_shows_nothing() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = crate::harness::signup(
        &router,
        "Flash",
        &unique_email("settings-tokens-flash"),
        "s3cret-enough",
    )
    .await;
    let response = router
        .handle(post(
            "/settings/security/tokens",
            &[("cookie", &cookie)],
            form_body(&[("name", "Once")]),
        ))
        .await;
    let flash = flash_cookie(&response).expect("flash");

    // Tampered ciphertext: authenticated encryption rejects it, the
    // page renders without a secret, and the junk cookie is cleared.
    let mut tampered = flash.clone();
    tampered.replace_range(tampered.len() - 4.., "AAAA");
    let response = router
        .handle(get(
            "/settings/security",
            &[("cookie", &format!("{cookie}; {tampered}"))],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(!html.contains("api-token-secret"), "{html}");

    // The genuine flash still works after the tampered attempt (it
    // was never consumed).
    let response = router
        .handle(get(
            "/settings/security",
            &[("cookie", &format!("{cookie}; {flash}"))],
        ))
        .await;
    let html = body_text(response).await;
    assert!(html.contains("api-token-secret"), "{html}");
}
