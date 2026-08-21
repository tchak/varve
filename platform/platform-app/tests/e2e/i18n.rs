//! Subject: locale resolution rendered end to end — a browser's
//! language reaching the page as French copy. Uses /signin as its
//! canvas, but the subject is i18n, not authentication.

use playwright_rs::protocol::{
    AriaRole, Browser, BrowserContext, BrowserContextOptions, GetByRoleOptions,
};
use playwright_rs::{expect, locator};

use crate::harness::{App, TestResult, run_scenario};

#[tokio::test(flavor = "multi_thread")]
async fn french_context_renders_signin_in_french() {
    run_scenario("french-signin", french_context, french_scenario).await;
}

/// A French browser context: Playwright's `locale` sets the browser's
/// Accept-Language, which is what the app's locale resolution reads
/// for anonymous requests.
async fn french_context(browser: &Browser) -> playwright_rs::Result<BrowserContext> {
    browser
        .new_context_with_options(
            BrowserContextOptions::builder()
                .locale("fr-FR".to_owned())
                .build(),
        )
        .await
}

async fn french_scenario(context: &BrowserContext, app: &App, _engine: &'static str) -> TestResult {
    let page = context.new_page().await?;
    page.goto(&app.url("/signin"), None).await?;
    expect(page.locator(locator!("main h1")))
        .to_have_text("Se connecter")
        .await?;
    // The signup link is French too. Substring matches sidestep the
    // no-break space French typography puts before `?`.
    let signup_link = page.get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Pas encore de compte")),
    );
    expect(signup_link.clone()).to_be_visible().await?;
    expect(signup_link).to_contain_text("Créez-en un").await?;
    Ok(())
}
