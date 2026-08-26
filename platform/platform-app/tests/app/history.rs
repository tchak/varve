//! Subject: procedure history (platform P.4 *Procedure history*,
//! G.11) — the event log on the procedure page rendered newest-first
//! with a diff link per publication, the diff page (first
//! publication as the initial schema, a later one naming additions
//! *and* removals by label), its one-404 rule (stranger, unknown
//! event), and French.

use topcoat::router::{Router, StatusCode, header};

use crate::harness::{
    body_text, form_body, get, post, session_cookie, signup, test_app, unique_email,
};

async fn member(router: &Router, tag: &str) -> String {
    signup(router, tag, &unique_email(tag), "s3cret-enough").await
}

/// A member whose stored locale is French.
async fn french_member(router: &Router, tag: &str) -> String {
    let response = router
        .handle(post(
            "/signup",
            &[("accept-language", "fr-FR,fr;q=0.9")],
            form_body(&[
                ("name", tag),
                ("email", &unique_email(tag)),
                ("password", "s3cret-enough"),
            ]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    session_cookie(&response).expect("signup sets a session cookie")
}

/// Creates an organization as `cookie`; its page path from the 303.
async fn create_organization(router: &Router, cookie: &str, name: &str) -> String {
    let slug = format!("history-{}", uuid::Uuid::new_v4());
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

/// Creates a procedure in `organization`; its page path.
async fn create_procedure(
    router: &Router,
    cookie: &str,
    organization: &str,
    title: &str,
) -> String {
    let procedures = format!("{organization}/procedures");
    let response = router
        .handle(post(
            &procedures,
            &[("cookie", cookie)],
            form_body(&[("title", title)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = router.handle(get(&procedures, &[("cookie", cookie)])).await;
    let html = body_text(response).await;
    let needle = "data-procedure-id=\"";
    let at = html.rfind(needle).expect("the created procedure is listed");
    let rest = &html[at + needle.len()..];
    let id = &rest[..rest.find('"').unwrap()];
    format!("{organization}/procedures/{id}")
}

/// POSTs an editor action and asserts the 303.
async fn act(router: &Router, cookie: &str, path: &str, fields: &[(&str, &str)]) {
    let response = router
        .handle(post(path, &[("cookie", cookie)], form_body(fields)))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER, "POST {path}");
}

/// The element id the add POST selected, from its 303 target.
async fn add_column(router: &Router, cookie: &str, procedure: &str, label: &str) -> String {
    let response = router
        .handle(post(
            &format!("{procedure}/schema/add"),
            &[("cookie", cookie)],
            form_body(&[("what", "column"), ("label", label)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()[header::LOCATION].to_str().unwrap();
    location
        .split("selected=")
        .nth(1)
        .expect("add selects the new element")
        .split('&')
        .next()
        .unwrap()
        .to_owned()
}

async fn page(router: &Router, cookie: &str, path: &str) -> String {
    let response = router.handle(get(path, &[("cookie", cookie)])).await;
    assert_eq!(response.status(), StatusCode::OK, "GET {path}");
    body_text(response).await
}

/// The diff hrefs in the history section, document order (the page
/// renders the trail newest-first).
fn diff_hrefs(html: &str) -> Vec<String> {
    let needle = "href=\"";
    html.match_indices(needle)
        .filter_map(|(at, _)| {
            let rest = &html[at + needle.len()..];
            let href = &rest[..rest.find('"').unwrap()];
            href.contains("/history/").then(|| href.to_owned())
        })
        .collect()
}

#[tokio::test]
async fn the_history_lists_events_and_each_publication_answers_its_diff() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "history").await;
    let organization = create_organization(&router, &cookie, "Préfecture").await;
    let procedure = create_procedure(&router, &cookie, &organization, "Permis").await;

    // Before any publication the trail holds creation only — a line
    // naming the actor, no diff link.
    let html = page(&router, &cookie, &procedure).await;
    assert!(html.contains("data-history"), "{html}");
    assert!(html.contains("Created by history on "), "{html}");
    assert!(diff_hrefs(&html).is_empty(), "{html}");

    // First publication: the history gains a linked row.
    let nom = add_column(&router, &cookie, &procedure, "Nom").await;
    act(
        &router,
        &cookie,
        &format!("{procedure}/schema/publish"),
        &[],
    )
    .await;
    let html = page(&router, &cookie, &procedure).await;
    assert!(html.contains("Revision published by history on "), "{html}");
    assert!(html.contains("View the changes"), "{html}");
    let hrefs = diff_hrefs(&html);
    assert_eq!(hrefs.len(), 1, "{html}");

    // Its diff page: the first publication is the initial schema —
    // the report against the empty schema, every column added.
    let html = page(&router, &cookie, &hrefs[0]).await;
    assert!(html.contains("Publication of "), "{html}");
    assert!(html.contains("Revision published by history on "), "{html}");
    assert!(
        html.contains("First publication \u{2014} the initial schema."),
        "{html}"
    );
    assert!(html.contains("data-impact-report"), "{html}");
    assert!(
        html.contains("\u{201c}Nom\u{201d} is added \u{2014} no impact on existing answers."),
        "{html}"
    );
    // Upward navigation is the breadcrumb trail: the procedure is a
    // link, the publication itself the current crumb.
    assert!(html.contains("data-breadcrumbs"), "{html}");
    assert!(html.contains(&format!("href=\"{procedure}\"")), "{html}");
    assert!(html.contains("aria-current=\"page\""), "{html}");

    // Second publication: one column added, one removed. Newest
    // first on the page; the diff names the removal by the *base*
    // schema's label — the draft no longer holds it.
    add_column(&router, &cookie, &procedure, "Ville").await;
    act(
        &router,
        &cookie,
        &format!("{procedure}/schema/elements/{nom}/remove"),
        &[],
    )
    .await;
    // §3.1: the new column is required (the public default), so the
    // surface half gates — the plain POST lands on the confirmation
    // state, and the re-send with `confirm` publishes.
    let response = router
        .handle(post(
            &format!("{procedure}/schema/publish"),
            &[("cookie", &cookie)],
            form_body(&[]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()[header::LOCATION].to_str().unwrap();
    assert!(location.contains("publish=confirm"), "{location}");
    // The confirmation names the surface half (§3.1): the required
    // newcomer joins the form, in-flight files may lapse — one line,
    // the applicant/reviewer pair deduplicated.
    let html = page(&router, &cookie, location).await;
    assert!(
        html.contains("\u{201c}Ville\u{201d} joins the form."),
        "{html}"
    );
    assert!(
        html.contains("Case files in progress may no longer be admissible."),
        "{html}"
    );
    act(
        &router,
        &cookie,
        &format!("{procedure}/schema/publish"),
        &[("confirm", "true")],
    )
    .await;
    let html = page(&router, &cookie, &procedure).await;
    let hrefs = diff_hrefs(&html);
    assert_eq!(hrefs.len(), 2, "{html}");
    assert_ne!(hrefs[0], hrefs[1]);
    let html = page(&router, &cookie, &hrefs[0]).await;
    assert!(
        !html.contains("First publication \u{2014} the initial schema."),
        "{html}"
    );
    assert!(
        html.contains("\u{201c}Ville\u{201d} is added \u{2014} no impact on existing answers."),
        "{html}"
    );
    assert!(
        html.contains("\u{201c}Nom\u{201d} is removed \u{2014} its stored answers are kept."),
        "{html}"
    );
}

#[tokio::test]
async fn strangers_and_unknown_events_are_404() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let owner = member(&router, "history-owner").await;
    let stranger = member(&router, "history-stranger").await;
    let organization = create_organization(&router, &owner, "Fermée").await;
    let procedure = create_procedure(&router, &owner, &organization, "Aide").await;
    add_column(&router, &owner, &procedure, "Nom").await;
    act(&router, &owner, &format!("{procedure}/schema/publish"), &[]).await;
    let html = page(&router, &owner, &procedure).await;
    let diff = diff_hrefs(&html).remove(0);

    // A member of nothing sees the same 404 as an absent procedure.
    let response = router.handle(get(&diff, &[("cookie", &stranger)])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // An event id the procedure does not hold, and a malformed one.
    for bad in [
        format!("{procedure}/history/{}", uuid::Uuid::new_v4()),
        format!("{procedure}/history/not-an-id"),
    ] {
        let response = router.handle(get(&bad, &[("cookie", &owner)])).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "GET {bad}");
    }
}

#[tokio::test]
async fn the_history_speaks_french() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = french_member(&router, "history-fr").await;
    let organization = create_organization(&router, &cookie, "Mairie").await;
    let procedure = create_procedure(&router, &cookie, &organization, "Aide").await;
    add_column(&router, &cookie, &procedure, "Nom").await;
    act(
        &router,
        &cookie,
        &format!("{procedure}/schema/publish"),
        &[],
    )
    .await;

    let html = page(&router, &cookie, &procedure).await;
    assert!(html.contains("Historique"), "{html}");
    assert!(
        html.contains("Révision publiée par history-fr le "),
        "{html}"
    );
    assert!(html.contains("Voir les modifications"), "{html}");
    let diff = diff_hrefs(&html).remove(0);

    let html = page(&router, &cookie, &diff).await;
    assert!(html.contains("Publication du "), "{html}");
    assert!(
        html.contains("Première publication \u{2014} le schéma initial."),
        "{html}"
    );
    assert!(
        html.contains("«\u{a0}Nom\u{a0}» est ajoutée \u{2014} sans impact"),
        "{html}"
    );
    assert!(html.contains("Fil d'Ariane"), "{html}");
}

/// The bug that opened Q23, end to end: a section rename is a
/// surface-only publication — same revision id, a distinct
/// publication — that publishes free (presentation is safe, §3.1)
/// and whose diff page *names the rename* instead of showing "no
/// changes".
#[tokio::test]
async fn a_section_rename_publishes_free_and_shows_in_the_diff() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "history-rename").await;
    let organization = create_organization(&router, &cookie, "Mairie").await;
    let procedure = create_procedure(&router, &cookie, &organization, "Aide").await;

    // A section, then the first publication.
    let response = router
        .handle(post(
            &format!("{procedure}/schema/add"),
            &[("cookie", &cookie)],
            form_body(&[("what", "section"), ("label", "Identité")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()[header::LOCATION].to_str().unwrap();
    let section = location
        .split("selected=")
        .nth(1)
        .expect("add selects the section")
        .split('&')
        .next()
        .unwrap()
        .to_owned();
    act(
        &router,
        &cookie,
        &format!("{procedure}/schema/publish"),
        &[],
    )
    .await;

    // The rename, and the surface-only publication: free — no
    // confirmation state on the way.
    act(
        &router,
        &cookie,
        &format!("{procedure}/schema/elements/{section}/update"),
        &[("title", "Votre identité")],
    )
    .await;
    act(
        &router,
        &cookie,
        &format!("{procedure}/schema/publish"),
        &[],
    )
    .await;

    // Two publications, and the newest diff names the rename.
    let html = page(&router, &cookie, &procedure).await;
    let hrefs = diff_hrefs(&html);
    assert_eq!(hrefs.len(), 2, "{html}");
    let html = page(&router, &cookie, &hrefs[0]).await;
    assert!(
        html.contains(
            "The \u{201c}Identité\u{201d} section is renamed \u{201c}Votre identité\u{201d}."
        ),
        "{html}"
    );
    assert!(
        !html.contains("No changes against the published revision."),
        "{html}"
    );
}
