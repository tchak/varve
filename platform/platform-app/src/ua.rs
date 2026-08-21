//! User-agent parsing for display: the stored raw string in, a
//! [`Browser`] out — family, major version, and OS family, the three
//! parts the security tab composes into a session row title
//! ("Chrome 129 · macOS"). Parsing happens at render time; the raw
//! string stays in storage untouched and still travels to the page
//! (as the row title's `title=` tooltip), so nothing is lost when
//! the dataset improves.
//!
//! The parser is the [`ua-parser`](https://docs.rs/ua-parser) crate
//! (the ua-parser org's Rust implementation of the uap standard)
//! over a vendored copy of uap-core's `regexes.yaml`:
//!
//! - file: `src/ua/regexes.yaml`
//! - source: <https://github.com/ua-parser/uap-core>, commit
//!   `6be33a16c486017459615186c8329ca759f7ee2f` (master, 2026-07-14)
//!
//! Only the user-agent and OS extractors are built — the device
//! infoset is never displayed, and each extractor is (per the crate
//! docs) costly to create — once, lazily, in a module-private
//! `LazyLock` static.

use std::sync::LazyLock;

/// The two extractors the module uses, compiled once from the
/// vendored dataset. Building is a few hundred milliseconds of regex
/// compilation, paid on the first session list render, not at
/// startup; the dataset is compiled into the binary
/// (`include_str!`), so this cannot fail on deployment-specific
/// state — a panic here means the vendored file itself is broken,
/// which the unit tests catch in CI.
struct Extractors {
    ua: ua_parser::user_agent::Extractor<'static>,
    os: ua_parser::os::Extractor<'static>,
}

static EXTRACTORS: LazyLock<Extractors> = LazyLock::new(|| {
    let regexes: ua_parser::Regexes<'static> =
        serde_yaml::from_str(include_str!("ua/regexes.yaml"))
            .expect("the vendored regexes.yaml parses");
    let ua = ua_parser::user_agent::Builder::new()
        .push_all(regexes.user_agent_parsers)
        .expect("the vendored user-agent regexes compile")
        .build()
        .expect("the user-agent prefilter builds");
    let os = ua_parser::os::Builder::new()
        .push_all(regexes.os_parsers)
        .expect("the vendored OS regexes compile")
        .build()
        .expect("the OS prefilter builds");
    Extractors { ua, os }
});

/// What a user-agent string says about the browser behind it, reduced
/// to the parts the UI shows. Kept as parts, not a composed string:
/// the page owns the display composition (and the icon choice keys on
/// [`family`](Self::family) alone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browser {
    /// The browser family per uap-core: `"Chrome"`, `"Firefox"`,
    /// `"Mobile Safari"`, `"Edge"`, …
    pub family: String,
    /// The major version, when the string carries one.
    pub major: Option<String>,
    /// The OS family (`"macOS"`, `"Windows"`, `"iOS"`, …), when the
    /// string identifies one.
    pub os: Option<String>,
}

/// Parses a stored user-agent string into a [`Browser`], or `None`
/// when the string identifies no browser: empty or blank input,
/// nothing in the dataset matching, or a match whose family (or, for
/// the OS part, whose OS) is the uap catch-all `"Other"` — a
/// non-answer the UI should replace with its localized fallback, not
/// print.
pub fn describe(user_agent: &str) -> Option<Browser> {
    let user_agent = user_agent.trim();
    if user_agent.is_empty() {
        return None;
    }
    let agent = EXTRACTORS.ua.extract(user_agent)?;
    if agent.family == "Other" {
        return None;
    }
    let os = EXTRACTORS
        .os
        .extract(user_agent)
        .map(|os| os.os)
        .filter(|os| os != "Other");
    Some(Browser {
        family: agent.family.into_owned(),
        major: agent.major.map(str::to_owned),
        os: os.map(std::borrow::Cow::into_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `describe` against a real string, unwrapped: these are all
    /// current, well-formed user agents.
    fn parsed(ua: &str) -> Browser {
        describe(ua).expect("a real browser UA parses")
    }

    #[test]
    fn chrome_on_macos() {
        let browser = parsed(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
        );
        assert_eq!(browser.family, "Chrome");
        assert_eq!(browser.major.as_deref(), Some("129"));
        assert_eq!(browser.os.as_deref(), Some("Mac OS X"));
    }

    #[test]
    fn chrome_on_windows() {
        let browser = parsed(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
        );
        assert_eq!(browser.family, "Chrome");
        assert_eq!(browser.major.as_deref(), Some("129"));
        assert_eq!(browser.os.as_deref(), Some("Windows"));
    }

    #[test]
    fn firefox_on_linux() {
        let browser =
            parsed("Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0");
        assert_eq!(browser.family, "Firefox");
        assert_eq!(browser.major.as_deref(), Some("130"));
        assert_eq!(browser.os.as_deref(), Some("Linux"));
    }

    #[test]
    fn safari_on_macos() {
        let browser = parsed(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/17.6 Safari/605.1.15",
        );
        assert_eq!(browser.family, "Safari");
        assert_eq!(browser.major.as_deref(), Some("17"));
        assert_eq!(browser.os.as_deref(), Some("Mac OS X"));
    }

    #[test]
    fn safari_on_ios_is_the_mobile_safari_family() {
        let browser = parsed(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_6 like Mac OS X) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/17.6 Mobile/15E148 Safari/604.1",
        );
        assert_eq!(browser.family, "Mobile Safari");
        assert_eq!(browser.major.as_deref(), Some("17"));
        assert_eq!(browser.os.as_deref(), Some("iOS"));
    }

    #[test]
    fn edge_on_windows() {
        let browser = parsed(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36 Edg/129.0.0.0",
        );
        assert_eq!(browser.family, "Edge");
        assert_eq!(browser.major.as_deref(), Some("129"));
        assert_eq!(browser.os.as_deref(), Some("Windows"));
    }

    #[test]
    fn garbage_and_empty_are_none() {
        // In this crate an unmatched UA is `extract() == None`, not a
        // synthesized `"Other"` family as in the uap reference
        // implementations — no parser in the vendored dataset emits
        // an `"Other"` family (the `describe` guard for it is
        // defensive). Garbage therefore exercises the same path an
        // "Other"-family UA would.
        assert_eq!(describe("definitely not a browser"), None);
        assert_eq!(describe(""), None);
        assert_eq!(describe("   "), None);
    }

    #[test]
    fn an_other_os_is_dropped_not_shown() {
        // The dataset *does* emit `"Other"` as an OS (the
        // AspiegelBot/PetalBot parser maps it explicitly); the
        // non-answer is dropped so the page never prints "· Other".
        let browser = parsed("Mozilla/5.0 AspiegelBot");
        assert_eq!(browser.family, "AspiegelBot");
        assert_eq!(browser.os, None);
    }
}
