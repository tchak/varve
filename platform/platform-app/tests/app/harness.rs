//! Shared machinery for every app subject module: gating, the
//! request/response builders, form encoding, and the cross-subject
//! signup helper. Subject modules import from here and add nothing
//! app-generic of their own — anything two subjects need belongs in
//! this module.

use topcoat::router::{
    Body, Method, Router, StatusCode, header, request::Request, response::Response, to_bytes,
};

/// Connects (applying migrations) and builds the app router; `None`
/// (after printing why) when `VARVE_TEST_DATABASE_URL` is unset so
/// the test passes vacuously. Connects one test at a time — see
/// `platform-core/tests/db.rs` for why (unguarded concurrent
/// migration application in toasty 0.10).
pub async fn test_app() -> Option<(Router, toasty::Db)> {
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
    Some((platform_app::router(db.clone(), None), db))
}

pub fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@example.test", uuid::Uuid::new_v4())
}

/// Builds a request; `headers` are `(name, value)` pairs.
pub fn request(method: Method, path: &str, headers: &[(&str, &str)], body: Body) -> Request {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(body).unwrap()
}

pub fn get(path: &str, headers: &[(&str, &str)]) -> Request {
    request(Method::GET, path, headers, Body::empty())
}

/// A `POST` with an `application/x-www-form-urlencoded` body.
pub fn post(path: &str, headers: &[(&str, &str)], form: String) -> Request {
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

pub fn form_body(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(name, value)| format!("{name}={}", urlencode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

pub async fn body_text(response: Response) -> String {
    let (_parts, body) = response.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// The `name=value` pair of the session cookie a response set.
pub fn session_cookie(response: &Response) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.contains("session="))
        .map(|value| value.split(';').next().unwrap().to_owned())
}

/// Signs a fresh account up through `POST /signup`, returning the
/// session cookie. Asserts the 303-to-home contract.
pub async fn signup(router: &Router, name: &str, email: &str, password: &str) -> String {
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
