//! The MF2 message runtime over ICU4X, and the catalog machinery the
//! English + French catalogs will load into (PLATFORM.md P.3; the
//! catalogs themselves ship with the first UI strings, not here).
//!
//! Messages are **MessageFormat 2** (Unicode's successor to classic
//! MessageFormat, final since LDML 47) — the stable contract is the
//! spec-standard message text, not any crate. The runtime is the
//! hand-rolled interpreter P.9 Q8 settled on: parse with
//! `ox_mf2_parser` (MIT, spec-final CST; evaluated from the CST
//! because its `SemanticModel` lowering is lint-only and drops option
//! values), delegate plural selection, number formatting, and date
//! formatting to ICU4X 2.x compiled data. When ICU4X ships an MF2
//! formatter, swap it in behind this API and delete ours — the
//! parser and formatter never leak through the crate surface.
//!
//! The shape is **compile once, format many** (Q8's implementation
//! notes): [`MessageTemplate::compile`] lowers the CST to a small
//! owned IR — text runs, placeholders, ordered declarations, match
//! variants — and [`MessageTemplate::format`] walks only that IR.
//! Everything the spec calls a *data model* or *syntax* error is a
//! [`CompileError`]; format time follows the spec's fallback
//! behavior instead of failing: a missing argument, unknown function,
//! or bad operand renders its fallback representation (`{$var}`,
//! `{:func}`) into the output and records a [`Warning`], and unknown
//! options warn and are ignored. [`Formatted`] carries both the text
//! and the warnings; `Err` at format time is reserved for what makes
//! the whole format meaningless (an unknown message id in
//! [`Catalogs::format`], an ICU4X data failure).
//!
//! Locale is a plain argument resolved in `platform-app`
//! (`Accept-Language`, principal preference); no crate below this one
//! knows what a locale is, and this one only accepts a typed
//! [`Locale`] (see [`locale`]).
//!
//! **Deliberately absent, so far** (each waits for its first real
//! use — implement here, not around this crate, when a page needs
//! it): a `:time` function and `:datetime`'s `timeStyle` (the
//! formatter is built on ICU4X's date-only YMD fieldsets, so
//! time-of-day cannot render at all; adding it also inherits the
//! time-zone question, PLATFORM.md P.9 Q11 — this crate formats the
//! civil date/time it is handed and must stay zone-ignorant);
//! **markup placeholders** (`{#b}...{/b}` is a
//! [`CompileError::Unsupported`] — a message cannot carry an inline
//! link or emphasis, so copy needing one must currently split into
//! fragments, which mangles translations; the fix is lowering markup
//! to open/close parts in the IR and letting the caller map them to
//! views); `:number` beyond `style=decimal` + fraction digits (no
//! percent/currency/unit, no ordinal selection); `:date style=full`
//! renders as `long` (no weekday); no bidi isolation of placeholder
//! output (mandatory before any RTL locale ships).
//!
//! Formatted output is **opaque** (Q8): CLDR French uses U+202F
//! NARROW NO-BREAK SPACE as the group separator — naive snapshot
//! assertions on "1 000 000" will break. Compare against literals
//! written with explicit escapes, as this crate's own tests do.
//!
//! ```
//! use platform_i18n::{ArgValue, Args, MessageTemplate, locale};
//!
//! let template = MessageTemplate::compile(
//!     ".input {$count :number}\n\
//!      .match $count\n\
//!      one {{You have {$count} item}}\n\
//!      * {{You have {$count} items}}",
//! )
//! .unwrap();
//! let en = locale("en").unwrap();
//! let mut args = Args::new();
//! args.insert("count".into(), ArgValue::from(3));
//! let out = template.format(&en, &args).unwrap();
//! assert_eq!(out.text, "You have 3 items");
//! assert!(out.warnings.is_empty());
//! ```

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

mod catalog;
mod compile;
mod format;
mod ir;

pub use catalog::{Catalog, CatalogError, Catalogs};
/// Arbitrary-precision decimal (re-exported from `fixed_decimal`, the
/// same type ICU4X formats): the lossless way to hand a fractional
/// number to [`ArgValue`].
pub use fixed_decimal::Decimal;
/// Calendar date (re-exported from `icu::calendar`); [`ArgValue`]
/// carries `Date<Iso>` — construct via [`Date::try_new_iso`] or
/// [`ArgValue::date`].
pub use icu::calendar::{Date, Iso};
/// A typed BCP-47 locale (re-exported from `icu::locale`). Parse one
/// with [`locale`] or [`Locale::try_from_str`].
pub use icu::locale::Locale;

/// Parse a BCP-47 locale string (`"fr"`, `"fr-CH"`, ...) into a typed
/// [`Locale`]. Convenience over [`Locale::try_from_str`] with an
/// error that keeps the offending input.
pub fn locale(s: &str) -> Result<Locale, LocaleError> {
    Locale::try_from_str(s).map_err(|source| LocaleError {
        input: s.to_owned(),
        source,
    })
}

/// A string that is not a BCP-47 locale.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("'{input}' is not a valid BCP-47 locale: {source}")]
pub struct LocaleError {
    /// The rejected input.
    pub input: String,
    /// The underlying `icu` parse error.
    #[source]
    pub source: icu::locale::ParseError,
}

/// An argument value handed to [`MessageTemplate::format`].
///
/// Numbers are kept exact: integers as themselves, fractions as
/// [`Decimal`]. There is deliberately no `f64` variant — `From<f64>`
/// converts at the boundary via `fixed_decimal`'s `ryu` feature
/// (shortest lossless representation), so a float can never smuggle
/// binary-rounding artifacts into exact-match plural keys.
#[derive(Debug, Clone, PartialEq)]
pub enum ArgValue {
    /// A plain string.
    String(String),
    /// A signed integer.
    Int(i64),
    /// An unsigned integer (covers `u64` values above `i64::MAX`).
    UInt(u64),
    /// An exact decimal number.
    Decimal(Decimal),
    /// An ISO calendar date.
    Date(Date<Iso>),
}

impl ArgValue {
    /// An ISO date argument from year/month/day, rejecting impossible
    /// dates (a `Date` in an [`ArgValue`] is always valid, so format
    /// time has no date-validation failure path).
    pub fn date(year: i32, month: u8, day: u8) -> Result<Self, icu::calendar::RangeError> {
        Ok(ArgValue::Date(Date::try_new_iso(year, month, day)?))
    }
}

impl From<&str> for ArgValue {
    fn from(s: &str) -> Self {
        ArgValue::String(s.to_owned())
    }
}

impl From<String> for ArgValue {
    fn from(s: String) -> Self {
        ArgValue::String(s)
    }
}

impl From<i64> for ArgValue {
    fn from(i: i64) -> Self {
        ArgValue::Int(i)
    }
}

impl From<i32> for ArgValue {
    fn from(i: i32) -> Self {
        ArgValue::Int(i64::from(i))
    }
}

impl From<u64> for ArgValue {
    fn from(u: u64) -> Self {
        ArgValue::UInt(u)
    }
}

impl From<u32> for ArgValue {
    fn from(u: u32) -> Self {
        ArgValue::UInt(u64::from(u))
    }
}

impl From<Decimal> for ArgValue {
    fn from(d: Decimal) -> Self {
        ArgValue::Decimal(d)
    }
}

impl From<Date<Iso>> for ArgValue {
    fn from(d: Date<Iso>) -> Self {
        ArgValue::Date(d)
    }
}

/// Lossless conversion via `fixed_decimal`'s `ryu` feature
/// (`FloatPrecision::RoundTrip`: exactly the digits needed to recover
/// the float, no trailing zeros). Q8 left f64 handling open between
/// this and a `format!`-roundtrip; `ryu` wins because it is the same
/// shortest-representation algorithm with the conversion done in one
/// audited place instead of through a formatting detour. Non-finite
/// values (NaN, ±inf) have no decimal form and become their display
/// string — `:number` then warns and falls back at format time, the
/// spec behavior for a non-numeric operand.
impl From<f64> for ArgValue {
    fn from(f: f64) -> Self {
        match Decimal::try_from_f64(f, fixed_decimal::FloatPrecision::RoundTrip) {
            Ok(d) => ArgValue::Decimal(d),
            Err(_) => ArgValue::String(format!("{f}")),
        }
    }
}

/// Named arguments for one format call.
pub type Args = BTreeMap<String, ArgValue>;

/// A compiled MF2 message: parse and lower once with
/// [`compile`](Self::compile), then [`format`](Self::format) any number of
/// times, in any locale, from the owned IR — no re-parsing, no CST at
/// format time (Q8). Cheap to clone, `Send + Sync`, and safe to share
/// behind a [`Catalog`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageTemplate {
    ir: ir::Ir,
}

impl MessageTemplate {
    /// Parse `source` as MF2 and lower it to the internal IR.
    ///
    /// All syntax diagnostics and data model errors (duplicate
    /// declarations, variant-key arity mismatches, a `.match` without
    /// an all-`*` fallback variant) are rejected here — a template
    /// that compiles can always be formatted to *some* text.
    pub fn compile(source: &str) -> Result<Self, CompileError> {
        Ok(Self {
            ir: compile::compile(source)?,
        })
    }

    /// Format this message in `locale` with `args`.
    ///
    /// Runtime resolution problems (missing argument, unknown
    /// function or option, bad operand, non-numeric selector) follow
    /// the MF2 fallback behavior: the output text carries the
    /// fallback representation (`{$var}`, `{:func}`) or ignores the
    /// option, and [`Formatted::warnings`] records what happened.
    /// `Err` is reserved for an ICU4X data failure
    /// ([`FormatError::Icu`]), which should not occur with compiled
    /// data.
    pub fn format(&self, locale: &Locale, args: &Args) -> Result<Formatted, FormatError> {
        format::format(&self.ir, locale, args)
    }
}

/// The result of a format call: the text, always, plus any warnings
/// recorded while producing it (per Q8: warn-and-continue, never fail
/// the whole message over one placeholder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatted {
    /// The formatted message text. Treat as opaque — French output
    /// contains U+202F group separators (Q8's snapshot warning).
    pub text: String,
    /// What went wrong along the way, if anything. Empty means a
    /// clean format; the platform decides whether to log or ignore.
    pub warnings: Vec<Warning>,
}

/// A non-fatal problem recorded during formatting. The spec's
/// resolution errors: each corresponds to a fallback (or an ignored
/// option) already embedded in the output text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// A `$name` with no declaration and no argument; the output
    /// carries `{$name}`.
    UnresolvedVariable(String),
    /// A `:function` this runtime does not implement; the output
    /// carries the expression's fallback.
    UnknownFunction(String),
    /// An option name the function does not recognize; the option
    /// was ignored.
    UnknownOption {
        /// Function the option was passed to.
        function: String,
        /// The unrecognized option name.
        option: String,
    },
    /// A recognized option with a value this runtime cannot honor;
    /// the option was ignored.
    UnsupportedOption {
        /// Function the option was passed to.
        function: String,
        /// The option name.
        option: String,
        /// The rejected value.
        value: String,
    },
    /// An operand the function cannot use (non-numeric string to
    /// `:number`, missing operand, ...); the output carries the
    /// expression's fallback.
    BadOperand {
        /// The function that rejected its operand.
        function: String,
        /// Human-readable detail.
        detail: String,
    },
    /// A `.match` selector that did not resolve to a number; it
    /// matches only `*` variants.
    SelectorNotNumeric(String),
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Warning::UnresolvedVariable(name) => write!(f, "unresolved variable ${name}"),
            Warning::UnknownFunction(name) => write!(f, "unknown function :{name}"),
            Warning::UnknownOption { function, option } => {
                write!(f, "unknown option {option} on :{function}")
            }
            Warning::UnsupportedOption {
                function,
                option,
                value,
            } => write!(f, "unsupported option {option}={value} on :{function}"),
            Warning::BadOperand { function, detail } => {
                write!(f, "bad operand for :{function}: {detail}")
            }
            Warning::SelectorNotNumeric(name) => {
                write!(f, "selector ${name} is not a number")
            }
        }
    }
}

/// Compile-time failure: the message never becomes a
/// [`MessageTemplate`]. Everything here is knowable from the source
/// alone — syntax diagnostics from the parser, plus the spec's data
/// model errors checked during lowering.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    /// Parser diagnostics (syntax errors), one rendered string per
    /// diagnostic, with source spans.
    #[error("MF2 syntax: {}", .0.join("; "))]
    Syntax(Vec<String>),
    /// MF2 the spec allows but this runtime does not lower yet
    /// (markup placeholders, non-variable selectors, ...).
    #[error("unsupported MF2 construct: {0}")]
    Unsupported(String),
    /// A name declared more than once (spec data model error).
    #[error("duplicate declaration of ${0}")]
    DuplicateDeclaration(String),
    /// A variant whose key count differs from the selector count
    /// (spec data model error).
    #[error("variant has {keys} key(s) for {selectors} selector(s)")]
    VariantKeyMismatch {
        /// Number of selectors in the `.match`.
        selectors: usize,
        /// Number of keys on the offending variant.
        keys: usize,
    },
    /// A `.match` with no all-`*` variant (spec data model error:
    /// missing fallback variant). Rejecting it here is what makes
    /// format-time selection total.
    #[error("matcher has no fallback variant (all-`*` keys)")]
    MissingFallbackVariant,
}

/// Format-time failure. Deliberately small: per Q8, resolution
/// problems are [`Warning`]s with fallback text, so only what makes
/// the whole format meaningless lands here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormatError {
    /// [`Catalogs::format`] found the id in no catalog on the
    /// fallback chain: a missing *message* (as opposed to a missing
    /// translation, which falls back) is a programming error.
    #[error("unknown message '{id}' in the requested locale and its fallback chain")]
    UnknownMessage {
        /// The message id that was requested.
        id: String,
    },
    /// ICU4X could not construct a formatter or plural rules. Should
    /// not occur with compiled data; surfaced rather than swallowed.
    #[error("icu: {0}")]
    Icu(String),
}
