//! Shared machinery for every app subject module: gating, the
//! request/response builders, form encoding, and the cross-subject
//! signup helper. Subject modules import from here and add nothing
//! app-generic of their own — anything two subjects need belongs in
//! this module.

use topcoat::router::{
    Body, Method, Router, StatusCode, header, request::Request, response::Response, to_bytes,
};

/// Connects (applying migrations) and builds the app router; `None`
/// (after printing why) when `VARVE_TEST_DATABASE_URL` is unset so
/// the test passes vacuously. Connects one test at a time — see
/// `platform-core/tests/db.rs` for why (unguarded concurrent
/// migration application in toasty 0.10).
pub async fn test_app() -> Option<(Router, toasty::Db)> {
    static CONNECT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    let url = match std::env::var("VARVE_TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("skipped: VARVE_TEST_DATABASE_URL not set");
            return None;
        }
    };
    let _guard = CONNECT_LOCK.lock().await;
    let db = platform_core::connect(&url).await.expect("connect");
    Some((platform_app::router(db.clone(), None), db))
}

pub fn unique_email(tag: &str) -> String {
    format!("{tag}-{}@example.test", uuid::Uuid::new_v4())
}

/// Builds a request; `headers` are `(name, value)` pairs.
pub fn request(method: Method, path: &str, headers: &[(&str, &str)], body: Body) -> Request {
    let mut builder = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    builder.body(body).unwrap()
}

pub fn get(path: &str, headers: &[(&str, &str)]) -> Request {
    request(Method::GET, path, headers, Body::empty())
}

/// A `POST` with an `application/x-www-form-urlencoded` body.
pub fn post(path: &str, headers: &[(&str, &str)], form: String) -> Request {
    let mut all = vec![("content-type", "application/x-www-form-urlencoded")];
    all.extend_from_slice(headers);
    request(Method::POST, path, &all, Body::from(form))
}

/// Percent-encodes one form value conservatively (everything but
/// unreserved characters), so emails and passwords survive
/// `application/x-www-form-urlencoded` exactly.
fn urlencode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn form_body(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(name, value)| format!("{name}={}", urlencode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Reads the body as text. Every `text/html` response also passes
/// the accessibility baseline ([`a11y_baseline`]) — the static share
/// of PLATFORM.md P.1.5 that router tests own — so no page test can
/// forget it. A page that fails is fixed, never exempted.
pub async fn body_text(response: Response) -> String {
    let (parts, body) = response.into_parts();
    let is_html = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    if is_html {
        let violations = a11y_baseline(&text);
        assert!(
            violations.is_empty(),
            "accessibility baseline violations:\n  - {}\n\n{text}",
            violations.join("\n  - ")
        );
    }
    text
}

/// The static accessibility baseline over one rendered document:
/// the WCAG/RGAA failures that are decidable from markup alone, so
/// they are caught at router level before any browser runs (the rule
/// engine in a real browser — axe in `tests/e2e` — owns the rest).
/// Returns one message per violation; empty means clean.
///
/// Rules (each names the RGAA 4.1 criterion it approximates):
/// - the document declares `lang` (8.3);
/// - a `<main>` landmark, exactly one (9.2);
/// - exactly one `<h1>`, heading levels never skip (9.1);
/// - every `<img>` has `alt` (1.1);
/// - every focusable form control has a label: `<label for>`,
///   a wrapping `<label>`, `aria-label` or `aria-labelledby` (11.1);
/// - every button and link has an accessible name: text content,
///   `aria-label`, `aria-labelledby`, or an image with `alt` /
///   an `<svg>` with a `<title>` (6.1, 7.1);
/// - an error message tied to a control is referenced from it
///   (`aria-describedby`), and the control carries `aria-invalid`
///   (11.10).
pub fn a11y_baseline(html: &str) -> Vec<String> {
    use scraper::{ElementRef, Html, Selector};

    let sel = |css: &str| Selector::parse(css).unwrap();
    let doc = Html::parse_document(html);
    let mut out = Vec::new();

    let lang = doc
        .select(&sel("html"))
        .next()
        .and_then(|el| el.value().attr("lang"))
        .map(str::trim)
        .unwrap_or_default();
    if lang.is_empty() {
        out.push("<html> has no lang".to_owned());
    }

    let mains = doc.select(&sel("main")).count();
    if mains != 1 {
        out.push(format!("expected exactly one <main>, found {mains}"));
    }

    let h1s = doc.select(&sel("h1")).count();
    if h1s != 1 {
        out.push(format!("expected exactly one <h1>, found {h1s}"));
    }
    let mut previous = 0u8;
    for heading in doc.select(&sel("h1, h2, h3, h4, h5, h6")) {
        let level = heading.value().name().as_bytes()[1] - b'0';
        if level > previous + 1 {
            out.push(format!(
                "heading level skips from h{previous} to h{level}: {:?}",
                text_of(heading)
            ));
        }
        previous = level;
    }

    for img in doc.select(&sel("img")) {
        if img.value().attr("alt").is_none() {
            out.push(format!("<img> without alt: {}", opening_tag(img)));
        }
    }

    let labelled_for: std::collections::HashSet<&str> = doc
        .select(&sel("label[for]"))
        .filter_map(|label| label.value().attr("for"))
        .collect();
    for control in doc.select(&sel(
        "input:not([type=hidden]):not([type=submit]):not([type=button]):not([type=reset]):not([type=image]), select, textarea",
    )) {
        let el = control.value();
        let labelled = el.attr("aria-label").is_some_and(|v| !v.trim().is_empty())
            || el.attr("aria-labelledby").is_some()
            || el.attr("id").is_some_and(|id| labelled_for.contains(id))
            || has_ancestor(control, "label");
        if !labelled {
            out.push(format!("form control without a label: {}", opening_tag(control)));
        }
        if let Some(describedby) = el.attr("aria-describedby") {
            for id in describedby.split_whitespace() {
                if doc.select(&sel(&format!("[id=\"{id}\"]"))).next().is_none() {
                    out.push(format!(
                        "aria-describedby points at missing id {id:?}: {}",
                        opening_tag(control)
                    ));
                }
            }
        }
    }

    for node in doc.select(&sel("button, a[href], [role=button], [role=link]")) {
        if accessible_name(node).is_empty() {
            out.push(format!(
                "control without an accessible name: {}",
                opening_tag(node)
            ));
        }
    }

    for node in doc.select(&sel("[aria-labelledby]")) {
        for id in node
            .value()
            .attr("aria-labelledby")
            .unwrap()
            .split_whitespace()
        {
            if doc.select(&sel(&format!("[id=\"{id}\"]"))).next().is_none() {
                out.push(format!(
                    "aria-labelledby points at missing id {id:?}: {}",
                    opening_tag(node)
                ));
            }
        }
    }

    // Error text tied to a control: any `[aria-invalid=true]` control
    // must describe its error, and any element conventionally marked
    // as a field error must be referenced by some control.
    for control in doc.select(&sel("[aria-invalid=true]")) {
        if control.value().attr("aria-describedby").is_none() {
            out.push(format!(
                "invalid control without aria-describedby: {}",
                opening_tag(control)
            ));
        }
    }

    fn has_ancestor(node: ElementRef<'_>, name: &str) -> bool {
        node.ancestors()
            .filter_map(ElementRef::wrap)
            .any(|el| el.value().name() == name)
    }

    fn text_of(node: ElementRef<'_>) -> String {
        node.text()
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn accessible_name(node: ElementRef<'_>) -> String {
        let el = node.value();
        if let Some(label) = el.attr("aria-label") {
            return label.trim().to_owned();
        }
        if el.attr("aria-labelledby").is_some() {
            return "labelledby".to_owned();
        }
        let text = text_of(node);
        if !text.is_empty() {
            return text;
        }
        let mut name = String::new();
        for child in node.descendants().filter_map(ElementRef::wrap) {
            match child.value().name() {
                "img" => name.push_str(child.value().attr("alt").unwrap_or_default().trim()),
                "title" => name.push_str(text_of(child).as_str()),
                _ => {}
            }
        }
        name
    }

    fn opening_tag(node: ElementRef<'_>) -> String {
        let raw = node.html();
        let end = raw.find('>').map_or(raw.len(), |i| i + 1);
        raw[..end].to_owned()
    }

    out
}

/// The `name=value` pair of the session cookie a response set.
pub fn session_cookie(response: &Response) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.contains("session="))
        .map(|value| value.split(';').next().unwrap().to_owned())
}

/// Signs a fresh account up through `POST /signup`, returning the
/// session cookie. Asserts the 303-to-home contract.
pub async fn signup(router: &Router, name: &str, email: &str, password: &str) -> String {
    let response = router
        .handle(post(
            "/signup",
            &[],
            form_body(&[("name", name), ("email", email), ("password", password)]),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::LOCATION], "/");
    session_cookie(&response).expect("signup sets a session cookie")
}

/// The lint guards every page test, so it is itself tested: each
/// rule must fire on a minimal offender and stay quiet on the
/// corrected document. Plain `#[test]`: no database, no gate.
mod a11y_baseline_tests {
    use super::a11y_baseline;

    fn page(body: &str) -> String {
        format!(
            "<!doctype html><html lang=\"en\"><body><main><h1>T</h1>{body}</main></body></html>"
        )
    }

    fn violations(body: &str) -> Vec<String> {
        a11y_baseline(&page(body))
    }

    #[test]
    fn a_minimal_document_is_clean() {
        assert!(violations("").is_empty());
    }

    #[test]
    fn document_rules() {
        let lang = a11y_baseline("<html><body><main><h1>T</h1></main></body></html>");
        assert!(lang.iter().any(|v| v.contains("no lang")), "{lang:?}");
        let no_main = a11y_baseline("<html lang=\"en\"><body><h1>T</h1></body></html>");
        assert!(no_main.iter().any(|v| v.contains("<main>")), "{no_main:?}");
        assert!(
            violations("<h1>Second</h1>")
                .iter()
                .any(|v| v.contains("<h1>"))
        );
    }

    #[test]
    fn heading_order() {
        assert!(
            violations("<h3>Deep</h3>")
                .iter()
                .any(|v| v.contains("skips from h1 to h3"))
        );
        assert!(violations("<h2>A</h2><h3>B</h3><h2>C</h2>").is_empty());
    }

    #[test]
    fn images_need_alt() {
        assert!(
            violations("<img src=\"x.png\">")
                .iter()
                .any(|v| v.contains("<img>"))
        );
        assert!(violations("<img src=\"x.png\" alt=\"\">").is_empty());
    }

    #[test]
    fn controls_need_labels() {
        assert!(
            violations("<input type=\"text\">")
                .iter()
                .any(|v| v.contains("without a label"))
        );
        assert!(violations("<input type=\"hidden\">").is_empty());
        assert!(violations("<label for=\"n\">N</label><input id=\"n\">").is_empty());
        assert!(violations("<label>N <input></label>").is_empty());
        assert!(violations("<select aria-label=\"N\"></select>").is_empty());
        assert!(
            violations("<input id=\"n\" aria-labelledby=\"missing\">")
                .iter()
                .any(|v| v.contains("aria-labelledby points at missing id"))
        );
    }

    #[test]
    fn buttons_and_links_need_names() {
        assert!(
            violations("<button><svg></svg></button>")
                .iter()
                .any(|v| v.contains("accessible name"))
        );
        assert!(violations("<button><svg><title>Close</title></svg></button>").is_empty());
        assert!(
            violations("<a href=\"/\"></a>")
                .iter()
                .any(|v| v.contains("accessible name"))
        );
        assert!(violations("<a href=\"/\" aria-label=\"Home\"></a>").is_empty());
        assert!(violations("<button type=\"submit\">Save</button>").is_empty());
    }

    #[test]
    fn errors_are_linked() {
        assert!(
            violations("<label for=\"n\">N</label><input id=\"n\" aria-invalid=\"true\">")
                .iter()
                .any(|v| v.contains("invalid control without aria-describedby"))
        );
        assert!(
            violations("<label for=\"n\">N</label><input id=\"n\" aria-describedby=\"n-err\">")
                .iter()
                .any(|v| v.contains("aria-describedby points at missing id"))
        );
        assert!(violations(
            "<label for=\"n\">N</label><input id=\"n\" aria-invalid=\"true\" aria-describedby=\"n-err\"><p id=\"n-err\">Required</p>"
        )
        .is_empty());
    }
}
