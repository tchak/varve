//! Subject: the schema editor of a procedure's revision draft —
//! the gate (sign-in redirect, the one-404 rule for strangers on GET
//! and POST), the empty state, adding columns and groups at the root
//! and inside a group (before an anchor), moving up / down / to,
//! updating label, type with unit, enum options (kept and minted
//! ids, a blank row removed), arity and cardinality, the refused
//! edits as alerts (blank label, a `many` group inside a `many`
//! group), removing, discarding behind its confirmation, sections
//! and notes with inherited audiences (the reviewer badge, the
//! refused widening), the preview tab (the draft rendered as a
//! read-only form, its gate, French), publishing (the two-phase
//! `publishRevision`: free first publication, the confirmation
//! carrying the impact report for a checked cast, the refused
//! draftless publish), and French.
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
        for action in ["add", "discard", "publish"] {
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
    // Top / bottom / after: the order is [Adresse, Ville, Rue, Nom] at
    // the root with Ville and Rue inside Adresse.
    let to = act(&router, &cookie, &relocate(&nom), &[("direction", "top")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [nom.clone(), adresse.clone(), ville.clone(), rue.clone()]
    );
    let to = act(
        &router,
        &cookie,
        &relocate(&nom),
        &[("direction", "bottom")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), ville.clone(), rue.clone(), nom.clone()]
    );
    let to = act(&router, &cookie, &relocate(&rue), &[("after", &nom)]).await;
    let html = landed(&router, &cookie, &to).await;
    // `after` a sibling at another level is ignored (no-op landing).
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), ville.clone(), rue.clone(), nom.clone()]
    );
    let to = act(&router, &cookie, &relocate(&rue), &[("after", &ville)]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), ville.clone(), rue.clone(), nom.clone()]
    );
    let to = act(&router, &cookie, &relocate(&ville), &[("after", &rue)]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), rue.clone(), ville.clone(), nom.clone()]
    );
    let to = act(&router, &cookie, &relocate(&ville), &[("after", &rue)]).await;
    let html = landed(&router, &cookie, &to).await;
    // Already right after it: unchanged.
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), rue.clone(), ville.clone(), nom.clone()]
    );
    // Menus only offer what applies: a lone child has no "move after"
    // (a disabled item, no submenu), and an element with nowhere to
    // go has a disabled "move to".
    let to = act(&router, &cookie, &relocate(&ville), &[("parent", "")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), rue.clone(), nom.clone(), ville.clone()]
    );
    let rue_row = html
        .split(&format!("data-element-id=\"{rue}\""))
        .nth(1)
        .unwrap();
    let rue_row = &rue_row[..rue_row.find("</li>").unwrap()];
    let move_after = rue_row
        .split("<button")
        .find(|tag| tag.contains("data-menu=\"move-after\""))
        .expect("a move-after item");
    assert!(move_after.contains("disabled=\"\""), "{move_after}");
    assert!(
        !rue_row.contains("<details class=\"group/sub relative\" data-menu=\"move-after\""),
        "{rue_row}"
    );
    // Its "move to" still opens: the top level is a destination.
    assert!(rue_row.contains("data-menu=\"move-to\""), "{rue_row}");
    let to = act(&router, &cookie, &relocate(&ville), &[("parent", &adresse)]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [adresse.clone(), rue.clone(), ville.clone(), nom.clone()]
    );

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
async fn publish_journey() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "schema-publish").await;
    let organization = create_organization(&router, &cookie, "Préfecture").await;
    let editor = create_procedure(&router, &cookie, &organization, "Permis").await;

    // No draft: publication is refused, as an alert with the
    // server's reason (`INVALID_DRAFT`).
    let to = act(&router, &cookie, &format!("{editor}/publish"), &[]).await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("role=\"alert\""), "{html}");
    assert!(html.contains("Publication was refused:"), "{html}");

    // A draft with one text column; the header offers publication.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "column"), ("label", "Nom")],
    )
    .await;
    let nom = selected_of(&to.to);
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("Publish the revision"), "{html}");

    // The confirmation state is a URL; a first draft classifies
    // against the empty schema, so its report is all additions.
    let html = page(&router, &cookie, &format!("{editor}?publish=confirm")).await;
    assert!(html.contains("role=\"alertdialog\""), "{html}");
    assert!(html.contains("data-impact-report"), "{html}");
    assert!(
        html.contains("\u{201c}Nom\u{201d} is added \u{2014} no impact on existing answers."),
        "{html}"
    );
    assert!(html.contains("Confirm and publish"), "{html}");

    // First publication: every column `ADDED`, free — it publishes
    // without a confirmation (G.10) and consumes the draft. What
    // remains is the pristine virtual draft (G.7): the published
    // schema stays visible in the structure, the state line says so,
    // and the draft-only actions are gone.
    let to = act(&router, &cookie, &format!("{editor}/publish"), &[]).await;
    assert!(!to.to.contains("publish=confirm"), "{}", to.to);
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("The revision has been published."), "{html}");
    assert!(
        html.contains("Published schema \u{2014} editing starts a new draft."),
        "{html}"
    );
    assert_eq!(element_ids(&html), std::slice::from_ref(&nom));
    assert!(!html.contains("Publish the revision"), "{html}");
    assert!(!html.contains("Discard the draft"), "{html}");

    // Editing the published column directly forks the next draft
    // from the head ("Nom" keeps its id — no add needed first), and
    // text → integer is a `CHECKED` cast: an unconfirmed publish
    // writes nothing and lands on the confirmation, which shows the
    // read-time report.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/elements/{nom}/update"),
        &[("kind", "INTEGER")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("Saved your changes."), "{html}");
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "column"), ("label", "Ville")],
    )
    .await;
    landed(&router, &cookie, &to).await;
    let to = act(&router, &cookie, &format!("{editor}/publish"), &[]).await;
    assert!(to.to.contains("publish=confirm"), "{}", to.to);
    assert!(to.notice.is_none());
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("role=\"alertdialog\""), "{html}");
    assert!(
        html.contains(
            "\u{201c}Nom\u{201d} changes type \u{2014} \
             existing answers will be checked against the new type."
        ),
        "{html}"
    );
    assert!(
        html.contains("\u{201c}Ville\u{201d} is added \u{2014} no impact on existing answers."),
        "{html}"
    );
    // The confirmation replaces the header's actions.
    assert!(!html.contains("Publish the revision"), "{html}");
    assert!(!html.contains("Discard the draft"), "{html}");
    assert!(html.contains("Keep editing"), "{html}");

    // Confirmed: the checked cast publishes and the draft is gone.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/publish"),
        &[("confirm", "true")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("The revision has been published."), "{html}");
    assert!(
        html.contains("Published schema \u{2014} editing starts a new draft."),
        "{html}"
    );
}

#[tokio::test]
async fn publishes_in_french() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = french_member(&router, "schema-publish-fr").await;
    let organization = create_organization(&router, &cookie, "Mairie").await;
    let editor = create_procedure(&router, &cookie, &organization, "Aide").await;
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "column"), ("label", "Nom")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("Publier la révision"), "{html}");
    let html = page(&router, &cookie, &format!("{editor}?publish=confirm")).await;
    assert!(
        html.contains("«\u{a0}Nom\u{a0}» est ajoutée \u{2014} sans impact"),
        "{html}"
    );
    assert!(html.contains("Confirmer et publier"), "{html}");
    let to = act(&router, &cookie, &format!("{editor}/publish"), &[]).await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("La révision a été publiée."), "{html}");
}

#[tokio::test]
async fn sections_notes_and_audience_journey() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "schema-sections").await;
    let organization = create_organization(&router, &cookie, "Mairie").await;
    let editor = create_procedure(&router, &cookie, &organization, "Aide").await;

    // The add form offers all four kinds.
    let html = page(&router, &cookie, &editor).await;
    assert!(html.contains("value=\"section\""), "{html}");
    assert!(html.contains("value=\"note\""), "{html}");

    // Add a section: selected, badged as a section, its detail form
    // with title, help and audience, and the add form now targets it.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "section"), ("label", " Identité ")],
    )
    .await;
    let section = selected_of(&to.to);
    let html = landed(&router, &cookie, &to).await;
    assert!(
        html.contains("Added the section \u{201c}Identité\u{201d}."),
        "{html}"
    );
    assert!(html.contains("Section: Identité"), "{html}");
    assert!(html.contains("data-element-kind=\"section\""), "{html}");
    assert!(html.contains("id=\"element-title\""), "{html}");
    assert!(html.contains("id=\"element-help\""), "{html}");
    assert!(html.contains("id=\"element-audience\""), "{html}");
    assert!(html.contains("Add inside this section"), "{html}");

    // A column inside the section, then a note (its text is the add
    // form's one field).
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "column"), ("label", "Nom"), ("parent", &section)],
    )
    .await;
    let nom = selected_of(&to.to);
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[
            ("what", "note"),
            ("label", "Vérifier la pièce."),
            ("parent", &section),
        ],
    )
    .await;
    let note = selected_of(&to.to);
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("Added the note."), "{html}");
    assert!(html.contains("data-element-kind=\"note\""), "{html}");
    assert_eq!(
        element_ids(&html),
        [section.clone(), nom.clone(), note.clone()]
    );
    assert!(html.contains("id=\"element-body\""), "{html}");
    assert!(html.contains("Vérifier la pièce."), "{html}");

    // A blank section title is refused with its own message.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "section"), ("label", "  ")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("A title is required."), "{html}");

    // Narrow the section to reviewers: audience is inherited, so
    // every row under it wears the badge.
    let update = |id: &str| format!("{editor}/elements/{id}/update");
    let to = act(
        &router,
        &cookie,
        &update(&section),
        &[("audience", "REVIEWER")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("value=\"REVIEWER\" selected"), "{html}");
    assert_eq!(
        html.match_indices("data-audience=\"reviewer\"").count(),
        3,
        "{html}"
    );
    assert!(html.contains("Reviewers only"), "{html}");

    // Explicitly widening a child beyond its parent is the refused
    // contradiction (P.4), shown as the editor's alert.
    //
    // This is the invariant the *editor* leans on: `nom` sits under a
    // reviewer-only section, so its audience select is not rendered
    // at all — and the refusal is what makes that a safe thing to do
    // rather than a client-side check. The field arrives here the way
    // a spoofed one would (a POST naming a control the form never
    // drew), and the kernel is what says no. Pinned by the reason, so
    // that a refusal arriving from anywhere else fails the test.
    let nom_html = page(&router, &cookie, &format!("{editor}?selected={nom}")).await;
    assert!(!nom_html.contains("id=\"element-audience\""), "{nom_html}");
    let to = act(&router, &cookie, &update(&nom), &[("audience", "ALL")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("role=\"alert\""), "{html}");
    assert!(html.contains("The change was refused:"), "{html}");
    assert!(
        html.contains("cannot be wider than its parent's audience"),
        "{html}"
    );
    // And nothing was half-applied: the column still reads
    // reviewer-only, as do the section and the note beside it.
    assert_eq!(
        html.match_indices("data-audience=\"reviewer\"").count(),
        3,
        "{html}"
    );

    // Section help is set and cleared; a retitle keeps the identity.
    let to = act(
        &router,
        &cookie,
        &update(&section),
        &[("title", "État civil"), ("help", "Vos informations")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("Section: État civil"), "{html}");
    assert!(html.contains("value=\"Vos informations\""), "{html}");
    assert_eq!(element_ids(&html)[0], section);
    let to = act(&router, &cookie, &update(&section), &[("help", "")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert!(!html.contains("value=\"Vos informations\""), "{html}");

    // A section is a "move to" destination; landing inside the
    // reviewer-only section badges the element, moving out unbadges.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "column"), ("label", "Ville")],
    )
    .await;
    let ville = selected_of(&to.to);
    let relocate = |id: &str| format!("{editor}/elements/{id}/relocate");
    let to = act(&router, &cookie, &relocate(&ville), &[("parent", &section)]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        element_ids(&html),
        [section.clone(), nom.clone(), note.clone(), ville.clone()]
    );
    assert_eq!(
        html.match_indices("data-audience=\"reviewer\"").count(),
        4,
        "{html}"
    );
    let to = act(&router, &cookie, &relocate(&ville), &[("parent", "")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        html.match_indices("data-audience=\"reviewer\"").count(),
        3,
        "{html}"
    );

    // Removing the section takes its subtree.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/elements/{section}/remove"),
        &[],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(element_ids(&html), std::slice::from_ref(&ville));

    // The add form offers the audience at the root…
    let html = page(&router, &cookie, &editor).await;
    assert!(html.contains("id=\"add-audience\""), "{html}");
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[
            ("what", "column"),
            ("label", "Notes internes"),
            ("audience", "REVIEWER"),
        ],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert_eq!(
        html.match_indices("data-audience=\"reviewer\"").count(),
        1,
        "{html}"
    );

    // …but not inside a reviewer-only container, where the audience
    // cannot change: the container's own select stays (its parent is
    // the root), its add form drops the field, and a child's detail
    // form drops the select.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "group"), ("label", "Interne")],
    )
    .await;
    let interne = selected_of(&to.to);
    let to = act(
        &router,
        &cookie,
        &update(&interne),
        &[("audience", "REVIEWER")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("id=\"element-audience\""), "{html}");
    assert!(
        !html.contains(&format!("id=\"add-{interne}-audience\"")),
        "{html}"
    );
    // Adding into it: the add form draws no audience field here, and
    // what a widened one posted anyway would do is the kernel's to
    // decide — it *clamps* on add rather than refusing (P.4). The
    // clamp is not observable from here (the element's own marker
    // never reaches the page; every row under a reviewer-only
    // container reads reviewer either way, by inheritance), so it is
    // pinned where it can be seen:
    // `platform_core::tree_edit::audience_clamps_on_add_and_refuses_widening`.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[
            ("what", "column"),
            ("label", "Détail"),
            ("parent", &interne),
        ],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(!html.contains("id=\"element-audience\""), "{html}");

    // Required rides the column detail as a switch: born reviewer-only
    // means optional by default…
    assert!(html.contains("id=\"element-required\""), "{html}");
    assert!(html.contains("data-required=\"false\""), "{html}");
    // …born public means required by default, and the switch toggles
    // it (the hidden `false` is overridden by the checked value).
    let html = page(&router, &cookie, &format!("{editor}?selected={ville}")).await;
    assert!(html.contains("data-required=\"true\""), "{html}");
    assert!(html.contains("checked"), "{html}");
    let to = act(
        &router,
        &cookie,
        &update(&ville),
        &[("required", "false"), ("required", "true")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("data-required=\"true\""), "{html}");
    let to = act(&router, &cookie, &update(&ville), &[("required", "false")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("data-required=\"false\""), "{html}");
    assert!(html.contains("Saved your changes."), "{html}");

    // A text format: a built-in, then a custom pattern; a
    // backtracking pattern is refused (draft unchanged), a blank one
    // asks for the pattern.
    assert!(html.contains("data-facet=\"format\""), "{html}");
    assert!(html.contains("id=\"element-format\""), "{html}");
    let to = act(&router, &cookie, &update(&ville), &[("format", "EMAIL")]).await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("value=\"EMAIL\" selected"), "{html}");
    let to = act(
        &router,
        &cookie,
        &update(&ville),
        &[("format", "REGEX"), ("pattern", "[0-9]{5}")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("value=\"[0-9]{5}\""), "{html}");
    let to = act(
        &router,
        &cookie,
        &update(&ville),
        &[("format", "REGEX"), ("pattern", "(?=x)")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("role=\"alert\""), "{html}");
    assert!(html.contains("value=\"[0-9]{5}\""), "{html}");
    let to = act(
        &router,
        &cookie,
        &update(&ville),
        &[("format", "REGEX"), ("pattern", "  ")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("A pattern is required"), "{html}");
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

    // The tree side in French too: a section with its audience.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "section"), ("label", "Identité")],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(
        html.contains("La section «\u{a0}Identité\u{a0}» a été ajoutée."),
        "{html}"
    );
    assert!(html.contains("Section\u{a0}: Identité"), "{html}");
    assert!(html.contains("Visibilité"), "{html}");
    assert!(html.contains("Instructeurs uniquement"), "{html}");
}

/// Regression: rendering deeply nested containers must not overflow
/// the stack. `tree_list` and `tree_row` recurse into each other per
/// nesting level with large render frames; both are boxed, and this
/// journey proves six levels — sections nest without a kernel depth
/// bound (the depth policy binds `many` groups only). The original
/// report was a group moved into a reviewer-only section inside a
/// section: SIGABRT on the move's landing page and on every load
/// after.
#[tokio::test]
async fn deeply_nested_containers_render() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "schema-repro").await;
    let organization = create_organization(&router, &cookie, "Mairie").await;
    let editor = create_procedure(&router, &cookie, &organization, "Repro").await;

    // Five nested sections, the innermost reviewer-only.
    let mut parent: Option<String> = None;
    let mut innermost = String::new();
    for level in 1..=5 {
        let label = format!("Niveau {level}");
        let mut fields = vec![("what", "section".to_owned()), ("label", label)];
        if let Some(parent) = &parent {
            fields.push(("parent", parent.clone()));
        }
        let fields: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let to = act(&router, &cookie, &format!("{editor}/add"), &fields).await;
        innermost = selected_of(&to.to);
        parent = Some(innermost.clone());
    }
    let update = |id: &str| format!("{editor}/elements/{id}/update");
    act(
        &router,
        &cookie,
        &update(&innermost),
        &[("audience", "REVIEWER")],
    )
    .await;

    // A group with a column at the root, moved into the innermost
    // section — the reported crash, two levels deeper.
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[("what", "group"), ("label", "Private group")],
    )
    .await;
    let group = selected_of(&to.to);
    act(
        &router,
        &cookie,
        &format!("{editor}/add"),
        &[
            ("what", "column"),
            ("label", "some text"),
            ("parent", &group),
        ],
    )
    .await;
    let to = act(
        &router,
        &cookie,
        &format!("{editor}/elements/{group}/relocate"),
        &[("parent", &innermost)],
    )
    .await;
    let html = landed(&router, &cookie, &to).await;
    assert!(html.contains("data-element-kind=\"group\""), "{html}");
    // The whole chain wears the inherited reviewer badge from the
    // innermost section down.
    assert!(html.contains("data-audience=\"reviewer\""), "{html}");
    let html = page(&router, &cookie, &editor).await;
    assert!(html.contains("Private group"), "{html}");
    assert!(html.contains("Niveau 5"), "{html}");
}

#[tokio::test]
async fn the_preview_tab() {
    let Some((router, _db)) = test_app().await else {
        return;
    };
    let cookie = member(&router, "preview-owner").await;
    let stranger = member(&router, "preview-stranger").await;
    let organization = create_organization(&router, &cookie, "Preview").await;
    let editor = create_procedure(&router, &cookie, &organization, "Aperçu").await;
    let preview = format!("{editor}/preview");

    // The gate matches the editor's: sign-in for the anonymous, one
    // 404 for strangers.
    let response = router.handle(get(&preview, &[])).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/signin");
    let response = router.handle(get(&preview, &[("cookie", &stranger)])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // The tabs link the two pages; with no draft the preview says so.
    let html = page(&router, &cookie, &editor).await;
    assert!(html.contains(">Preview<"), "{html}");
    assert!(html.contains("/schema/preview\""), "{html}");
    let html = page(&router, &cookie, &preview).await;
    assert!(html.contains("aria-current=\"page\""), "{html}");
    assert!(html.contains(">Editor<"), "{html}");
    assert!(html.contains("data-preview-empty"), "{html}");

    // A draft: a section holding a text column (public, so required
    // by default) and a note; a reviewer-only date column; a `many`
    // group holding a choice column with one option; a geometry
    // column.
    let add = format!("{editor}/add");
    let update = |id: &str| format!("{editor}/elements/{id}/update");
    let to = act(
        &router,
        &cookie,
        &add,
        &[("what", "section"), ("label", "Identité")],
    )
    .await;
    let section = selected_of(&to.to);
    let to = act(
        &router,
        &cookie,
        &add,
        &[("what", "column"), ("label", "Nom"), ("parent", &section)],
    )
    .await;
    let nom = selected_of(&to.to);
    act(
        &router,
        &cookie,
        &add,
        &[
            ("what", "note"),
            ("label", "Vérifier la pièce."),
            ("parent", &section),
        ],
    )
    .await;
    act(
        &router,
        &cookie,
        &add,
        &[
            ("what", "column"),
            ("label", "Avis"),
            ("kind", "DATE"),
            ("audience", "REVIEWER"),
        ],
    )
    .await;
    let to = act(
        &router,
        &cookie,
        &add,
        &[("what", "group"), ("label", "Enfants")],
    )
    .await;
    let group = selected_of(&to.to);
    act(
        &router,
        &cookie,
        &update(&group),
        &[("cardinality", "MANY")],
    )
    .await;
    let to = act(
        &router,
        &cookie,
        &add,
        &[
            ("what", "column"),
            ("label", "Ville"),
            ("kind", "ENUM"),
            ("parent", &group),
        ],
    )
    .await;
    let ville = selected_of(&to.to);
    act(
        &router,
        &cookie,
        &format!("{editor}/elements/{ville}/options/add"),
        &[("label", "Paris")],
    )
    .await;
    act(
        &router,
        &cookie,
        &add,
        &[("what", "column"), ("label", "Zone"), ("kind", "GEOMETRY")],
    )
    .await;

    let html = page(&router, &cookie, &preview).await;
    // The section is a heading; its column a labelled text control
    // carrying the required default.
    assert!(html.contains("<h3"), "{html}");
    assert!(html.contains("Identité"), "{html}");
    assert!(html.contains(&format!("for=\"preview-{nom}\"")), "{html}");
    let at = html
        .find(&format!("id=\"preview-{nom}\""))
        .expect("the text control");
    let tag_start = html[..at].rfind("<input").expect("an input tag");
    let tag = &html[tag_start..tag_start + html[tag_start..].find('>').unwrap()];
    assert!(tag.contains("type=\"text\""), "{tag}");
    assert!(tag.contains("required"), "{tag}");
    // The note's body renders; the reviewer-only column wears the
    // badge and its native date input.
    assert!(html.contains("Vérifier la pièce."), "{html}");
    assert!(html.contains("Reviewers only"), "{html}");
    assert!(html.contains("type=\"date\""), "{html}");
    // The group is a fieldset with the `many` badge; its choice is a
    // select with the blank option and the authored one.
    assert!(html.contains("<fieldset"), "{html}");
    assert!(html.contains("Enfants"), "{html}");
    assert!(html.contains(">Many<"), "{html}");
    assert!(html.contains("<option value=\"\">"), "{html}");
    assert!(html.contains(">Paris<"), "{html}");
    // Geometry has no control: the caption is plain text and the gap
    // is stated.
    assert!(html.contains("Zone"), "{html}");
    assert!(html.contains("data-preview-geometry"), "{html}");
    assert!(html.contains("not shown in the preview"), "{html}");

    // French: the tabs and the empty state localize.
    let french = french_member(&router, "preview-french").await;
    let organization = create_organization(&router, &french, "Aperçu FR").await;
    let editor_fr = create_procedure(&router, &french, &organization, "Titre").await;
    let html = page(&router, &french, &format!("{editor_fr}/preview")).await;
    assert!(html.contains(">Aperçu<"), "{html}");
    assert!(html.contains(">Édition<"), "{html}");
    assert!(html.contains("Rien à prévisualiser"), "{html}");
}
