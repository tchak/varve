//! Subject: the HTML shell around every page — the signed-out
//! default in English, the signed-in header's account menu, and the
//! branded not-found rendered inside the layout instead of a bare
//! 404.

use topcoat::router::StatusCode;

use crate::harness::{body_text, get, signup, test_app, unique_email};

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

#[tokio::test]
async fn signed_in_header_shows_the_account_menu() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let email = unique_email("shell-menu");
    let cookie = signup(&router, "Menu", &email, "s3cret-enough").await;
    let response = router.handle(get("/", &[("cookie", &cookie)])).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;

    // The signed-in header is a `<details>`-based account menu: an
    // icon-only trigger named for assistive technology...
    assert!(html.contains(r#"aria-label="Account menu""#), "{html}");
    let start = html.find("<details").expect("the account menu renders");
    let end = start + html[start..].find("</details>").expect("the menu closes");
    let menu = &html[start..end];
    // ...holding the account email as non-interactive identity
    // display (a paragraph, not a control), the settings link, and
    // the sign-out POST form — the origin-policy protection depends
    // on sign-out staying a POST.
    assert!(menu.contains(&format!(">{email}</p>")), "{menu}");
    assert!(!menu.contains(r#"name="email""#), "{menu}");
    assert!(menu.contains(r#"href="/settings""#), "{menu}");
    assert!(
        menu.contains(r#"<form method="post" action="/signout""#),
        "{menu}"
    );
    // The old inline signed-in state is gone.
    assert!(!html.contains("Signed in as"), "{html}");
}
