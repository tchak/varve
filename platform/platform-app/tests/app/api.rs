//! Subject: the public API transport — `POST /graphql` guarded by
//! bearer API tokens (the 401 contract for absent, foreign, unknown,
//! revoked, and expired tokens; the cookie never authenticating it;
//! a live token executing as its account's principal).

use topcoat::router::{Body, Method, StatusCode, header};

use crate::harness::{body_text, request, test_app, unique_email};

/// A GraphQL POST with an optional `Authorization` header.
fn graphql(
    query: &str,
    authorization: Option<&str>,
    extra: &[(&str, &str)],
) -> topcoat::router::request::Request {
    let body = serde_json::json!({ "query": query }).to_string();
    let mut headers = vec![("content-type", "application/json")];
    if let Some(authorization) = authorization {
        headers.push(("authorization", authorization));
    }
    headers.extend_from_slice(extra);
    request(Method::POST, "/graphql", &headers, Body::from(body))
}

const VIEWER: &str = "{ viewer { accountId email locale } }";

/// Registers an account and mints one API token for it through
/// platform-core, returning (account id, the secret, the session
/// cookie the signup set).
async fn account_with_token(
    router: &topcoat::router::Router,
    db: &mut toasty::Db,
    tag: &str,
) -> (uuid::Uuid, String, String) {
    let email = unique_email(tag);
    let cookie = crate::harness::signup(router, "Api", &email, "s3cret-enough").await;
    let account = platform_core::verify_credentials(db, &email, "s3cret-enough")
        .await
        .expect("verify")
        .expect("account");
    let issued = platform_core::create_api_token(db, account.id, "test", jiff::Timestamp::now())
        .await
        .expect("create token");
    (account.id, issued.secret, cookie)
}

fn assert_unauthorized(response: &topcoat::router::response::Response) {
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers()[header::WWW_AUTHENTICATE],
        "Bearer realm=\"varve\""
    );
}

#[tokio::test]
async fn graphql_without_a_live_bearer_token_is_401() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let (_, secret, cookie) = account_with_token(&router, &mut db, "api-401").await;

    // No header; a foreign scheme; a well-formed but unknown token.
    for authorization in [
        None,
        Some("Basic dXNlcjpwYXNz"),
        Some("Bearer"),
        Some("Bearer not-ours"),
        Some("Bearer varve_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
    ] {
        let response = router.handle(graphql(VIEWER, authorization, &[])).await;
        assert_unauthorized(&response);
        let body = body_text(response).await;
        assert!(body.contains("Unauthorized"), "{authorization:?}: {body}");
        assert!(!body.contains("viewer"), "{authorization:?}: {body}");
    }

    // The session cookie never authenticates the API.
    let response = router
        .handle(graphql(VIEWER, None, &[("cookie", &cookie)]))
        .await;
    assert_unauthorized(&response);

    // The real token, for contrast, does — so the 401s above are
    // the guard, not a broken route.
    let response = router
        .handle(graphql(VIEWER, Some(&format!("Bearer {secret}")), &[]))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn graphql_executes_as_the_token_account() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let (account_id, secret, _) = account_with_token(&router, &mut db, "api-viewer").await;

    let response = router
        .handle(graphql(VIEWER, Some(&format!("bearer  {secret} ")), &[]))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    let body: serde_json::Value = serde_json::from_str(&body_text(response).await).unwrap();
    assert_eq!(body["errors"], serde_json::Value::Null, "{body}");
    assert_eq!(body["data"]["viewer"]["accountId"], account_id.to_string());
    assert!(
        body["data"]["viewer"]["email"]
            .as_str()
            .unwrap()
            .starts_with("api-viewer-")
    );
    // Signup stored the resolved locale (English, no Accept-Language).
    assert_eq!(body["data"]["viewer"]["locale"], "en");
}

#[tokio::test]
async fn revoked_and_expired_tokens_are_401() {
    let Some((router, db)) = test_app().await else {
        return;
    };
    let mut db = db;
    let (account_id, secret, _) = account_with_token(&router, &mut db, "api-revoked").await;

    let token = platform_core::find_live_api_token(&mut db, &secret, jiff::Timestamp::now())
        .await
        .expect("lookup")
        .expect("live");
    assert!(
        platform_core::destroy_api_token(&mut db, account_id, token.id)
            .await
            .expect("destroy")
    );
    let response = router
        .handle(graphql(VIEWER, Some(&format!("Bearer {secret}")), &[]))
        .await;
    assert_unauthorized(&response);

    // A token created seven months ago is past its six-month life.
    let long_ago = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .checked_sub(jiff::Span::new().months(7))
        .unwrap()
        .timestamp();
    let expired = platform_core::create_api_token(&mut db, account_id, "old", long_ago)
        .await
        .expect("create");
    let response = router
        .handle(graphql(
            VIEWER,
            Some(&format!("Bearer {}", expired.secret)),
            &[],
        ))
        .await;
    assert_unauthorized(&response);
}
