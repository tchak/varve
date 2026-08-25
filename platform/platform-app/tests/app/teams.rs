//! Subject: teams of an organization — the page under the
//! organization's id (gate, the one-404 rule for foreign
//! organizations on GET and POST alike), the list (empty and
//! populated, reflected on the organization page), creation with the
//! blank-name error in the field, and French.

use topcoat::router::{Router, StatusCode, header};

use crate::harness::{
    body_text, form_body, get, post, session_cookie, signup, test_app, unique_email,
};

async fn member(router: &Router, tag: &str) -> String {
    signup(router, tag, &unique_email(tag), "s3cret-enough").await
}

/// Creates an organization as `cookie`; its page path from the 303.
async fn create_organization(router: &Router, cookie: &str, name: &str) -> String {
    let slug = format!("teams-{}", uuid::Uuid::new_v4());
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
    let path = "/organizations/00000000-0000-0000-0000-000000000000/teams";
    let response = router.handle(get(path, &[])).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signin");
    let response = router
        .handle(post(path, &[], form_body(&[("name", "Nope")])))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signin");
}

#[tokio::test]
async fn foreign_and_absent_organizations_are_404_on_get_and_post() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let owner = member(&router, "teams-owner").await;
    let stranger = member(&router, "teams-stranger").await;
    let organization = create_organization(&router, &owner, "Closed").await;
    let teams = format!("{organization}/teams");
    for (who, path) in [
        (&stranger, teams.as_str()),
        (
            &owner,
            "/organizations/00000000-0000-0000-0000-000000000000/teams",
        ),
        (&owner, "/organizations/not-a-uuid/teams"),
    ] {
        let response = router.handle(get(path, &[("cookie", who)])).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "GET {path}");
        assert!(body_text(response).await.contains("Page not found."));
        let response = router
            .handle(post(
                path,
                &[("cookie", who)],
                form_body(&[("name", "Smuggled")]),
            ))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "POST {path}");
    }
    // The stranger's POST created nothing.
    let response = router.handle(get(&teams, &[("cookie", &owner)])).await;
    let html = body_text(response).await;
    assert!(!html.contains("Smuggled"), "{html}");
}

#[tokio::test]
async fn the_list_starts_empty_and_creation_shows_everywhere() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "teams-creator").await;
    let organization = create_organization(&router, &cookie, "Staffed").await;
    let teams = format!("{organization}/teams");

    // The organization page links here.
    let response = router
        .handle(get(&organization, &[("cookie", &cookie)]))
        .await;
    let html = body_text(response).await;
    assert!(html.contains(&format!("href=\"{teams}\"")), "{html}");
    assert!(html.contains("Manage teams"), "{html}");

    let response = router.handle(get(&teams, &[("cookie", &cookie)])).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("<h1"), "{html}");
    // The breadcrumb trail carries the upward navigation: the
    // organization is a link, the page itself the current crumb.
    assert!(html.contains("data-breadcrumbs"), "{html}");
    assert!(html.contains(&format!("href=\"{organization}\"")), "{html}");
    assert!(html.contains("aria-current=\"page\""), "{html}");
    assert!(html.contains("Staffed"), "{html}");
    assert!(html.contains("No teams yet."), "{html}");
    assert!(html.contains("id=\"team-name\""), "{html}");

    let response = router
        .handle(post(
            &teams,
            &[("cookie", &cookie)],
            form_body(&[("name", "  Reviewers  ")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], teams);

    let response = router.handle(get(&teams, &[("cookie", &cookie)])).await;
    let html = body_text(response).await;
    assert!(html.contains("data-team-id="), "{html}");
    assert!(html.contains(">Reviewers<"), "{html}");
    assert!(!html.contains("No teams yet."), "{html}");

    // ... and on the organization page, with its count.
    let response = router
        .handle(get(&organization, &[("cookie", &cookie)]))
        .await;
    let html = body_text(response).await;
    assert!(html.contains("Teams (1)"), "{html}");
    assert!(html.contains("Reviewers"), "{html}");
}

#[tokio::test]
async fn a_blank_name_lands_in_the_field() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "teams-validator").await;
    let organization = create_organization(&router, &cookie, "Picky").await;
    let teams = format!("{organization}/teams");
    let response = router
        .handle(post(
            &teams,
            &[("cookie", &cookie)],
            form_body(&[("name", "   ")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("Please enter a name."), "{html}");
    assert!(
        html.contains("id=\"team-name\"") && html.contains("aria-invalid=\"true\""),
        "{html}"
    );
    // Still the teams page, still empty.
    assert!(html.contains("Picky"), "{html}");
    assert!(html.contains("No teams yet."), "{html}");
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
                ("email", &unique_email("teams-fr")),
                ("password", "s3cret-enough"),
            ]),
        ))
        .await;
    let cookie = session_cookie(&response).expect("signup sets a session cookie");
    let organization = create_organization(&router, &cookie, "Org FR").await;
    let response = router
        .handle(get(
            &format!("{organization}/teams"),
            &[("cookie", &cookie)],
        ))
        .await;
    let html = body_text(response).await;
    assert!(html.contains("lang=\"fr\""), "{html}");
    assert!(html.contains("Fil d'Ariane"), "{html}");
    assert!(html.contains("Organisations"), "{html}");
    assert!(html.contains("Créer une équipe"), "{html}");
    assert!(html.contains("Aucune équipe pour le moment."), "{html}");
}
