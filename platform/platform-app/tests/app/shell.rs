//! Subject: the HTML shell around every page — the signed-out
//! default in English, and the branded not-found rendered inside the
//! layout instead of a bare 404.

use topcoat::router::StatusCode;

use crate::harness::{body_text, get, test_app};

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
