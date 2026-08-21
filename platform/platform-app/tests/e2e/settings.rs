//! Subject: the settings area as one browser journey — the session
//! list across two real browser contexts (what `Router::handle`
//! cannot prove: two live cookie jars against one account), the
//! current-session marker, per-session revocation, the tab
//! navigation ridden along the way, and the profile save through the
//! real language select flipping the whole page to French.

use playwright_rs::protocol::{AriaRole, Browser, BrowserContext, GetByRoleOptions};
use playwright_rs::{expect, expect_page, locator};

use crate::harness::{
    App, TestResult, accepts_secure_cookie_on_loopback_http, browser_signup, default_context,
    run_scenario, unique_email,
};

#[tokio::test(flavor = "multi_thread")]
async fn second_browser_lists_and_revocation_shrinks_the_sessions() {
    run_scenario("settings-sessions", default_context, sessions_scenario).await;
}

async fn sessions_scenario(
    browser: &Browser,
    context: &BrowserContext,
    app: &App,
    engine: &'static str,
) -> TestResult {
    let page = context.new_page().await?;
    let email = unique_email(&format!("e2e-settings-{engine}"));
    browser_signup(&page, app, "Solène", &email).await?;

    if !accepts_secure_cookie_on_loopback_http(engine) {
        // The engine refused the `Secure` session cookie over plain
        // http (see [`accepts_secure_cookie_on_loopback_http`]), so
        // the signed-in journey cannot happen here. Assert the
        // divergent outcome explicitly: the settings gate sees an
        // anonymous request and redirects to /signin.
        println!(
            "[{engine}] Secure session cookie refused over http://127.0.0.1; \
             asserting the anonymous redirect to /signin instead"
        );
        page.goto(&app.url("/settings/security"), None).await?;
        expect_page(&page).to_have_url(&app.url("/signin")).await?;
        return Ok(());
    }

    // The security tab lists exactly this session, marked current.
    page.goto(&app.url("/settings/security"), None).await?;
    let rows = page.locator(locator!("main li"));
    let current_row = page.locator(locator!("main li[data-current='true']"));
    expect(rows.clone()).to_have_count(1).await?;
    expect(current_row.get_by_text("Current session", false))
        .to_be_visible()
        .await?;

    // The tabs are real navigation: Account and back to Security.
    page.get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Account").exact(true)),
    )
    .click(None)
    .await?;
    expect_page(&page)
        .to_have_url(&app.url("/settings/account"))
        .await?;
    expect(page.get_by_text("User profile", false))
        .to_be_visible()
        .await?;
    page.get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Security").exact(true)),
    )
    .click(None)
    .await?;
    expect_page(&page)
        .to_have_url(&app.url("/settings/security"))
        .await?;

    // A second browser context — its own cookie jar — signs in to
    // the same account through the real form.
    let second = browser.new_context().await?;
    let second_page = second.new_page().await?;
    second_page.goto(&app.url("/signin"), None).await?;
    second_page
        .locator(locator!("#signin-email"))
        .fill(&email, None)
        .await?;
    second_page
        .locator(locator!("#signin-password"))
        .fill("s3cret-enough", None)
        .await?;
    second_page
        .get_by_role(
            AriaRole::Button,
            Some(GetByRoleOptions::default().name("Sign in").exact(true)),
        )
        .click(None)
        .await?;
    expect_page(&second_page).to_have_url(&app.url("/")).await?;
    second.close().await?;

    // The first browser now sees two sessions; revoking the other
    // one shrinks the list back to just the current session.
    page.goto(&app.url("/settings/security"), None).await?;
    expect(rows.clone()).to_have_count(2).await?;
    page.locator(locator!("main li[data-current='false']"))
        .get_by_role(
            AriaRole::Button,
            Some(
                GetByRoleOptions::default()
                    .name("Revoke session")
                    .exact(true),
            ),
        )
        .click(None)
        .await?;
    expect_page(&page)
        .to_have_url(&app.url("/settings/security"))
        .await?;
    expect(rows.clone()).to_have_count(1).await?;
    expect(current_row.clone()).to_be_visible().await?;

    // Ride into the Account tab and change the language through the
    // real select — the part only a browser proves: a native select
    // interaction, the form submit, and the full-page language flip
    // on the followed redirect.
    page.get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Account").exact(true)),
    )
    .click(None)
    .await?;
    expect_page(&page)
        .to_have_url(&app.url("/settings/account"))
        .await?;
    page.locator(locator!("#account-locale"))
        .select_option("fr", None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(GetByRoleOptions::default().name("Save changes").exact(true)),
    )
    .click(None)
    .await?;
    expect_page(&page)
        .to_have_url(&app.url("/settings/account"))
        .await?;
    // The page re-rendered in the stored preference's locale: the
    // security tab label is now French.
    expect(page.get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Sécurité").exact(true)),
    ))
    .to_be_visible()
    .await?;
    Ok(())
}
