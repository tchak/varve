//! # Browser tests
//!
//! Real-browser end-to-end tests: the app is served in-process on an
//! ephemeral port ([`topcoat::serve_until`]) and driven headless
//! through [`playwright-rs`](https://docs.rs/playwright-rs) on every
//! installed engine — chromium, firefox, and webkit. Each test runs
//! its scenario against each engine in turn (serially, with a fresh
//! browser context per engine) and reports failures per engine.
//!
//! **Gating** (the settled P.3 convention — `cargo test --workspace`
//! stays green with nothing installed): each test passes vacuously,
//! after printing why, unless **both**
//!
//! 1. `VARVE_TEST_DATABASE_URL` is set (same scratch database as
//!    `tests/app.rs`), and
//! 2. at least one Playwright engine matching the bundled driver is
//!    installed — a miss surfaces as the crate's
//!    [`playwright_rs::Error::BrowserNotInstalled`] at launch. Each
//!    missing engine prints its own `skipped:` line and is dropped
//!    from the run; the installed ones still run.
//!
//! **One-time setup.** The pinned `playwright-rs` dev-dependency
//! downloads its Playwright driver (with its own Node runtime) at
//! build time; browsers are installed once, through the crate so the
//! versions match, via the vendored example (no arguments installs
//! all three engines):
//!
//! ```text
//! cargo run -p platform-app --example install-browsers
//! ```
//!
//! Then run for real with e.g.:
//!
//! ```text
//! VARVE_TEST_DATABASE_URL=postgres://localhost/varve_platform_test \
//!   cargo test -p platform-app --test e2e
//! ```
//!
//! Tests share one database and run in parallel, so every test mints
//! unique emails (unique per engine too — engines within one test
//! share the database), uses a fresh browser context per engine, and
//! never asserts on global counts (same rules as `tests/app.rs`). On
//! failure a Playwright trace named after the scenario *and* engine
//! (e.g. `signup-roundtrip.webkit.trace.zip`) is written under
//! `CARGO_TARGET_TMPDIR` (the path is printed); open it at
//! <https://trace.playwright.dev>.
//!
//! **Known engine divergence.** WebKit refuses the app's `Secure`
//! `__Host-session` cookie over plain-http loopback, so signed-in
//! flows cannot run there; the affected scenarios assert that
//! refusal explicitly instead — see
//! [`accepts_secure_cookie_on_loopback_http`].

use std::net::SocketAddr;
use std::path::Path;

use playwright_rs::protocol::{
    AriaRole, Browser, BrowserContext, BrowserContextOptions, GetByRoleOptions, Page, Playwright,
    Tracing, TracingStartOptions, TracingStopOptions,
};
use playwright_rs::{Error, expect, expect_page, locator};
use tokio::sync::oneshot;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A plain-`bool` assertion inside a scenario. Returns `Err` instead
/// of panicking so the trace-on-failure cleanup in [`run_scenario`]
/// still runs (a panic would skip it).
fn ensure(cond: bool, message: impl Into<String>) -> TestResult {
    if cond {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@example.test", uuid::Uuid::new_v4())
}

/// The app served in-process on an ephemeral port, plus the database
/// handle for direct seeding.
struct App {
    db: toasty::Db,
    addr: SocketAddr,
    shutdown: oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}

impl App {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    async fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = self.server.await;
    }
}

/// The gate: connects the database, boots the app server, and
/// launches every installed engine. `None` (after printing why) when
/// the database env var is unset or when *no* engine is installed, so
/// the test passes vacuously. Each missing engine prints its own
/// `skipped:` line and drops out; the installed ones still run.
///
/// The returned [`Playwright`] handle must stay alive for the whole
/// test — dropping it tears down the driver process.
async fn e2e() -> Option<(Playwright, Vec<(&'static str, Browser)>, App)> {
    // Connects one test at a time — see `platform-core/tests/db.rs`
    // for why (unguarded concurrent migration application in toasty
    // 0.10).
    static CONNECT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    let url = match std::env::var("VARVE_TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("skipped: VARVE_TEST_DATABASE_URL not set");
            return None;
        }
    };
    let playwright = match Playwright::launch().await {
        Ok(playwright) => playwright,
        Err(error) => {
            println!("skipped: playwright driver failed to start: {error}");
            return None;
        }
    };
    let mut engines = Vec::new();
    for (name, browser_type) in [
        ("chromium", playwright.chromium()),
        ("firefox", playwright.firefox()),
        ("webkit", playwright.webkit()),
    ] {
        match browser_type.launch().await {
            Ok(browser) => engines.push((name, browser)),
            Err(Error::BrowserNotInstalled { .. }) => println!(
                "skipped: playwright {name} is not installed; run \
                 `cargo run -p platform-app --example install-browsers -- {name}`"
            ),
            Err(error) => panic!("{name} failed to launch: {error}"),
        }
    }
    if engines.is_empty() {
        return None;
    }

    let db = {
        let _guard = CONNECT_LOCK.lock().await;
        platform_core::connect(&url).await.expect("connect")
    };
    let router = platform_app::router(db.clone(), None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let (shutdown, signal) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        topcoat::serve_until(listener, router, async {
            let _ = signal.await;
        })
        .await
        .expect("serve the app");
    });

    Some((
        playwright,
        engines,
        App {
            db,
            addr,
            shutdown,
            server,
        },
    ))
}

/// Starts a trace on `context` (screenshots + DOM snapshots) so a
/// failed scenario leaves a post-mortem artifact.
async fn trace_start(context: &BrowserContext, name: &str) -> Tracing {
    let tracing = context.tracing().await.expect("tracing channel");
    tracing
        .start(Some(
            TracingStartOptions::default()
                .name(name)
                .screenshots(true)
                .snapshots(true),
        ))
        .await
        .expect("start tracing");
    tracing
}

/// A fresh default context — the per-engine context factory most
/// scenarios use.
async fn default_context(browser: &Browser) -> playwright_rs::Result<BrowserContext> {
    browser.new_context().await
}

/// The whole multi-engine test body: gate, then for each installed
/// engine run `scenario` in a fresh context (from `make_context`)
/// with trace-on-failure, then unconditional cleanup (Rust has no
/// async `Drop`, so it is explicit — the crate's canonical pattern):
/// stop tracing (writing `{name}.{engine}.trace.zip` only when that
/// engine's run failed), close everything, and panic listing every
/// failing engine.
async fn run_scenario<C, F>(name: &str, make_context: C, scenario: F)
where
    C: AsyncFn(&Browser) -> playwright_rs::Result<BrowserContext>,
    F: AsyncFn(&BrowserContext, &App, &'static str) -> TestResult,
{
    let Some((_playwright, engines, app)) = e2e().await else {
        return;
    };
    let mut failures = Vec::new();
    for (engine, browser) in &engines {
        let context = match make_context(browser).await {
            Ok(context) => context,
            Err(error) => {
                failures.push(format!("[{engine}] new context: {error}"));
                continue;
            }
        };
        let tracing = trace_start(&context, &format!("{name}.{engine}")).await;
        let result = scenario(&context, &app, engine).await;
        let stop = if result.is_err() {
            let path =
                Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.{engine}.trace.zip"));
            println!(
                "[{engine}] trace written to {} — open it at https://trace.playwright.dev",
                path.display()
            );
            TracingStopOptions::default().path(path.to_string_lossy().into_owned())
        } else {
            TracingStopOptions::default()
        };
        let _ = tracing.stop(Some(stop)).await;
        let _ = context.close().await;
        if let Err(error) = result {
            failures.push(format!("[{engine}] {error}"));
        }
    }
    for (_, browser) in &engines {
        let _ = browser.close().await;
    }
    app.stop().await;
    assert!(
        failures.is_empty(),
        "{name} failed on {} engine(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Whether `engine` treats plain-http loopback (`http://127.0.0.1`)
/// as a trustworthy origin for `Secure` cookies — which the
/// `__Host-session` cookie needs to be stored at all. Chromium and
/// Firefox extend the secure-context loopback carve-out to cookies;
/// WebKit does not: it refuses a `Secure` cookie set over plain http
/// even on loopback, so the session never sticks there. Verified
/// empirically against the pinned engines; over real https all three
/// accept the cookie, so this is a test-transport divergence, not an
/// app bug.
fn accepts_secure_cookie_on_loopback_http(engine: &str) -> bool {
    engine != "webkit"
}

/// Signs a fresh account up through the real form and waits for the
/// 303-to-home landing.
async fn browser_signup(page: &Page, app: &App, name: &str, email: &str) -> TestResult {
    page.goto(&app.url("/signup"), None).await?;
    page.locator(locator!("#signup-name"))
        .fill(name, None)
        .await?;
    page.locator(locator!("#signup-email"))
        .fill(email, None)
        .await?;
    page.locator(locator!("#signup-password"))
        .fill("s3cret-enough", None)
        .await?;
    page.get_by_role(
        AriaRole::Button,
        Some(
            GetByRoleOptions::default()
                .name("Create account")
                .exact(true),
        ),
    )
    .click(None)
    .await?;
    expect_page(page).to_have_url(&app.url("/")).await?;
    Ok(())
}

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
