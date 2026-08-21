//! Router-level tests over the real database, gated on
//! `VARVE_TEST_DATABASE_URL` (the settled P.3 convention, same as
//! `platform-core/tests/db.rs`): `cargo test --workspace` stays
//! green without Postgres. Run for real with e.g.:
//!
//! ```text
//! VARVE_TEST_DATABASE_URL=postgres://localhost/varve_platform_test \
//!   cargo test -p platform-app
//! ```
//!
//! Everything goes through `Router::handle` — no listener, no
//! browser. Tests share one database and run in parallel, so every
//! test mints unique emails and never asserts on global counts.

use platform_app::auth::encode_token_hash;
use topcoat::{
    router::{
        Body, Method, Router, StatusCode, header, request::Request, response::Response, to_bytes,
    },
    session::Token,
};

/// Connects (applying migrations) and builds the app router; `None`
/// (after printing why) when `VARVE_TEST_DATABASE_URL` is unset so
/// the test passes vacuously. Connects one test at a time — see
/// `platform-core/tests/db.rs` for why (unguarded concurrent
/// migration application in toasty 0.10).
async fn test_app() -> Option<(Router, toasty::Db)> {
    static CONNECT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    let url = match std::env::var("VARVE_TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("skipped: VARVE_TEST_DATABASE_URL not set");
            return None;
        }
    };
    let _guard = CONNECT_LOCK.lock().await;
    let db = platform_core::connect(&url).await.expect("connect");
    Some((platform_app::router(db.clone()), db))
}

fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@example.test", uuid::Uuid::new_v4())
}

/// Builds a request; `headers` are `(name, value)` pairs.
fn request(method: Method, path: &str, headers: &[(&str, &str)], body: Body) -> Request {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(body).unwrap()
}

fn get(path: &str, headers: &[(&str, &str)]) -> Request {
    request(Method::GET, path, headers, Body::empty())
}

/// A `POST` with an `application/x-www-form-urlencoded` body.
fn post(path: &str, headers: &[(&str, &str)], form: String) -> Request {
    let mut all = vec![("content-type", "application/x-www-form-urlencoded")];
    all.extend_from_slice(headers);
    request(Method::POST, path, &all, Body::from(form))
}

/// Percent-encodes one form value conservatively (everything but
/// unreserved characters), so emails and passwords survive
/// `application/x-www-form-urlencoded` exactly.
fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn form_body(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(name, value)| format!("{name}={}", urlencode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

async fn body_text(response: Response) -> String {
    let (_parts, body) = response.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// The `name=value` pair of the session cookie a response set.
fn session_cookie(response: &Response) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.contains("session="))
        .map(|value| value.split(';').next().unwrap().to_owned())
}

/// Signs a fresh account up through `POST /signup`, returning
/// `(email, session cookie)`. Asserts the 303-to-home contract.
async fn signup(router: &Router, name: &str, email: &str, password: &str) -> String {
    let response = router
        .handle(post(
            "/signup",
            &[],
            form_body(&[("name", name), ("email", email), ("password", password)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/");
    session_cookie(&response).expect("signup sets a session cookie")
}

#[tokio::test]
async fn home_signed_out_prompts_signin_in_english_by_default() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let response = router.handle(get("/", &[])).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("Please sign in to continue."), "{html}");
    assert!(html.contains("Sign in"), "{html}");
    assert!(html.contains(r#"lang="en""#), "{html}");
}

#[tokio::test]
async fn home_renders_french_from_accept_language() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let response = router
        .handle(get("/", &[("accept-language", "fr-CH, en;q=0.7")]))
        .await;
    let html = body_text(response).await;
    assert!(
        html.contains("Veuillez vous connecter pour continuer."),
        "{html}"
    );
    assert!(html.contains("Se connecter"), "{html}");
    assert!(html.contains(r#"lang="fr""#), "{html}");
}

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

#[tokio::test]
async fn unknown_url_renders_the_branded_not_found() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let response = router.handle(get("/no/such/page", &[])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let html = body_text(response).await;
    // The layout rendered around the 404 (branding, header intact).
    assert!(html.contains("Page not found."), "{html}");
    assert!(html.contains("Varve"), "{html}");
}
