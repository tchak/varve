//! Subject: organizations — the signed-in gate, the list (empty and
//! populated), creation through the typed client with every
//! viewer-fixable failure in the field, and the organization page
//! with its one 404 answer for absent, foreign, and malformed ids.

use topcoat::router::{Router, StatusCode, header};

use crate::harness::{
    body_text, form_body, get, post, session_cookie, signup, test_app, unique_email,
};

async fn member(router: &Router, tag: &str) -> String {
    signup(router, tag, &unique_email(tag), "s3cret-enough").await
}

fn unique_slug(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4())
}

/// Creates an organization as `cookie`, returning the new page's path
/// (`/organizations/{id}`) from the 303.
async fn create(router: &Router, cookie: &str, name: &str, slug: &str) -> String {
    let response = router
        .handle(post(
            "/organizations",
            &[("cookie", cookie)],
            form_body(&[("name", name), ("slug", slug)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(location.starts_with("/organizations/"), "{location}");
    location
}

#[tokio::test]
async fn anonymous_requests_redirect_to_signin() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    for path in [
        "/organizations",
        "/organizations/00000000-0000-0000-0000-000000000000",
    ] {
        let response = router.handle(get(path, &[])).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{path}");
        assert_eq!(response.headers()[header::LOCATION], "/signin");
    }
    let response = router
        .handle(post(
            "/organizations",
            &[],
            form_body(&[("name", "Nope"), ("slug", "nope")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signin");
}

#[tokio::test]
async fn the_list_starts_empty_and_shows_what_the_viewer_created() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "lister").await;
    let response = router
        .handle(get("/organizations", &[("cookie", &cookie)]))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("<h1"), "{html}");
    assert!(html.contains("Organizations"), "{html}");
    assert!(
        html.contains("You are not a member of any organization yet."),
        "{html}"
    );
    assert!(html.contains("id=\"organization-name\""), "{html}");
    assert!(html.contains("id=\"organization-slug\""), "{html}");

    let slug = unique_slug("listed");
    let location = create(&router, &cookie, "Listed Org", &slug).await;
    let response = router
        .handle(get("/organizations", &[("cookie", &cookie)]))
        .await;
    let html = body_text(response).await;
    assert!(html.contains("Listed Org"), "{html}");
    assert!(html.contains(&slug), "{html}");
    assert!(html.contains(&format!("href=\"{location}\"")), "{html}");
    assert!(
        !html.contains("You are not a member of any organization yet."),
        "{html}"
    );

    // Another account's list does not carry it (G.6: the list is
    // the viewer's memberships).
    let other = member(&router, "other-lister").await;
    let response = router
        .handle(get("/organizations", &[("cookie", &other)]))
        .await;
    let html = body_text(response).await;
    assert!(!html.contains("Listed Org"), "{html}");
}

#[tokio::test]
async fn creation_lands_on_the_organization_page() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "creator").await;
    let slug = unique_slug("created");
    // Surrounding whitespace is trimmed before the round trip; the
    // slug's case is normalized by the schema's scalar.
    let location = create(
        &router,
        &cookie,
        "  Created Org  ",
        &format!(" {} ", slug.to_uppercase()),
    )
    .await;
    let response = router.handle(get(&location, &[("cookie", &cookie)])).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("<h1"), "{html}");
    assert!(html.contains("Created Org"), "{html}");
    assert!(html.contains(&slug), "{html}");
    assert!(html.contains("Created on"), "{html}");
    // The creator is the first member, shown by name and email.
    assert!(html.contains("Members (1)"), "{html}");
    assert!(html.contains("creator"), "{html}");
    assert!(html.contains("Teams (0)"), "{html}");
    assert!(html.contains("No teams yet."), "{html}");
    assert!(html.contains("Procedures (0)"), "{html}");
    assert!(html.contains("No procedures yet."), "{html}");
}

#[tokio::test]
async fn creation_errors_land_in_their_field() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "validator").await;

    // Blank fields: both errors at once, nothing created, the
    // submitted values kept.
    let response = router
        .handle(post(
            "/organizations",
            &[("cookie", &cookie)],
            form_body(&[("name", "   "), ("slug", " ")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("Please enter a name."), "{html}");
    assert!(html.contains("Please enter an identifier."), "{html}");
    assert!(
        html.contains("id=\"organization-name\"") && html.contains("aria-invalid=\"true\""),
        "{html}"
    );

    // An invalid slug: the schema's INVALID_INPUT, mapped to the
    // slug field, the name kept.
    let response = router
        .handle(post(
            "/organizations",
            &[("cookie", &cookie)],
            form_body(&[("name", "Kept Name"), ("slug", "not valid!")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(
        html.contains("The identifier may only contain lowercase letters, digits, and hyphens."),
        "{html}"
    );
    assert!(html.contains("value=\"Kept Name\""), "{html}");
    assert!(html.contains("value=\"not valid!\""), "{html}");
    assert!(!html.contains("Please enter a name."), "{html}");

    // A taken slug: SLUG_TAKEN, in the slug field.
    let slug = unique_slug("taken");
    create(&router, &cookie, "First", &slug).await;
    let response = router
        .handle(post(
            "/organizations",
            &[("cookie", &cookie)],
            form_body(&[("name", "Second"), ("slug", &slug)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = body_text(response).await;
    assert!(html.contains("This identifier is already taken."), "{html}");
    assert!(html.contains("id=\"organization-slug\""), "{html}");
}

#[tokio::test]
async fn absent_foreign_and_malformed_ids_are_one_404() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let owner = member(&router, "owner").await;
    let stranger = member(&router, "stranger").await;
    let location = create(&router, &owner, "Private Org", &unique_slug("private")).await;

    for (who, path) in [
        (&stranger, location.as_str()),
        (
            &owner,
            "/organizations/00000000-0000-0000-0000-000000000000",
        ),
        (&owner, "/organizations/not-a-uuid"),
    ] {
        let response = router.handle(get(path, &[("cookie", who)])).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        let html = body_text(response).await;
        assert!(html.contains("Page not found."), "{path}: {html}");
        assert!(!html.contains("Private Org"), "{path}: {html}");
    }
}

#[tokio::test]
async fn the_pages_speak_french() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    // Signup stores the resolved locale as the account's preference,
    // and a stored preference outranks the header — so the account
    // signs up in French.
    let response = router
        .handle(post(
            "/signup",
            &[("accept-language", "fr")],
            form_body(&[
                ("name", "Francophone"),
                ("email", &unique_email("francophone")),
                ("password", "s3cret-enough"),
            ]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie = session_cookie(&response).expect("signup sets a session cookie");
    let headers = [("cookie", cookie.as_str())];
    let response = router.handle(get("/organizations", &headers)).await;
    let html = body_text(response).await;
    assert!(html.contains("lang=\"fr\""), "{html}");
    assert!(html.contains("Organisations"), "{html}");
    assert!(html.contains("Créer une organisation"), "{html}");
    assert!(
        html.contains("Vous n'êtes membre d'aucune organisation pour le moment."),
        "{html}"
    );
    let location = create(&router, &cookie, "Org FR", &unique_slug("fr")).await;
    let response = router.handle(get(&location, &headers)).await;
    let html = body_text(response).await;
    assert!(html.contains("Créée le"), "{html}");
    assert!(html.contains("Membres (1)"), "{html}");
    assert!(html.contains("Aucune équipe pour le moment."), "{html}");
}
