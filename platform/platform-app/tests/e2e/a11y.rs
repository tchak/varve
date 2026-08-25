//! Subject: accessibility in a real browser (design/platform.md P.1.5) —
//! the axe-core rule engine over every page the app renders, in
//! both locales and in the states a journey reaches (validation
//! errors, the open account menu), and the keyboard journey through
//! the header: what `Router::handle` plus the static baseline lint
//! in `tests/app` cannot prove (computed names and roles, contrast,
//! focus order, key activation).
//!
//! Every page the app gains is added to the sweep here; a page left
//! out is a page unchecked.

use playwright_rs::protocol::{AriaRole, Browser, BrowserContext, GetByRoleOptions, Page};
use playwright_rs::{expect, expect_page, locator};

use crate::harness::{
    App, TestResult, accepts_secure_cookie_on_loopback_http, browser_signup, check_axe,
    default_context, french_context, run_scenario, unique_email,
};

#[tokio::test(flavor = "multi_thread")]
async fn every_page_passes_axe() {
    run_scenario("a11y-axe", default_context, axe_scenario).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn anonymous_pages_pass_axe_in_french() {
    run_scenario("a11y-axe-fr", french_context, anonymous_axe_scenario).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keyboard_drives_the_account_menu() {
    run_scenario("a11y-keyboard", default_context, keyboard_scenario).await;
}

/// The pages reachable without a session, plus the sign-in form in
/// its error state.
async fn anonymous_sweep(page: &Page, app: &App) -> TestResult {
    for path in ["/", "/signin", "/signup", "/no-such-page"] {
        page.goto(&app.url(path), None).await?;
        check_axe(page, path).await?;
    }

    page.goto(&app.url("/signin"), None).await?;
    page.locator(locator!("#signin-email"))
        .fill("nobody@example.test", None)
        .await?;
    page.locator(locator!("#signin-password"))
        .fill("wrong", None)
        .await?;
    page.locator(locator!("form button[type='submit']"))
        .click(None)
        .await?;
    expect(page.locator(locator!("[role='alert']")))
        .to_be_visible()
        .await?;
    check_axe(page, "/signin (failed attempt)").await?;
    Ok(())
}

async fn anonymous_axe_scenario(
    _browser: &Browser,
    context: &BrowserContext,
    app: &App,
    _engine: &'static str,
) -> TestResult {
    let page = context.new_page().await?;
    anonymous_sweep(&page, app).await
}

async fn axe_scenario(
    _browser: &Browser,
    context: &BrowserContext,
    app: &App,
    engine: &'static str,
) -> TestResult {
    let page = context.new_page().await?;
    anonymous_sweep(&page, app).await?;

    let email = unique_email(&format!("e2e-a11y-{engine}"));
    browser_signup(&page, app, "Aurélie", &email).await?;
    if !accepts_secure_cookie_on_loopback_http(engine) {
        println!(
            "[{engine}] Secure session cookie refused over http://127.0.0.1; \
             the signed-in sweep cannot run here"
        );
        return Ok(());
    }

    // Signed-in home, with the account menu closed and then open:
    // the open panel is what a keyboard or screen-reader user meets.
    page.goto(&app.url("/"), None).await?;
    check_axe(&page, "/ (signed in)").await?;
    account_menu_trigger(&page).click(None).await?;
    expect(page.locator(locator!("header details[open]")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/ (account menu open)").await?;

    for path in ["/settings/account", "/settings/security"] {
        page.goto(&app.url(path), None).await?;
        check_axe(&page, path).await?;
    }

    // The profile form in its error state: a blank name rerenders
    // with the error linked to its control. Whitespace, not empty —
    // the field is `required`, so an empty value never leaves the
    // browser; the server trims and rejects the blank.
    page.goto(&app.url("/settings/account"), None).await?;
    page.locator(locator!("#account-name"))
        .fill("   ", None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(GetByRoleOptions::default().name("Save changes").exact(true)),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("#account-name[aria-invalid='true']")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/settings/account (name error)").await?;

    // The security tab right after creating an API token: the
    // one-time secret notice and the populated token list.
    page.goto(&app.url("/settings/security"), None).await?;
    page.locator(locator!("#api-token-name"))
        .fill("Axe sweep", None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(GetByRoleOptions::default().name("Create token").exact(true)),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("#api-token-secret")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/settings/security (token created)").await?;

    // Organizations: the empty list with the creation form, the form
    // in its error state (an invalid identifier, the name kept), then
    // the organization page reached by creating one.
    page.goto(&app.url("/organizations"), None).await?;
    check_axe(&page, "/organizations (empty)").await?;
    page.locator(locator!("#organization-name"))
        .fill("Axe Org", None)
        .await?;
    page.locator(locator!("#organization-slug"))
        .fill("not valid!", None)
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
    expect(page.locator(locator!("#organization-slug[aria-invalid='true']")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/organizations (slug error)").await?;
    page.locator(locator!("#organization-slug"))
        .fill(&format!("axe-{}", uuid::Uuid::new_v4()), None)
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
    expect(page.get_by_role(
        AriaRole::Heading,
        Some(GetByRoleOptions::default().name("Axe Org").exact(true)),
    ))
    .to_be_visible()
    .await?;
    check_axe(&page, "/organizations/{id}").await?;
    let organization_url = page.url();

    // The organization's teams: empty, the form in its error state
    // (a blank name — whitespace, since the field is `required`),
    // then populated.
    page.get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Manage teams").exact(true)),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("#team-name")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/organizations/{id}/teams (empty)").await?;
    page.locator(locator!("#team-name"))
        .fill("   ", None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(GetByRoleOptions::default().name("Create team").exact(true)),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("#team-name[aria-invalid='true']")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/organizations/{id}/teams (name error)").await?;
    page.locator(locator!("#team-name"))
        .fill("Axe reviewers", None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(GetByRoleOptions::default().name("Create team").exact(true)),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("li[data-team-id]")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/organizations/{id}/teams (populated)").await?;

    // The organization's procedures: the same three states, reached
    // from the organization page (by URL — walking history back
    // through a POST result stalls Chromium on resubmission).
    page.goto(&organization_url, None).await?;
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
    expect(page.locator(locator!("#procedure-title")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/organizations/{id}/procedures (empty)").await?;
    page.locator(locator!("#procedure-title"))
        .fill("   ", None)
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
    expect(page.locator(locator!("#procedure-title[aria-invalid='true']")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/organizations/{id}/procedures (title error)").await?;
    page.locator(locator!("#procedure-title"))
        .fill("Axe permit", None)
        .await?;
    page.locator(locator!("#procedure-description"))
        .fill("Checked by axe.", None)
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
    expect(page.locator(locator!("li[data-procedure-id]")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/organizations/{id}/procedures (populated)").await?;

    // The procedure page, then the schema editor in the states an
    // administrator meets: empty, with a column added and selected
    // (the detail form), a refused edit (a blank label saved through
    // the form — whitespace, the field is `required`), and the
    // discard confirmation.
    page.locator(locator!("li[data-procedure-id] a"))
        .click(None)
        .await?;
    expect(
        page.get_by_role(
            AriaRole::Link,
            Some(
                GetByRoleOptions::default()
                    .name("Edit the schema")
                    .exact(true),
            ),
        ),
    )
    .to_be_visible()
    .await?;
    check_axe(&page, "/organizations/{id}/procedures/{pid}").await?;
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
    check_axe(&page, "/organizations/{id}/procedures/{pid}/schema (empty)").await?;
    page.locator(locator!("#add-label"))
        .fill("Nom", None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(GetByRoleOptions::default().name("Add").exact(true)),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("li[data-element-id] a[aria-current='true']")))
        .to_be_visible()
        .await?;
    check_axe(
        &page,
        "/organizations/{id}/procedures/{pid}/schema (column selected)",
    )
    .await?;
    let editor_url = page.url();
    // A choice column: the options card with its row and add form.
    page.locator(locator!("#element-kind"))
        .select_option("ENUM", None)
        .await?;
    expect(page.locator(locator!("[data-save-status]")))
        .to_have_text("Saved your changes.")
        .await?;
    expect(page.locator(locator!("[data-options-empty]")))
        .to_be_visible()
        .await?;
    page.locator(locator!("#element-option-new"))
        .fill("Paris", None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(GetByRoleOptions::default().name("Add option").exact(true)),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("li[data-option-id]")))
        .to_be_visible()
        .await?;
    check_axe(
        &page,
        "/organizations/{id}/procedures/{pid}/schema (choice options)",
    )
    .await?;
    // The preview tab: the same draft rendered as a read-only form
    // (the selected column is a choice by now, so a select shows).
    page.get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Preview").exact(true)),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("select[id^='preview-']")))
        .to_be_visible()
        .await?;
    check_axe(&page, "/organizations/{id}/procedures/{pid}/schema/preview").await?;
    page.goto(&editor_url, None).await?;
    // A refused autosave: a blank label, blurred, is reported on the
    // status line (no Save button exists with the script running).
    page.locator(locator!("#element-label"))
        .fill("   ", None)
        .await?;
    page.keyboard().press("Tab", None).await?;
    expect(page.locator(locator!("[data-save-status]")))
        .to_have_text("A label is required.")
        .await?;
    check_axe(
        &page,
        "/organizations/{id}/procedures/{pid}/schema (refused edit)",
    )
    .await?;
    // The publish confirmation: the impact report over the draft (a
    // first draft classifies against the empty schema — all
    // additions), the re-send form, the way back.
    page.goto(&format!("{editor_url}&publish=confirm"), None)
        .await?;
    expect(page.locator(locator!("[data-impact-report]")))
        .to_be_visible()
        .await?;
    check_axe(
        &page,
        "/organizations/{id}/procedures/{pid}/schema (publish confirmation)",
    )
    .await?;
    page.goto(&editor_url, None).await?;
    page.get_by_role(
        AriaRole::Link,
        Some(
            GetByRoleOptions::default()
                .name("Discard the draft")
                .exact(true),
        ),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("[role='alertdialog']")))
        .to_be_visible()
        .await?;
    check_axe(
        &page,
        "/organizations/{id}/procedures/{pid}/schema (discard confirmation)",
    )
    .await?;

    page.goto(&app.url("/organizations"), None).await?;
    check_axe(&page, "/organizations (populated)").await?;
    Ok(())
}

fn account_menu_trigger(page: &Page) -> playwright_rs::protocol::Locator {
    page.locator(locator!("header summary[aria-label='Account menu']"))
}

/// Tab order through the header of the signed-in home, and the
/// account menu driven by keys alone: brand link, then the menu
/// trigger; Enter opens it; Tab walks its items (organizations,
/// settings, sign out); Enter on the last
/// one signs out.
///
/// The menu is a `<details>` element, so Escape does not close it —
/// the WAI-ARIA menu-button pattern's expectation, open as P.9 Q12
/// (d). This journey asserts what holds; it is not weakened to
/// paper over that gap.
async fn keyboard_scenario(
    _browser: &Browser,
    context: &BrowserContext,
    app: &App,
    engine: &'static str,
) -> TestResult {
    let page = context.new_page().await?;
    let email = unique_email(&format!("e2e-a11y-keys-{engine}"));
    browser_signup(&page, app, "Aurélie", &email).await?;
    if !accepts_secure_cookie_on_loopback_http(engine) {
        println!(
            "[{engine}] Secure session cookie refused over http://127.0.0.1; \
             the signed-in keyboard journey cannot run here"
        );
        return Ok(());
    }
    page.goto(&app.url("/"), None).await?;

    let keyboard = page.keyboard();
    let header = page.locator(locator!("header"));

    // `to_be_focused` hands the locator's selector straight to
    // `querySelectorAll`, so neither role locators nor chained
    // (`>>`) ones work there: the focus checks use single page-level
    // CSS selectors; role locators still prove visibility and names.
    keyboard.press("Tab", None).await?;
    expect(page.locator(locator!("header a[href='/']")))
        .to_be_focused()
        .await?;

    keyboard.press("Tab", None).await?;
    let trigger = account_menu_trigger(&page);
    expect(trigger.clone()).to_be_focused().await?;

    keyboard.press("Enter", None).await?;
    let settings_link = header.get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Settings").exact(true)),
    );
    expect(settings_link.clone()).to_be_visible().await?;

    keyboard.press("Tab", None).await?;
    expect(page.locator(locator!("header a[href='/organizations']")))
        .to_be_focused()
        .await?;

    keyboard.press("Tab", None).await?;
    expect(page.locator(locator!("header a[href='/settings']")))
        .to_be_focused()
        .await?;

    keyboard.press("Tab", None).await?;
    expect(header.get_by_role(
        AriaRole::Button,
        Some(GetByRoleOptions::default().name("Sign out").exact(true)),
    ))
    .to_be_visible()
    .await?;
    expect(page.locator(locator!("header form button[type='submit']")))
        .to_be_focused()
        .await?;

    keyboard.press("Enter", None).await?;
    expect_page(&page).to_have_url(&app.url("/")).await?;
    expect(header.get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Sign in").exact(true)),
    ))
    .to_be_visible()
    .await?;
    Ok(())
}
