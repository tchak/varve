//! Subject: accessibility in a real browser (PLATFORM.md P.1.5) —
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
    Ok(())
}

fn account_menu_trigger(page: &Page) -> playwright_rs::protocol::Locator {
    page.locator(locator!("header summary[aria-label='Account menu']"))
}

/// Tab order through the header of the signed-in home, and the
/// account menu driven by keys alone: brand link, then the menu
/// trigger; Enter opens it; Tab walks its items; Enter on the last
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
