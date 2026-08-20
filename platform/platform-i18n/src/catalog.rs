//! Message catalogs: id → compiled template per locale, and the
//! locale-fallback walk over them.
//!
//! Deliberately absent: a container file format. PLATFORM.md P.3
//! ships English and French catalogs *later* (no UI strings exist
//! yet); how they are stored on disk (TOML, JSON, directories of
//! `.mf2` files, ...) is an open design point, so loading takes plain
//! `(id, source)` pairs and nothing else — the machinery without the
//! format.

use icu::locale::Locale;

use crate::{Args, CompileError, FormatError, Formatted, MessageTemplate};

/// One locale's messages: message id → compiled [`MessageTemplate`].
///
/// Compilation is eager: [`Catalog::from_pairs`] compiles every
/// message up front and reports **all** failures with their ids, not
/// just the first — a translation drop is validated in one pass, and
/// format time never parses.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    messages: std::collections::BTreeMap<String, MessageTemplate>,
}

impl Catalog {
    /// An empty catalog.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compile `(id, source)` pairs into a catalog. On failure the
    /// error carries *every* failing id with its [`CompileError`].
    /// A repeated id keeps the last occurrence, like a map insert.
    pub fn from_pairs<I, K, S>(pairs: I) -> Result<Self, CatalogError>
    where
        I: IntoIterator<Item = (K, S)>,
        K: Into<String>,
        S: AsRef<str>,
    {
        let mut catalog = Self::new();
        let mut errors: Vec<(String, CompileError)> = Vec::new();
        for (id, source) in pairs {
            let id = id.into();
            match MessageTemplate::compile(source.as_ref()) {
                Ok(template) => {
                    catalog.messages.insert(id, template);
                }
                Err(error) => errors.push((id, error)),
            }
        }
        if errors.is_empty() {
            Ok(catalog)
        } else {
            Err(CatalogError { errors })
        }
    }

    /// Insert one already-compiled template under `id`, replacing any
    /// previous one.
    pub fn insert(&mut self, id: impl Into<String>, template: MessageTemplate) {
        self.messages.insert(id.into(), template);
    }

    /// Look up a compiled template by message id.
    pub fn get(&self, id: &str) -> Option<&MessageTemplate> {
        self.messages.get(id)
    }

    /// Number of messages in the catalog.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the catalog holds no messages.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Iterate message ids in sorted order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.messages.keys().map(String::as_str)
    }
}

/// All compile failures from one [`Catalog::from_pairs`] pass, each
/// with the id of the message that failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{} message(s) failed to compile: {}", errors.len(), failing_ids(errors))]
pub struct CatalogError {
    /// `(message id, compile error)`, in input order.
    pub errors: Vec<(String, CompileError)>,
}

fn failing_ids(errors: &[(String, CompileError)]) -> String {
    errors
        .iter()
        .map(|(id, error)| format!("'{id}' ({error})"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Catalogs for several locales, with an explicit caller-supplied
/// fallback chain.
///
/// The chain is a suffix appended to every request: with fallback
/// `[fr, en]`, a request for `fr-CH` walks `fr-CH → fr → en` and the
/// first catalog holding the id wins. Nothing is inferred from locale
/// structure — the platform decides its own chain (PLATFORM.md P.3:
/// locale is a plain argument; this crate does not negotiate).
///
/// A message found in a fallback locale is formatted **with that
/// locale**, not the requested one: message text and CLDR data
/// (plural categories, separators) must agree — formatting an English
/// sentence with French plural rules would select the wrong variant.
#[derive(Debug, Clone, Default)]
pub struct Catalogs {
    catalogs: Vec<(Locale, Catalog)>,
    fallback: Vec<Locale>,
}

impl Catalogs {
    /// A catalog set with the given fallback chain (may be empty).
    pub fn new(fallback: Vec<Locale>) -> Self {
        Self {
            catalogs: Vec::new(),
            fallback,
        }
    }

    /// Insert (or replace) the catalog for `locale`.
    pub fn insert(&mut self, locale: Locale, catalog: Catalog) {
        if let Some(slot) = self.catalogs.iter_mut().find(|(l, _)| *l == locale) {
            slot.1 = catalog;
        } else {
            self.catalogs.push((locale, catalog));
        }
    }

    /// The catalog registered for exactly `locale`, if any.
    pub fn get(&self, locale: &Locale) -> Option<&Catalog> {
        self.catalogs
            .iter()
            .find(|(l, _)| l == locale)
            .map(|(_, c)| c)
    }

    /// Format message `id` for `locale`, walking the fallback chain.
    ///
    /// Returns [`FormatError::UnknownMessage`] (carrying the id) when
    /// no catalog on the chain holds `id` — a missing translation
    /// falls back, a missing *message* is a programming error worth
    /// surfacing.
    pub fn format(&self, locale: &Locale, id: &str, args: &Args) -> Result<Formatted, FormatError> {
        for candidate in std::iter::once(locale).chain(self.fallback.iter()) {
            if let Some(template) = self.get(candidate).and_then(|catalog| catalog.get(id)) {
                return template.format(candidate, args);
            }
        }
        Err(FormatError::UnknownMessage { id: id.to_owned() })
    }
}
