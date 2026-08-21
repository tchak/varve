//! Request locale resolution and the string-formatting helpers.
//!
//! PLATFORM.md P.3: locale is resolved **here** and nowhere else —
//! no crate below `platform-app` knows what a locale is. The order,
//! per P.7's principal model:
//!
//! 1. the principal's stored locale preference, when it names a
//!    supported language;
//! 2. the first supported match in `Accept-Language`, by descending
//!    quality;
//! 3. English.
//!
//! Resolution always lands on one of [`SUPPORTED_LOCALES`] exactly
//! (`"fr-CH"` resolves to `fr`), because the catalogs in
//! [`crate::strings`] are registered per base locale and the
//! [`platform_i18n::Catalogs`] fallback chain is `[en]`: an
//! unnormalized regional tag would silently skip the French catalog.
//!
//! [`request_locale`] resolves it on demand from the (memoized)
//! principal and the request headers — a request function, not a
//! layer; [`t`] / [`t_args`] format message ids against the app's
//! catalogs with it. Format-time
//! [`platform_i18n::Warning`]s (missing argument, unknown function)
//! are already embedded as MF2 fallback text in the output; P0 drops
//! the warning list rather than logging it — observability arrives
//! with the platform's logging story.

use platform_i18n::{Args, Catalogs, Locale};
use topcoat::{
    context::{Cx, app_context},
    router::{header, request},
};

/// The languages the platform ships catalogs for (P.3: English and
/// French), by primary subtag. Order is meaningless; resolution
/// order comes from the request.
pub const SUPPORTED_LOCALES: &[&str] = &["en", "fr"];

/// The final fallback, and the catalogs' fallback chain.
pub const DEFAULT_LOCALE: &str = "en";

/// Parses one of the supported base tags; infallible by construction.
fn supported_locale(tag: &str) -> Locale {
    platform_i18n::locale(tag).expect("supported locale literals parse")
}

/// The default locale as a typed value.
pub fn default_locale() -> Locale {
    supported_locale(DEFAULT_LOCALE)
}

/// Maps a candidate tag (`"fr"`, `"FR-ch"`, `"en_US"`) to the
/// supported base locale of its primary subtag, or `None` when the
/// language is not supported. `"*"` (an `Accept-Language` wildcard)
/// maps to the default.
fn match_supported(tag: &str) -> Option<Locale> {
    let tag = tag.trim();
    if tag == "*" {
        return Some(default_locale());
    }
    let primary = tag
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    SUPPORTED_LOCALES
        .iter()
        .find(|supported| **supported == primary)
        .map(|supported| supported_locale(supported))
}

/// Resolves the request locale from the principal's stored preference
/// and the `Accept-Language` header — the order in the module docs.
/// A preference for an unsupported language falls through to the
/// header rather than being honored blindly: the catalogs could not
/// serve it anyway.
pub fn resolve_locale(preference: Option<&str>, accept_language: Option<&str>) -> Locale {
    if let Some(preference) = preference
        && let Some(locale) = match_supported(preference)
    {
        return locale;
    }
    if let Some(header) = accept_language
        && let Some(locale) = negotiate(header)
    {
        return locale;
    }
    default_locale()
}

/// Walks an `Accept-Language` value by descending quality (stable on
/// ties, so header order breaks them) and returns the first supported
/// match. Entries with a malformed or zero `q` are skipped.
fn negotiate(header: &str) -> Option<Locale> {
    let mut candidates: Vec<(u16, &str)> = Vec::new();
    for entry in header.split(',') {
        let mut parts = entry.split(';');
        let tag = parts.next().unwrap_or_default().trim();
        if tag.is_empty() {
            continue;
        }
        let quality = parts
            .find_map(|param| param.trim().strip_prefix("q="))
            .map_or(Some(1000), |q| {
                q.trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|q| (0.0..=1.0).contains(q))
                    .map(|q| (q * 1000.0) as u16)
            });
        match quality {
            Some(0) | None => continue,
            Some(quality) => candidates.push((quality, tag)),
        }
    }
    candidates.sort_by_key(|(quality, _)| std::cmp::Reverse(*quality));
    candidates
        .into_iter()
        .find_map(|(_, tag)| match_supported(tag))
}

/// The locale resolved for this request: [`resolve_locale`] over the
/// principal's stored preference ([`crate::auth::principal`],
/// memoized — the session lookup is shared with everything else that
/// asks) and the `Accept-Language` header. The resolution itself is
/// string work, so it is not memoized.
pub async fn request_locale(cx: &Cx) -> topcoat::Result<Locale> {
    let principal = crate::auth::principal(cx).await?;
    Ok(resolve_locale(
        principal.and_then(|principal| principal.locale.as_deref()),
        request::headers(cx)
            .get(header::ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok()),
    ))
}

/// Formats message `id` for the request locale with no arguments.
/// Every user-visible string in the shell's views goes through here
/// or [`t_args`] — no bare literals in views.
pub async fn t(cx: &Cx, id: &str) -> topcoat::Result<String> {
    t_args(cx, id, &Args::new()).await
}

/// Formats message `id` for the request locale with `args`.
pub async fn t_args(cx: &Cx, id: &str, args: &Args) -> topcoat::Result<String> {
    format_in(cx, &request_locale(cx).await?, id, args)
}

/// Formats message `id` in an explicit `locale` against the app's
/// catalogs. An unknown id is an error (a missing *message* is a
/// programming error; a missing *translation* falls back inside the
/// catalogs).
pub fn format_in(cx: &Cx, locale: &Locale, id: &str, args: &Args) -> topcoat::Result<String> {
    let catalogs = app_context::<Catalogs>(cx);
    Ok(catalogs.format(locale, id, args)?.text)
}

#[cfg(test)]
mod tests {
    use platform_i18n::ArgValue;
    use topcoat::context::CxTestBuilder;

    use super::*;

    fn tag(locale: &Locale) -> String {
        locale.to_string()
    }

    #[test]
    fn preference_beats_header() {
        let locale = resolve_locale(Some("fr"), Some("en"));
        assert_eq!(tag(&locale), "fr");
    }

    #[test]
    fn preference_normalizes_regional_tags() {
        assert_eq!(tag(&resolve_locale(Some("fr-CH"), None)), "fr");
        assert_eq!(tag(&resolve_locale(Some("en_US"), None)), "en");
        assert_eq!(tag(&resolve_locale(Some("FR"), None)), "fr");
    }

    #[test]
    fn unsupported_preference_falls_through_to_header() {
        let locale = resolve_locale(Some("de"), Some("fr"));
        assert_eq!(tag(&locale), "fr");
    }

    #[test]
    fn header_quality_orders_candidates() {
        let locale = resolve_locale(None, Some("de, fr;q=0.8, en;q=0.9"));
        assert_eq!(tag(&locale), "en");
    }

    #[test]
    fn header_regional_tag_matches_base_language() {
        let locale = resolve_locale(None, Some("fr-CH, de;q=0.9"));
        assert_eq!(tag(&locale), "fr");
    }

    #[test]
    fn header_ties_keep_header_order() {
        let locale = resolve_locale(None, Some("fr, en"));
        assert_eq!(tag(&locale), "fr");
    }

    #[test]
    fn header_wildcard_means_default() {
        let locale = resolve_locale(None, Some("de, *;q=0.1"));
        assert_eq!(tag(&locale), "en");
    }

    #[test]
    fn zero_or_malformed_quality_is_skipped() {
        assert_eq!(tag(&resolve_locale(None, Some("fr;q=0, en;q=0.5"))), "en");
        assert_eq!(
            tag(&resolve_locale(None, Some("fr;q=oops, en;q=0.5"))),
            "en"
        );
    }

    #[test]
    fn no_signal_at_all_is_english() {
        assert_eq!(tag(&resolve_locale(None, None)), "en");
        assert_eq!(tag(&resolve_locale(None, Some("de, ja;q=0.9"))), "en");
        assert_eq!(tag(&resolve_locale(Some("de"), None)), "en");
    }

    fn catalogs_cx() -> Cx {
        CxTestBuilder::new()
            .app_context(crate::strings::catalogs())
            .build()
    }

    #[test]
    fn format_in_formats_for_the_given_locale() {
        let cx = catalogs_cx();
        let fr = supported_locale("fr");
        assert_eq!(
            format_in(&cx, &fr, "nav.sign-out", &Args::new()).unwrap(),
            "Se déconnecter"
        );

        let mut args = Args::new();
        args.insert("name".to_owned(), ArgValue::from("Élodie"));
        assert_eq!(
            format_in(&cx, &fr, "home.greeting", &args).unwrap(),
            "Bonjour Élodie."
        );
    }

    #[test]
    fn format_in_english_is_the_default_locale() {
        let cx = catalogs_cx();
        assert_eq!(
            format_in(&cx, &default_locale(), "nav.sign-out", &Args::new()).unwrap(),
            "Sign out"
        );
    }

    #[test]
    fn format_in_rejects_an_unknown_message_id() {
        let cx = catalogs_cx();
        assert!(format_in(&cx, &default_locale(), "no.such.message", &Args::new()).is_err());
    }
}
