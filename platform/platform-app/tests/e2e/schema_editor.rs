//! Subject: the schema editor in a real browser — what the router
//! tests cannot prove. Two journeys: **keyboard** (a row's actions
//! menu reached and driven by keys alone, *Move down* reordering the
//! structure) and **autosave** (the runtime path: a label changed and
//! blurred is saved through the procedure, the status line reports
//! it, and the structure panel — a shard — shows the new label with
//! no navigation; changing the kind hides and shows the
//! kind-dependent fields).

use playwright_rs::protocol::{AriaRole, Browser, BrowserContext, GetByRoleOptions, Page};
use playwright_rs::{expect, expect_page, locator};

use crate::harness::{
    App, TestResult, accepts_secure_cookie_on_loopback_http, browser_signup, default_context,
    run_scenario, unique_email,
};

type Outcome<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread")]
async fn keyboard_moves_an_element_down() {
    run_scenario("schema-keyboard", default_context, keyboard_scenario).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn autosave_updates_the_structure_without_navigating() {
    run_scenario("schema-autosave", default_context, autosave_scenario).await;
}

/// Signs up, creates an organization and a procedure, opens the
/// editor, and adds `labels` as root columns; returns the editor URL.
async fn editor_with_columns(
    page: &Page,
    app: &App,
    tag: &str,
    labels: &[&str],
) -> Outcome<String> {
    page.goto(&app.url("/organizations"), None).await?;
    page.locator(locator!("#organization-name"))
        .fill("Mairie", None)
        .await?;
    page.locator(locator!("#organization-slug"))
        .fill(&format!("{tag}-{}", uuid::Uuid::new_v4()), None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(
            GetByRoleOptions::default()
                .name("Create organization")
                .exact(true),
        ),
    )
    .click(None)
    .await?;
    page.get_by_role(
        AriaRole::Link,
        Some(
            GetByRoleOptions::default()
                .name("Manage procedures")
                .exact(true),
        ),
    )
    .click(None)
    .await?;
    page.locator(locator!("#procedure-title"))
        .fill("Bourse", None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(
            GetByRoleOptions::default()
                .name("Create procedure")
                .exact(true),
        ),
    )
    .click(None)
    .await?;
    page.locator(locator!("li[data-procedure-id] a"))
        .click(None)
        .await?;
    page.get_by_role(
        AriaRole::Link,
        Some(
            GetByRoleOptions::default()
                .name("Edit the schema")
                .exact(true),
        ),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("#add-label")))
        .to_be_visible()
        .await?;
    let editor = page.url();
    for label in labels {
        page.goto(&editor, None).await?;
        page.locator(locator!("#add-label"))
            .fill(label, None)
            .await?;
        page.get_by_role(
            AriaRole::Button,
            Some(GetByRoleOptions::default().name("Add").exact(true)),
        )
        .click(None)
        .await?;
        expect(page.locator(locator!("li[data-element-id] a[aria-current='true']")))
            .to_have_text(label)
            .await?;
    }
    Ok(editor)
}

/// The labels of the structure panel's rows, document order.
async fn row_labels(page: &Page) -> Outcome<Vec<String>> {
    Ok(page
        .locator(locator!("li[data-element-id] > div > a"))
        .all_inner_texts()
        .await?)
}

async fn keyboard_scenario(
    _browser: &Browser,
    context: &BrowserContext,
    app: &App,
    engine: &'static str,
) -> TestResult {
    let page = context.new_page().await?;
    let email = unique_email(&format!("e2e-schema-keys-{engine}"));
    browser_signup(&page, app, "Aurélie", &email).await?;
    if !accepts_secure_cookie_on_loopback_http(engine) {
        println!(
            "[{engine}] Secure session cookie refused over http://127.0.0.1; \
             the editor journey cannot run here"
        );
        return Ok(());
    }
    let editor = editor_with_columns(&page, app, "keys", &["Nom", "Prénom"]).await?;
    page.goto(&editor, None).await?;
    assert_eq!(row_labels(&page).await?, ["Nom", "Prénom"]);

    // Focus the first row's actions trigger, open it, walk to "Move
    // down" (the second item — "Move up" is disabled on the first row
    // and skipped by Tab), and activate it.
    page.locator(locator!(
        "li[data-element-id] summary[aria-label='Actions for Nom']"
    ))
    .focus()
    .await?;
    let keyboard = page.keyboard();
    keyboard.press("Enter", None).await?;
    expect(page.locator(locator!("li[data-element-id] details[open]")))
        .to_be_visible()
        .await?;
    keyboard.press("Tab", None).await?;
    // `to_be_focused` takes a page-level CSS selector only (harness
    // docs): the second form in the open menu is "Move down".
    expect(page.locator(locator!(
        "li[data-element-id] details[open] form:nth-of-type(2) button[type='submit']"
    )))
    .to_be_focused()
    .await?;
    keyboard.press("Enter", None).await?;
    expect(page.locator(locator!("[data-schema-notice]")))
        .to_have_text("Moved \u{201c}Nom\u{201d}.")
        .await?;
    assert_eq!(row_labels(&page).await?, ["Prénom", "Nom"]);
    Ok(())
}

async fn autosave_scenario(
    _browser: &Browser,
    context: &BrowserContext,
    app: &App,
    engine: &'static str,
) -> TestResult {
    let page = context.new_page().await?;
    let email = unique_email(&format!("e2e-schema-autosave-{engine}"));
    browser_signup(&page, app, "Aurélie", &email).await?;
    if !accepts_secure_cookie_on_loopback_http(engine) {
        println!(
            "[{engine}] Secure session cookie refused over http://127.0.0.1; \
             the editor journey cannot run here"
        );
        return Ok(());
    }
    let _editor = editor_with_columns(&page, app, "autosave", &["Nom"]).await?;
    // The last add left the column selected.
    let url_before = page.url();
    expect(page.locator(locator!("#element-label")))
        .to_have_value("Nom")
        .await?;

    // Change the label and blur: the procedure saves, the status line
    // says so, the structure's row (the shard) shows the new label —
    // and the URL has not changed.
    page.locator(locator!("#element-label"))
        .fill("Nom de famille", None)
        .await?;
    page.keyboard().press("Tab", None).await?;
    expect(page.locator(locator!("[data-save-status]")))
        .to_have_text("Saved your changes.")
        .await?;
    expect(page.locator(locator!("li[data-element-id] a[aria-current='true']")))
        .to_have_text("Nom de famille")
        .await?;
    expect_page(&page).to_have_url(url_before.as_str()).await?;

    // The kind decides which facets show: a text column hides the
    // unit; switching to a decimal reveals it (and saves the kind).
    expect(page.locator(locator!("[data-facet='unit']")))
        .to_be_hidden()
        .await?;
    page.locator(locator!("#element-kind"))
        .select_option("DECIMAL", None)
        .await?;
    expect(page.locator(locator!("[data-facet='unit']")))
        .to_be_visible()
        .await?;
    expect(page.locator(locator!("[data-save-status]")))
        .to_have_text("Saved your changes.")
        .await?;
    page.locator(locator!("#element-unit"))
        .select_option("m2", None)
        .await?;
    expect(page.locator(locator!("[data-save-status]")))
        .to_have_text("Saved your changes.")
        .await?;
    expect(page.locator(locator!("li[data-element-id]")))
        .to_contain_text("Decimal (m2)")
        .await?;

    // A reload shows what was stored.
    page.reload(None).await?;
    expect(page.locator(locator!("#element-label")))
        .to_have_value("Nom de famille")
        .await?;
    expect(page.locator(locator!("#element-unit")))
        .to_have_value("m2")
        .await?;
    Ok(())
}
