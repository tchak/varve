//! Shared machinery for every e2e subject module: gating, the
//! in-process app, the multi-engine scenario driver, traces, and the
//! cross-subject helpers. Subject modules import from here and add
//! nothing app-generic of their own — anything two subjects need
//! belongs in this module.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use playwright_rs::protocol::{
    AddScriptTagOptions, AriaRole, Browser, BrowserContext, BrowserContextOptions,
    GetByRoleOptions, Page, Playwright, Tracing, TracingStartOptions, TracingStopOptions,
};
use playwright_rs::{Error, expect_page, locator};
use tokio::sync::oneshot;
use topcoat::asset::AssetBundle;
use topcoat_asset::{Bundler, BundlerConfig};

pub type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// A plain-`bool` assertion inside a scenario. Returns `Err` instead
/// of panicking so the trace-on-failure cleanup in [`run_scenario`]
/// still runs (a panic would skip it).
pub fn ensure(cond: bool, message: impl Into<String>) -> TestResult {
    if cond {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

pub fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@example.test", uuid::Uuid::new_v4())
}

/// The app served in-process on an ephemeral port, plus the database
/// handle for direct seeding.
pub struct App {
    pub db: toasty::Db,
    addr: SocketAddr,
    shutdown: oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}

impl App {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    pub async fn stop(self) {
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
pub async fn e2e() -> Option<(Playwright, Vec<(&'static str, Browser)>, App)> {
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
    let router = platform_app::router(db.clone(), Some(assets().await));
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

/// The app's asset bundle — the Tailwind stylesheet — scanned from
/// *this test binary*, the way `topcoat asset bundle` scans a server
/// binary: bundle and binary come from the same build, so the
/// content-hashed asset ids match without a CI step. Bundled once
/// per process under `CARGO_TARGET_TMPDIR`, then loaded per test.
///
/// Styled pages are a precondition, not a nicety: the `a11y` subject
/// runs axe over contrast and target size, and unstyled markup would
/// pass or fail those rules meaninglessly. Hence the hard check that
/// the stylesheet made it into the bundle.
async fn assets() -> AssetBundle {
    static BUNDLE_DIR: tokio::sync::OnceCell<PathBuf> = tokio::sync::OnceCell::const_new();
    let dir = BUNDLE_DIR
        .get_or_init(|| async {
            let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
            let out_dir = tmp.join("e2e-assets");
            let cache_dir = tmp.join("e2e-asset-cache");
            let out = out_dir.clone();
            tokio::task::spawn_blocking(move || {
                let exe = std::env::current_exe().expect("current executable");
                let bytes = std::fs::read(exe).expect("read the test binary");
                Bundler::new(&BundlerConfig::new().cache_dir(cache_dir))
                    .bundle(&bytes, &out)
                    .expect("bundle the assets declared in the test binary");
            })
            .await
            .expect("bundling task");
            out_dir
        })
        .await;
    let bundle = AssetBundle::load_dir(dir).expect("load the e2e asset bundle");
    assert!(
        bundle
            .catalog()
            .assets()
            .any(|asset| asset.content_type() == "text/css"),
        "the e2e asset bundle holds no stylesheet — pages would render unstyled"
    );
    bundle
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
pub async fn default_context(browser: &Browser) -> playwright_rs::Result<BrowserContext> {
    browser.new_context().await
}

/// A French browser context: Playwright's `locale` sets the browser's
/// Accept-Language, which is what the app's locale resolution reads
/// for anonymous requests.
pub async fn french_context(browser: &Browser) -> playwright_rs::Result<BrowserContext> {
    browser
        .new_context_with_options(
            BrowserContextOptions::builder()
                .locale("fr-FR".to_owned())
                .build(),
        )
        .await
}

/// The whole multi-engine test body: gate, then for each installed
/// engine run `scenario` in a fresh context (from `make_context`)
/// with trace-on-failure, then unconditional cleanup (Rust has no
/// async `Drop`, so it is explicit — the crate's canonical pattern):
/// stop tracing (writing `{name}.{engine}.trace.zip` only when that
/// engine's run failed), close everything, and panic listing every
/// failing engine.
///
/// The scenario also receives the engine's [`Browser`], so
/// multi-context journeys (a second signed-in browser) can open
/// extra contexts; close them in the scenario when the flow is done
/// — the engine teardown's `browser.close()` only backstops the
/// error paths (extra contexts are not traced).
pub async fn run_scenario<C, F>(name: &str, make_context: C, scenario: F)
where
    C: AsyncFn(&Browser) -> playwright_rs::Result<BrowserContext>,
    F: AsyncFn(&Browser, &BrowserContext, &App, &'static str) -> TestResult,
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
        let result = scenario(browser, &context, &app, engine).await;
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
pub fn accepts_secure_cookie_on_loopback_http(engine: &str) -> bool {
    engine != "webkit"
}

/// Signs a fresh account up through the real form and waits for the
/// 303-to-home landing.
pub async fn browser_signup(page: &Page, app: &App, name: &str, email: &str) -> TestResult {
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

/// One axe-core rule violation, the fields worth a failure message.
#[derive(Debug, serde::Deserialize)]
pub struct AxeViolation {
    pub id: String,
    pub impact: Option<String>,
    pub help: String,
    #[serde(rename = "helpUrl")]
    pub help_url: String,
    pub nodes: Vec<AxeNode>,
}

#[derive(Debug, serde::Deserialize)]
pub struct AxeNode {
    pub target: Vec<String>,
    pub html: String,
    #[serde(rename = "failureSummary")]
    pub failure_summary: Option<String>,
}

/// Runs the axe-core rule engine (vendored, `tests/e2e/vendor/axe-core`)
/// over the page as currently rendered — WCAG 2.x A/AA tags plus the
/// best-practice set — and fails with every violation listed. The
/// dynamic share of PLATFORM.md P.1.5 (contrast, computed roles and
/// names, focusability) that the router tests' static baseline
/// cannot decide; `label` names the page in the failure.
///
/// Inject-then-evaluate mirrors `@axe-core/playwright`: the script
/// tag is added to the live document, so call it after navigation
/// (and again after any client-side state change worth checking).
pub async fn check_axe(page: &Page, label: &str) -> TestResult {
    page.add_script_tag(
        AddScriptTagOptions::builder()
            .content(include_str!("vendor/axe-core/axe.min.js"))
            .build(),
    )
    .await?;
    let violations: Vec<AxeViolation> = page
        .evaluate(
            "() => axe.run(document, { runOnly: { type: 'tag', values: \
             ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa', 'best-practice'] } }) \
             .then(results => results.violations)",
            None::<&()>,
        )
        .await?;
    if violations.is_empty() {
        return Ok(());
    }
    let mut report = format!("axe found {} violation(s) on {label}:", violations.len());
    for violation in &violations {
        report.push_str(&format!(
            "\n  - [{}] {} ({}) {}",
            violation.impact.as_deref().unwrap_or("unknown"),
            violation.id,
            violation.help,
            violation.help_url
        ));
        for node in &violation.nodes {
            report.push_str(&format!(
                "\n      {} — {}",
                node.target.join(" "),
                node.html
            ));
            if let Some(summary) = &node.failure_summary {
                for line in summary.lines() {
                    report.push_str(&format!("\n        {line}"));
                }
            }
        }
    }
    Err(report.into())
}
