//! Subject: procedures of an organization — the page under the
//! organization's id (gate, the one-404 rule for foreign
//! organizations on GET and POST alike), the list (empty and
//! populated, reflected on the organization page), creation with the
//! optional description and the blank-title error in the field, and
//! French.

use topcoat::router::{Router, StatusCode, header};

use crate::harness::{
    body_text, form_body, get, post, session_cookie, signup, test_app, unique_email,
};

async fn member(router: &Router, tag: &str) -> String {
    signup(router, tag, &unique_email(tag), "s3cret-enough").await
}

/// Creates an organization as `cookie`; its page path from the 303.
async fn create_organization(router: &Router, cookie: &str, name: &str) -> String {
    let slug = format!("procedures-{}", uuid::Uuid::new_v4());
    let response = router
        .handle(post(
            "/organizations",
            &[("cookie", cookie)],
            form_body(&[("name", name), ("slug", &slug)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn anonymous_requests_redirect_to_signin() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let path = "/organizations/00000000-0000-0000-0000-000000000000/procedures";
    let response = router.handle(get(path, &[])).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signin");
    let response = router
        .handle(post(path, &[], form_body(&[("title", "Nope")])))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signin");
}

#[tokio::test]
async fn foreign_and_absent_organizations_are_404_on_get_and_post() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let owner = member(&router, "procedures-owner").await;
    let stranger = member(&router, "procedures-stranger").await;
    let organization = create_organization(&router, &owner, "Closed").await;
    let procedures = format!("{organization}/procedures");
    for (who, path) in [
        (&stranger, procedures.as_str()),
        (
            &owner,
            "/organizations/00000000-0000-0000-0000-000000000000/procedures",
        ),
        (&owner, "/organizations/not-a-uuid/procedures"),
    ] {
        let response = router.handle(get(path, &[("cookie", who)])).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "GET {path}");
        assert!(body_text(response).await.contains("Page not found."));
        let response = router
            .handle(post(
                path,
                &[("cookie", who)],
                form_body(&[("title", "Smuggled")]),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "POST {path}");
    }
    // The stranger's POST created nothing.
    let response = router.handle(get(&procedures, &[("cookie", &owner)])).await;
    let html = body_text(response).await;
    assert!(!html.contains("Smuggled"), "{html}");
}

#[tokio::test]
async fn the_list_starts_empty_and_creation_shows_everywhere() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "procedures-creator").await;
    let organization = create_organization(&router, &cookie, "Staffed").await;
    let procedures = format!("{organization}/procedures");

    // The organization page links here.
    let response = router
        .handle(get(&organization, &[("cookie", &cookie)]))
        .await;
    let html = body_text(response).await;
    assert!(html.contains(&format!("href=\"{procedures}\"")), "{html}");
    assert!(html.contains("Manage procedures"), "{html}");

    let response = router
        .handle(get(&procedures, &[("cookie", &cookie)]))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("<h1"), "{html}");
    assert!(html.contains("Procedures of"), "{html}");
    assert!(html.contains(&format!("href=\"{organization}\"")), "{html}");
    assert!(html.contains("Staffed"), "{html}");
    assert!(html.contains("No procedures yet."), "{html}");
    assert!(html.contains("id=\"procedure-title\""), "{html}");
    assert!(html.contains("id=\"procedure-description\""), "{html}");

    let response = router
        .handle(post(
            &procedures,
            &[("cookie", &cookie)],
            form_body(&[
                ("title", "  Permit  "),
                ("description", " With a description. "),
            ]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], procedures);

    let response = router
        .handle(get(&procedures, &[("cookie", &cookie)]))
        .await;
    let html = body_text(response).await;
    assert!(html.contains("data-procedure-id="), "{html}");
    assert!(html.contains(">Permit<"), "{html}");
    assert!(!html.contains("No procedures yet."), "{html}");

    // ... and on the organization page, with its count.
    let response = router
        .handle(get(&organization, &[("cookie", &cookie)]))
        .await;
    let html = body_text(response).await;
    assert!(html.contains("Procedures (1)"), "{html}");
    assert!(html.contains("Permit"), "{html}");

    // A creation without a description is fine too (the server's
    // default): the field is optional.
    let response = router
        .handle(post(
            &procedures,
            &[("cookie", &cookie)],
            form_body(&[("title", "Bare")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = router
        .handle(get(&organization, &[("cookie", &cookie)]))
        .await;
    let html = body_text(response).await;
    assert!(html.contains("Procedures (2)"), "{html}");
}

#[tokio::test]
async fn a_blank_title_lands_in_the_field() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "procedures-validator").await;
    let organization = create_organization(&router, &cookie, "Picky").await;
    let procedures = format!("{organization}/procedures");
    let response = router
        .handle(post(
            &procedures,
            &[("cookie", &cookie)],
            form_body(&[("title", "   "), ("description", "Kept")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("Please enter a title."), "{html}");
    assert!(
        html.contains("id=\"procedure-title\"") && html.contains("aria-invalid=\"true\""),
        "{html}"
    );
    // The description survives the re-render.
    assert!(html.contains(">Kept</textarea>"), "{html}");
    // Still the procedures page, still empty.
    assert!(html.contains("Picky"), "{html}");
    assert!(html.contains("No procedures yet."), "{html}");
}

#[tokio::test]
async fn the_page_speaks_french() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let response = router
        .handle(post(
            "/signup",
            &[("accept-language", "fr")],
            form_body(&[
                ("name", "Francophone"),
                ("email", &unique_email("procedures-fr")),
                ("password", "s3cret-enough"),
            ]),
        ))
        .await;
    let cookie = session_cookie(&response).expect("signup sets a session cookie");
    let organization = create_organization(&router, &cookie, "Org FR").await;
    let response = router
        .handle(get(
            &format!("{organization}/procedures"),
            &[("cookie", &cookie)],
        ))
        .await;
    let html = body_text(response).await;
    assert!(html.contains("lang=\"fr\""), "{html}");
    assert!(html.contains("Procédures de"), "{html}");
    assert!(html.contains("Créer une procédure"), "{html}");
    assert!(html.contains("Aucune procédure pour le moment."), "{html}");
}
