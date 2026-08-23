//! Subject: the schema editor of a procedure's revision draft —
//! the gate (sign-in redirect, the one-404 rule for strangers on GET
//! and POST), the empty state, adding columns and groups at the root
//! and inside a group (before an anchor), moving up / down / to,
//! updating label, type with unit, enum options (kept and minted
//! ids, a blank row removed), arity and cardinality, the refused
//! edits as alerts (blank label, a `many` group inside a `many`
//! group), removing, discarding behind its confirmation, and French.
//! Every response passes the static accessibility baseline through
//! `body_text`.

use topcoat::router::{Router, StatusCode, header};

use crate::harness::{
    body_text, form_body, get, post, session_cookie, signup, test_app, unique_email,
};

async fn member(router: &Router, tag: &str) -> String {
    signup(router, tag, &unique_email(tag), "s3cret-enough").await
}

/// A member whose stored locale is French (resolved from the
/// sign-up request's `Accept-Language`, which the account keeps).
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

/// The one-shot notice cookie a POST sets for the editor's next GET,
/// as a `name=value` pair to send back with the session.
fn notice_cookie(response: &topcoat::router::response::Response) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("__Host-schema-notice=") && !value.contains("Max-Age=0"))
        .map(|value| value.split(';').next().unwrap().to_owned())
}

/// Where a POST landed: the 303 target and the notice it left.
struct Landing {
    to: String,
    notice: Option<String>,
}

fn location(response: &topcoat::router::response::Response) -> String {
    response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned()
}

/// Creates an organization as `cookie`; its page path from the 303.
async fn create_organization(router: &Router, cookie: &str, name: &str) -> String {
    let slug = format!("schema-{}", uuid::Uuid::new_v4());
    let response = router
        .handle(post(
            "/organizations",
            &[("cookie", cookie)],
            form_body(&[("name", name), ("slug", &slug)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    location(&response)
}

/// Creates a procedure in `organization`; its editor path.
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
    let id = attribute_values(&html, "data-procedure-id")
        .pop()
        .expect("the created procedure is listed");
    format!("{organization}/procedures/{id}/schema")
}

/// Every value of `attribute` in `html`, in document order.
fn attribute_values(html: &str, attribute: &str) -> Vec<String> {
    let needle = format!("{attribute}=\"");
    html.match_indices(&needle)
        .map(|(at, _)| {
            let rest = &html[at + needle.len()..];
            rest[..rest.find('"').unwrap()].to_owned()
        })
        .collect()
}

/// The element ids in the structure panel, document order.
fn element_ids(html: &str) -> Vec<String> {
    attribute_values(html, "data-element-id")
}

/// The `selected` id a 303 to the editor carries.
fn selected_of(location: &str) -> String {
    let (_, query) = location.split_once("?selected=").expect("a selection");
    query.split('&').next().unwrap().to_owned()
}

/// Posts `fields` to `path` and returns where it landed.
async fn act(router: &Router, cookie: &str, path: &str, fields: &[(&str, &str)]) -> Landing {
    let response = router
        .handle(post(path, &[("cookie", cookie)], form_body(fields)))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER, "POST {path}");
    Landing {
        to: location(&response),
        notice: notice_cookie(&response),
    }
}

/// Gets `path` as `cookie`, asserting 200, and returns the HTML.
async fn page(router: &Router, cookie: &str, path: &str) -> String {
    let response = router.handle(get(path, &[("cookie", cookie)])).await;
    assert_eq!(response.status(), StatusCode::OK, "GET {path}");
    body_text(response).await
}

/// Follows a landing with its notice, as a browser would.
async fn landed(router: &Router, cookie: &str, landing: &Landing) -> String {
    let jar = match &landing.notice {
        Some(notice) => format!("{cookie}; {notice}"),
        None => cookie.to_owned(),
    };
    let response = router.handle(get(&landing.to, &[("cookie", &jar)])).await;
    assert_eq!(response.status(), StatusCode::OK, "GET {}", landing.to);
    body_text(response).await
}

#[tokio::test]
async fn anonymous_requests_redirect_to_signin() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let editor = "/organizations/00000000-0000-0000-0000-000000000000/procedures/00000000-0000-0000-0000-000000000000/schema";
    let response = router.handle(get(editor, &[])).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signin");
    let response = router
        .handle(post(
            &format!("{editor}/add"),
            &[],
            form_body(&[("what", "column"), ("label", "Nope")]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signin");
}

#[tokio::test]
async fn strangers_and_absent_procedures_are_404_on_get_and_post() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let owner = member(&router, "schema-owner").await;
    let stranger = member(&router, "schema-stranger").await;
    let organization = create_organization(&router, &owner, "Closed").await;
    let editor = create_procedure(&router, &owner, &organization, "Private").await;
    let absent = format!("{organization}/procedures/00000000-0000-0000-0000-000000000000/schema");
    let malformed = format!("{organization}/procedures/not-a-uuid/schema");
    for (who, path) in [
        (&stranger, editor.as_str()),
        (&owner, absent.as_str()),
        (&owner, malformed.as_str()),
    ] {
        let response = router.handle(get(path, &[("cookie", who)])).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "GET {path}");
        assert!(body_text(response).await.contains("Page not found."));
        for action in ["add", "discard"] {
            let response = router
                .handle(post(
                    &format!("{path}/{action}"),
                    &[("cookie", who)],
                    form_body(&[("what", "column"), ("label", "Smuggled")]),
                ))
                .await;
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "POST {path}/{action}"
            );
        }
        let response = router
            .handle(post(
                &format!("{path}/elements/x/remove"),
                &[("cookie", who)],
                form_body(&[]),
            ))
            .await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "POST {path}/elements/x/remove"
        );
    }
    // Nothing was smuggled in.
    let html = page(&router, &owner, &editor).await;
    assert!(!html.contains("Smuggled"), "{html}");
    assert!(html.contains("The schema is empty."), "{html}");
}

#[tokio::test]
async fn the_editing_journey() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "schema-editor").await;
    let organization = create_organization(&router, &cookie, "Mairie").await;
    let editor = create_procedure(&router, &cookie, &organization, "Bourse").await;

    // The procedure page reaches the editor; the editor starts empty.
    let procedure_page = editor.trim_end_matches("/schema").to_owned();
    let html = page(&router, &cookie, &procedure_page).await;
    assert!(html.contains("No draft yet."), "{html}");
    assert!(html.contains(&format!("href=\"{editor}\"")), "{html}");
    let html = page(&router, &cookie, &editor).await;
    assert!(html.contains("Schema of Bourse"), "{html}");
    assert!(
        html.contains("No draft yet: adding an element starts one."),
        "{html}"
    );
    assert!(html.contains("data-schema-empty"), "{html}");
    assert!(html.contains("id=\"add-label\""), "{html}");
    assert!(!html.contains("Discard the draft"), "{html}");
    // The notice slot is there even with nothing to say (no jump).
    assert!(html.contains("data-schema-notices"), "{html}");
    assert!(!html.contains("data-schema-notice="), "{html}");

    // Add a column at the root: selected, listed, noticed, counted.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "column"), ("label", " Nom ")],
    )
    .await;
    let nom = selected_of(&to.to);
    let html = landed(&router, &cookie, &to).await;
    assert!(
        html.contains("Added the column \u{201c}Nom\u{201d}."),
        "{html}"
    );
    assert!(html.contains("role=\"status\""), "{html}");
    assert_eq!(element_ids(&html), std::slice::from_ref(&nom));
    assert!(html.contains("aria-current=\"true\""), "{html}");
    assert!(html.contains("Column: Nom"), "{html}");
    assert!(html.contains("1 column, 0 groups"), "{html}");
    assert!(html.contains("Discard the draft"), "{html}");
    assert!(
        html.contains(&format!("data-element-form=\"{nom}\"")),
        "{html}"
    );
    // With a row selected the add form is gone from the detail panel;
    // the structure heading's "Add an element" leads back to it.
    assert!(!html.contains("id=\"add-label\""), "{html}");
    assert!(html.contains(&format!("href=\"{editor}\"")), "{html}");
    assert!(html.contains("data-schema-add"), "{html}");
    let html = page(&router, &cookie, &editor).await;
    assert!(html.contains("id=\"add-label\""), "{html}");
    assert!(!html.contains("data-schema-add"), "{html}");

    // A group, then columns inside it: appended, then before an anchor.
    // The add form offers the column's type in the same step.
    assert!(html.contains("id=\"add-type\""), "{html}");
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[
            ("what", "column"),
            ("label", "Date de naissance"),
            ("kind", "DATE"),
        ],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains(">Date<"), "{html}");
    assert!(html.contains("value=\"DATE\" selected"), "{html}");
    let naissance = selected_of(&to.to);
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/elements/{naissance}/remove"),
        &[],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(element_ids(&html), std::slice::from_ref(&nom));
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "group"), ("label", "Adresse")],
    )
    .await;
    let adresse = selected_of(&to.to);
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("Group: Adresse"), "{html}");
    assert!(html.contains("Add inside this group"), "{html}");
    assert!(
        html.contains("Added the group \u{201c}Adresse\u{201d}."),
        "{html}"
    );
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "column"), ("label", "Rue"), ("parent", &adresse)],
    )
    .await;
    let rue = selected_of(&to.to);
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[
            ("what", "column"),
            ("label", "Ville"),
            ("parent", &adresse),
            ("before", &rue),
        ],
    )
    .await;
    let ville = selected_of(&to.to);
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [nom.clone(), adresse.clone(), ville.clone(), rue.clone()]
    );
    assert!(html.contains("3 columns, 1 group"), "{html}");

    // Move down, up, into the group, back to the top level.
    let relocate = |id: &str| format!("{editor}/elements/{id}/relocate");
    let to = act(&router, &cookie, &relocate(&nom), &[("direction", "down")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), ville.clone(), rue.clone(), nom.clone()]
    );
    assert!(html.contains("Moved \u{201c}Nom\u{201d}."), "{html}");
    let to = act(&router, &cookie, &relocate(&nom), &[("direction", "up")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [nom.clone(), adresse.clone(), ville.clone(), rue.clone()]
    );
    // At the edge, up is a no-op.
    let to = act(&router, &cookie, &relocate(&nom), &[("direction", "up")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(element_ids(&html)[0], nom);
    let to = act(&router, &cookie, &relocate(&nom), &[("parent", &adresse)]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), ville.clone(), rue.clone(), nom.clone()]
    );
    let to = act(&router, &cookie, &relocate(&nom), &[("parent", "")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), ville.clone(), rue.clone(), nom.clone()]
    );
    // The actions menu names each row and offers the group as a target.
    assert!(html.contains("aria-label=\"Actions for Nom\""), "{html}");
    assert!(html.contains("Move to"), "{html}");

    // Update: label, a decimal with a unit, many values.
    let update = |id: &str| format!("{editor}/elements/{id}/update");
    let to = act(
        &router,
        &cookie,
        &update(&nom),
        &[("label", " Surface "), ("kind", "DECIMAL"), ("unit", "m2")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("Column: Surface"), "{html}");
    assert!(html.contains("Decimal (m2)"), "{html}");
    assert!(html.contains("Saved your changes."), "{html}");
    assert!(html.contains("value=\"m2\" selected"), "{html}");
    // Many values is offered on choices, attachments and geometries
    // only: posted with a decimal it is ignored (the select is hidden,
    // not absent), on an attachment it sticks, and a type change back
    // takes it to one.
    let to = act(
        &router,
        &cookie,
        &update(&nom),
        &[("kind", "DECIMAL"), ("arity", "MANY")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("value=\"ONE\" selected"), "{html}");
    let to = act(
        &router,
        &cookie,
        &update(&nom),
        &[("kind", "ATTACHMENT"), ("arity", "MANY")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("value=\"MANY\" selected"), "{html}");
    assert!(html.contains(">Many<"), "{html}");
    let to = act(
        &router,
        &cookie,
        &update(&nom),
        &[("kind", "DECIMAL"), ("unit", "m2")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("value=\"ONE\" selected"), "{html}");

    // An enum starts with no options (a draft state — publication is
    // where an empty choice is refused); options are added, renamed
    // (the id kept), and removed through their own routes.
    let options = |id: &str, action: &str| format!("{editor}/elements/{id}/options/{action}");
    let to = act(&router, &cookie, &update(&ville), &[("kind", "ENUM")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("Choice"), "{html}");
    assert!(html.contains("data-options-empty"), "{html}");
    assert!(html.contains("id=\"element-option-new\""), "{html}");
    assert!(
        attribute_values(&html, "data-option-id").is_empty(),
        "{html}"
    );
    let to = act(
        &router,
        &cookie,
        &options(&ville, "add"),
        &[("label", " Paris ")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(
        html.contains("Added the option \u{201c}Paris\u{201d}."),
        "{html}"
    );
    let to = act(
        &router,
        &cookie,
        &options(&ville, "add"),
        &[("label", " Lyon ")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    let ids = attribute_values(&html, "data-option-id");
    assert_eq!(ids.len(), 2, "{html}");
    assert!(html.contains("value=\"Paris\""), "{html}");
    assert!(html.contains("value=\"Lyon\""), "{html}");
    assert!(html.contains("aria-label=\"Remove option Lyon\""), "{html}");
    let to = act(
        &router,
        &cookie,
        &options(&ville, "update"),
        &[("option_id", &ids[0]), ("label", "Paris (75)")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("value=\"Paris (75)\""), "{html}");
    assert_eq!(attribute_values(&html, "data-option-id")[0], ids[0]);
    let to = act(
        &router,
        &cookie,
        &options(&ville, "remove"),
        &[("option_id", &ids[1])],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(
        html.contains("Removed the option \u{201c}Lyon\u{201d}."),
        "{html}"
    );
    assert!(!html.contains("value=\"Lyon\""), "{html}");
    // The last option can go too: an empty choice is a draft state.
    let to = act(
        &router,
        &cookie,
        &options(&ville, "remove"),
        &[("option_id", &ids[0])],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("data-options-empty"), "{html}");
    assert!(!html.contains("value=\"Paris (75)\""), "{html}");
    let to = act(
        &router,
        &cookie,
        &options(&ville, "add"),
        &[("label", "  ")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("A label is required."), "{html}");

    // A group's cardinality; a blank label is refused with an alert.
    let to = act(
        &router,
        &cookie,
        &update(&adresse),
        &[("cardinality", "MANY")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("value=\"MANY\" selected"), "{html}");
    let to = act(&router, &cookie, &update(&adresse), &[("label", "   ")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("role=\"alert\""), "{html}");
    assert!(html.contains("A label is required."), "{html}");
    assert!(html.contains("Group: Adresse"), "{html}");

    // A `many` group inside a `many` group: the kernel refuses, the
    // draft is unchanged, the reason is shown.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "group"), ("label", "Lignes"), ("parent", &adresse)],
    )
    .await;
    let lignes = selected_of(&to.to);
    let to = act(
        &router,
        &cookie,
        &update(&lignes),
        &[("cardinality", "MANY")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("role=\"alert\""), "{html}");
    assert!(html.contains("The change was refused:"), "{html}");
    assert!(html.contains("value=\"ONE\" selected"), "{html}");

    // Remove a column (selection falls back to its parent), then the
    // group with its subtree.
    let remove = |id: &str| format!("{editor}/elements/{id}/remove");
    let to = act(&router, &cookie, &remove(&rue), &[]).await;
    assert_eq!(selected_of(&to.to), adresse);
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("Removed \u{201c}Rue\u{201d}."), "{html}");
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), ville.clone(), lignes.clone(), nom.clone()]
    );
    let to = act(&router, &cookie, &remove(&adresse), &[]).await;
    assert!(!to.to.contains("selected="), "{}", to.to);
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(element_ids(&html), std::slice::from_ref(&nom));

    // Discard: a confirmation state first, then the draft is gone.
    let html = page(&router, &cookie, &format!("{editor}?discard=confirm")).await;
    assert!(html.contains("role=\"alertdialog\""), "{html}");
    assert!(html.contains("Discard the whole draft?"), "{html}");
    assert!(html.contains("Keep editing"), "{html}");
    let to = act(&router, &cookie, &format!("{editor}/discard"), &[]).await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("The draft has been discarded."), "{html}");
    assert!(html.contains("data-schema-empty"), "{html}");
    assert!(html.contains("No draft yet"), "{html}");
}

#[tokio::test]
async fn the_editor_speaks_french() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = french_member(&router, "schema-fr").await;
    let organization = create_organization(&router, &cookie, "Mairie").await;
    let editor = create_procedure(&router, &cookie, &organization, "Bourse").await;
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "column"), ("label", "Nom")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(
        html.contains("La colonne «\u{a0}Nom\u{a0}» a été ajoutée."),
        "{html}"
    );
    assert!(html.contains("lang=\"fr\""), "{html}");
    assert!(html.contains("Schéma de Bourse"), "{html}");
    assert!(html.contains("Colonne\u{a0}: Nom"), "{html}");
    assert!(html.contains("Actions pour Nom"), "{html}");
    assert!(html.contains("Abandonner le brouillon"), "{html}");
    // CLDR plural rules, not `(s)`: French "one" covers 0 and 1.
    assert!(html.contains("1 colonne, 0 groupe \u{2014}"), "{html}");
}
