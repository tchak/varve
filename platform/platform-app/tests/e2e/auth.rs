//! Subject: authentication journeys — signup, sign-in failure, the
//! session cookie's transport hardening, signout.

use playwright_rs::protocol::{AriaRole, BrowserContext, GetByRoleOptions};
use playwright_rs::{expect, expect_page, locator};

use crate::harness::{
    App, TestResult, accepts_secure_cookie_on_loopback_http, browser_signup, default_context,
    ensure, run_scenario, unique_email,
};

#[tokio::test(flavor = "multi_thread")]
async fn signup_greets_signs_out_and_reverts_the_header() {
    run_scenario("signup-roundtrip", default_context, signup_scenario).await;
}

async fn signup_scenario(context: &BrowserContext, app: &App, engine: &'static str) -> TestResult {
    let page = context.new_page().await?;
    // Mixed case in, normalized (trimmed, lowercased) out — the
    // header must show what platform-core stored, not what was typed.
    let typed = format!(
        "E2E-Roundtrip-{engine}-{}@Example.Test",
        uuid::Uuid::new_v4()
    );
    let normalized = typed.to_lowercase();
    browser_signup(&page, app, "Amélie", &typed).await?;

    let header = page.locator(locator!("header"));
    if !accepts_secure_cookie_on_loopback_http(engine) {
        // The engine refused the `Secure` session cookie over plain
        // http (see [`accepts_secure_cookie_on_loopback_http`]), so
        // the signed-in half of the round trip cannot happen here.
        // Assert the divergent outcome explicitly: signup itself
        // succeeded (the 303-to-home landed, awaited above), but the
        // session did not stick — the header stays signed out.
        println!(
            "[{engine}] Secure session cookie refused over http://127.0.0.1; \
             asserting the signed-out landing instead of the greeting"
        );
        expect(header.get_by_role(
            AriaRole::Link,
            Some(GetByRoleOptions::default().name("Sign in").exact(true)),
        ))
        .to_be_visible()
        .await?;
        expect(header.get_by_text("Signed in as", false))
            .not()
            .to_be_visible()
            .await?;
        return Ok(());
    }
    expect(header.get_by_text(&format!("Signed in as {normalized}"), false))
        .to_be_visible()
        .await?;
    expect(page.locator(locator!("main p")))
        .to_have_text("Hello, Amélie.")
        .await?;

    header
        .get_by_role(
            AriaRole::Button,
            Some(GetByRoleOptions::default().name("Sign out").exact(true)),
        )
        .click(None)
        .await?;

    // The header reverts to the signed-out state.
    expect(header.get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Sign in").exact(true)),
    ))
    .to_be_visible()
    .await?;
    expect(header.get_by_text("Signed in as", false))
        .not()
        .to_be_visible()
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_password_stays_on_signin_with_one_generic_alert() {
    run_scenario("wrong-password", default_context, wrong_password_scenario).await;
}

async fn wrong_password_scenario(
    context: &BrowserContext,
    app: &App,
    engine: &'static str,
) -> TestResult {
    // Seed the account directly through platform-core — the scenario
    // under test is the failed sign-in, not registration.
    let email = unique_email(&format!("e2e-wrong-password-{engine}"));
    let mut db = app.db.clone();
    platform_core::register(&mut db, &email, "s3cret-enough", "Alice")
        .await
        .map_err(|error| format!("seed account: {error}"))?;

    let page = context.new_page().await?;
    page.goto(&app.url("/signin"), None).await?;
    page.locator(locator!("#signin-email"))
        .fill(&email, None)
        .await?;
    page.locator(locator!("#signin-password"))
        .fill("not-the-password", None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(GetByRoleOptions::default().name("Sign in").exact(true)),
    )
    .click(None)
    .await?;

    // Exactly one alert, carrying the generic message (the
    // platform-core `None` collapse carried through to the view).
    let alerts = page.locator(locator!("[role='alert']"));
    expect(alerts.clone()).to_have_count(1).await?;
    expect(alerts.first())
        .to_have_text("Incorrect email address or password.")
        .await?;
    // Still on /signin; the email survives the re-render, the
    // password never does.
    expect_page(&page).to_have_url(&app.url("/signin")).await?;
    expect(page.locator(locator!("#signin-email")))
        .to_have_value(&email)
        .await?;
    expect(page.locator(locator!("#signin-password")))
        .to_have_value("")
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn session_cookie_is_hardened_and_cleared_on_signout() {
    run_scenario("session-cookie", default_context, cookie_scenario).await;
}

async fn cookie_scenario(context: &BrowserContext, app: &App, engine: &'static str) -> TestResult {
    let page = context.new_page().await?;
    let email = unique_email(&format!("e2e-cookie-{engine}"));
    browser_signup(&page, app, "Benoît", &email).await?;

    // Read the session cookie back from the browser context — the
    // hardened transport (`__Host-` prefix, `HttpOnly`, `SameSite=Lax`,
    // `Path=/`) as the browser actually stored it. Chromium and
    // Firefox accept `Secure` cookies from http://127.0.0.1: loopback
    // is a trustworthy origin for them.
    let cookies = context.cookies(None).await?;
    if !accepts_secure_cookie_on_loopback_http(engine) {
        // WebKit refuses the `Secure` cookie over plain http (see
        // [`accepts_secure_cookie_on_loopback_http`]). Assert that
        // divergence explicitly — the refusal must be total, not a
        // downgraded (non-Secure or renamed) cookie sneaking in.
        println!(
            "[{engine}] Secure session cookie refused over http://127.0.0.1; \
             asserting the refusal instead of the hardened attributes"
        );
        ensure(
            !cookies.iter().any(|cookie| cookie.name.contains("session")),
            format!("expected {engine} to refuse the Secure session cookie, got {cookies:?}"),
        )?;
        return Ok(());
    }
    let session: Vec<_> = cookies
        .iter()
        .filter(|cookie| cookie.name.contains("session"))
        .collect();
    ensure(
        session.len() == 1,
        format!("expected exactly one session cookie, got {cookies:?}"),
    )?;
    let cookie = session[0];
    ensure(
        cookie.name == "__Host-session",
        format!(
            "session cookie is named {:?}, not __Host-session",
            cookie.name
        ),
    )?;
    ensure(cookie.http_only, "session cookie is not HttpOnly")?;
    ensure(
        cookie.same_site.as_deref() == Some("Lax"),
        format!("session cookie SameSite is {:?}, not Lax", cookie.same_site),
    )?;
    ensure(cookie.secure, "session cookie is not Secure")?;
    ensure(
        cookie.path == "/",
        format!("session cookie path is {:?}, not /", cookie.path),
    )?;

    // Sign out, wait for the signed-out header, and the cookie is gone.
    page.get_by_role(
        AriaRole::Button,
        Some(GetByRoleOptions::default().name("Sign out").exact(true)),
    )
    .click(None)
    .await?;
    expect(page.locator(locator!("header")).get_by_role(
        AriaRole::Link,
        Some(GetByRoleOptions::default().name("Sign in").exact(true)),
    ))
    .to_be_visible()
    .await?;
    let cookies = context.cookies(None).await?;
    ensure(
        !cookies.iter().any(|cookie| cookie.name.contains("session")),
        format!("session cookie survived signout: {cookies:?}"),
    )?;
    Ok(())
}
