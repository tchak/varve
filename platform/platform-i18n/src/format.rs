//! IR → text: the format half of the Q8 split. Walks the compiled
//! [`ir`](crate::ir) only — no parsing, no CST — and delegates every
//! locale-sensitive operation to ICU4X compiled data: plural
//! selection (`icu::plurals` cardinal rules), number formatting
//! (`DecimalFormatter` over `Decimal`), date formatting
//! (`DateTimeFormatter` YMD fieldsets).
//!
//! Error philosophy (Q8, and the MF2 spec's own): *resolution* errors
//! — missing argument, unknown function, unknown option, bad operand,
//! non-numeric selector — never abort the format. They emit the
//! expression's fallback representation (`{$var}`, `{|lit|}`,
//! `{:func}`) or ignore the option, and record a [`Warning`]; the
//! caller always gets text. `Err` is reserved for
//! [`FormatError::Icu`], an ICU4X data failure that should not occur
//! with compiled data — nothing else can make the whole format
//! meaningless, because the locale is typed and compilation already
//! rejected every data model error.

use std::collections::BTreeMap;

use fixed_decimal::Decimal;
use icu::calendar::{Date, Iso};
use icu::datetime::{DateTimeFormatter, fieldsets::YMD};
use icu::decimal::DecimalFormatter;
use icu::decimal::options::{DecimalFormatterOptions, GroupingStrategy};
use icu::locale::Locale;
use icu::plurals::{PluralCategory, PluralRules};

use crate::compile::nfc;
use crate::ir::{Body, Expr, Func, Ir, Key, Operand, OptValue, Part, Pattern, Variant};
use crate::{ArgValue, Args, FormatError, Formatted, Warning};

/// Format one compiled message.
pub(crate) fn format(ir: &Ir, locale: &Locale, args: &Args) -> Result<Formatted, FormatError> {
    let fmt = Fmt { locale, args };
    let mut warnings: Vec<Warning> = Vec::new();
    // Declarations: evaluated exactly once, in source order, into a
    // resolved-value environment (Q8). A declaration sees every
    // earlier declaration through `env` and everything else through
    // `args`; forward references simply miss `env` and read the
    // caller's argument (or warn-and-fall-back), never recurse.
    let mut env: BTreeMap<String, ResolvedValue> = BTreeMap::new();
    for decl in &ir.decls {
        let value = fmt.eval_expr(&decl.expr, &env, &mut warnings)?;
        env.insert(decl.name.clone(), value);
    }
    let text = match &ir.body {
        Body::Pattern(pattern) => fmt.pattern(pattern, &env, &mut warnings)?,
        Body::Match {
            selectors,
            variants,
            fallback_variant,
        } => {
            let variant =
                fmt.select(selectors, variants, *fallback_variant, &env, &mut warnings)?;
            fmt.pattern(&variant.pattern, &env, &mut warnings)?
        }
    };
    Ok(Formatted { text, warnings })
}

/// A value after resolution. `formatted` caches the locale-rendered
/// string once an annotation ran, so `.local $p = {$n :number ...}`
/// followed by `{$p}` renders the annotated form — the environment
/// stores these, which is what "declarations evaluated once" means
/// observationally.
#[derive(Debug, Clone)]
enum ResolvedValue {
    Str(String),
    Num {
        decimal: Decimal,
        formatted: Option<String>,
    },
    Date {
        date: Date<Iso>,
        formatted: Option<String>,
    },
    /// A resolution failure, carrying the fallback representation it
    /// renders as. Selecting on it matches only `*`. The warning was
    /// recorded when the failure happened; propagation is silent.
    Fallback(String),
}

struct Fmt<'a> {
    locale: &'a Locale,
    args: &'a Args,
}

impl Fmt<'_> {
    // ── pattern ─────────────────────────────────────────────────────────

    fn pattern(
        &self,
        pattern: &Pattern,
        env: &BTreeMap<String, ResolvedValue>,
        warnings: &mut Vec<Warning>,
    ) -> Result<String, FormatError> {
        let mut out = String::new();
        for part in &pattern.parts {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::Placeholder(expr) => {
                    let value = self.eval_expr(expr, env, warnings)?;
                    out.push_str(&self.render(value)?);
                }
            }
        }
        Ok(out)
    }

    // ── expression evaluation ───────────────────────────────────────────

    fn eval_expr(
        &self,
        expr: &Expr,
        env: &BTreeMap<String, ResolvedValue>,
        warnings: &mut Vec<Warning>,
    ) -> Result<ResolvedValue, FormatError> {
        match expr {
            Expr::Operand { operand, func } => {
                let value = match operand {
                    Operand::Var(name) => self.resolve_var(name, env, warnings),
                    Operand::Literal(value) => ResolvedValue::Str(value.clone()),
                };
                match func {
                    Some(func) => self.apply(func, Some(value), expr, env, warnings),
                    None => Ok(value),
                }
            }
            Expr::Func(func) => self.apply(func, None, expr, env, warnings),
        }
    }

    /// Resolve `$name`: the declaration environment shadows the
    /// caller's arguments. Argument lookup is exact first, then
    /// NFC-normalized (IR names are already NFC; a caller may hand us
    /// a decomposed key). Missing → fallback `{$name}` plus warning.
    fn resolve_var(
        &self,
        name: &str,
        env: &BTreeMap<String, ResolvedValue>,
        warnings: &mut Vec<Warning>,
    ) -> ResolvedValue {
        if let Some(value) = env.get(name) {
            return value.clone();
        }
        let arg = self.args.get(name).or_else(|| {
            self.args
                .iter()
                .find(|(key, _)| nfc(key) == name)
                .map(|(_, value)| value)
        });
        match arg {
            Some(ArgValue::String(s)) => ResolvedValue::Str(s.clone()),
            Some(ArgValue::Int(i)) => ResolvedValue::Num {
                decimal: Decimal::from(*i),
                formatted: None,
            },
            Some(ArgValue::UInt(u)) => ResolvedValue::Num {
                decimal: Decimal::from(*u),
                formatted: None,
            },
            Some(ArgValue::Decimal(d)) => ResolvedValue::Num {
                decimal: d.clone(),
                formatted: None,
            },
            Some(ArgValue::Date(d)) => ResolvedValue::Date {
                date: *d,
                formatted: None,
            },
            None => {
                warnings.push(Warning::UnresolvedVariable(name.to_owned()));
                ResolvedValue::Fallback(format!("{{${name}}}"))
            }
        }
    }

    // ── functions ───────────────────────────────────────────────────────

    fn apply(
        &self,
        func: &Func,
        operand: Option<ResolvedValue>,
        expr: &Expr,
        env: &BTreeMap<String, ResolvedValue>,
        warnings: &mut Vec<Warning>,
    ) -> Result<ResolvedValue, FormatError> {
        // A failed operand stays failed: the warning was already
        // recorded, the fallback text already names the source
        // expression.
        if let Some(ResolvedValue::Fallback(fb)) = operand {
            return Ok(ResolvedValue::Fallback(fb));
        }
        match func.name.as_str() {
            "number" | "integer" => self.fn_number(func, operand, expr, env, warnings),
            "datetime" | "date" => self.fn_datetime(func, operand, expr, env, warnings),
            "string" => match operand {
                Some(value) => Ok(ResolvedValue::Str(self.render(value)?)),
                None => {
                    warnings.push(Warning::BadOperand {
                        function: func.name.clone(),
                        detail: ":string needs an operand".into(),
                    });
                    Ok(ResolvedValue::Fallback(expr.fallback()))
                }
            },
            other => {
                warnings.push(Warning::UnknownFunction(other.to_owned()));
                Ok(ResolvedValue::Fallback(expr.fallback()))
            }
        }
    }

    /// An option value: literal, or a variable rendered *raw* (plain
    /// digits for numbers — a locale-formatted "1 234" would not
    /// parse as a digit count).
    fn opt_value(
        &self,
        func: &str,
        key: &str,
        value: &OptValue,
        env: &BTreeMap<String, ResolvedValue>,
        warnings: &mut Vec<Warning>,
    ) -> Option<String> {
        match value {
            OptValue::Literal(s) => Some(s.clone()),
            OptValue::Var(name) => match self.resolve_var(name, env, warnings) {
                ResolvedValue::Str(s) => Some(s),
                ResolvedValue::Num { decimal, .. } => Some(decimal.to_string()),
                other => {
                    warnings.push(Warning::UnsupportedOption {
                        function: func.to_owned(),
                        option: key.to_owned(),
                        value: format!("${name} ({})", kind_of(&other)),
                    });
                    None
                }
            },
        }
    }

    fn fn_number(
        &self,
        func: &Func,
        operand: Option<ResolvedValue>,
        expr: &Expr,
        env: &BTreeMap<String, ResolvedValue>,
        warnings: &mut Vec<Warning>,
    ) -> Result<ResolvedValue, FormatError> {
        let mut decimal = match operand {
            Some(ResolvedValue::Num { decimal, .. }) => decimal,
            Some(ResolvedValue::Str(s)) => match s.parse::<Decimal>() {
                Ok(d) => d,
                Err(e) => {
                    warnings.push(Warning::BadOperand {
                        function: func.name.clone(),
                        detail: format!("{s:?} as number: {e}"),
                    });
                    return Ok(ResolvedValue::Fallback(expr.fallback()));
                }
            },
            other => {
                warnings.push(Warning::BadOperand {
                    function: func.name.clone(),
                    detail: match other {
                        Some(ResolvedValue::Date { .. }) => "date operand".into(),
                        _ => "missing operand".into(),
                    },
                });
                return Ok(ResolvedValue::Fallback(expr.fallback()));
            }
        };
        let mut grouping = GroupingStrategy::Auto;
        if func.name == "integer" {
            decimal.round(0);
        }
        for (key, value) in &func.options {
            let Some(value) = self.opt_value(&func.name, key, value, env, warnings) else {
                continue;
            };
            match (key.as_str(), value.as_str()) {
                ("maximumFractionDigits", v) => match parse_digits(v) {
                    Some(digits) => decimal.round(-digits),
                    None => warnings.push(bad_option(func, key, v)),
                },
                ("minimumFractionDigits", v) => match parse_digits(v) {
                    Some(digits) => decimal.pad_end(-digits),
                    None => warnings.push(bad_option(func, key, v)),
                },
                ("useGrouping", "never") => grouping = GroupingStrategy::Never,
                ("useGrouping", "auto" | "always") => grouping = GroupingStrategy::Auto,
                ("useGrouping", "min2") => grouping = GroupingStrategy::Min2,
                ("useGrouping", v) => warnings.push(bad_option(func, key, v)),
                ("style", "decimal") => {}
                ("style", v) => warnings.push(bad_option(func, key, v)),
                (k, v) => warnings.push(unknown_option(func, k, v)),
            }
        }
        let formatter =
            DecimalFormatter::try_new(self.locale.into(), DecimalFormatterOptions::from(grouping))
                .map_err(|e| FormatError::Icu(format!("decimal formatter: {e}")))?;
        let formatted = formatter.format(&decimal).to_string();
        Ok(ResolvedValue::Num {
            decimal,
            formatted: Some(formatted),
        })
    }

    fn fn_datetime(
        &self,
        func: &Func,
        operand: Option<ResolvedValue>,
        expr: &Expr,
        env: &BTreeMap<String, ResolvedValue>,
        warnings: &mut Vec<Warning>,
    ) -> Result<ResolvedValue, FormatError> {
        let date = match operand {
            Some(ResolvedValue::Date { date, .. }) => date,
            Some(ResolvedValue::Str(s)) => match parse_iso_date(&s) {
                Ok(d) => d,
                Err(detail) => {
                    warnings.push(Warning::BadOperand {
                        function: func.name.clone(),
                        detail,
                    });
                    return Ok(ResolvedValue::Fallback(expr.fallback()));
                }
            },
            _ => {
                warnings.push(Warning::BadOperand {
                    function: func.name.clone(),
                    detail: format!(":{} needs a date operand", func.name),
                });
                return Ok(ResolvedValue::Fallback(expr.fallback()));
            }
        };
        // `:datetime` takes dateStyle, `:date` takes style (MF2
        // default function registry).
        let style_key = if func.name == "date" {
            "style"
        } else {
            "dateStyle"
        };
        let mut fieldset = YMD::medium();
        for (key, value) in &func.options {
            let Some(value) = self.opt_value(&func.name, key, value, env, warnings) else {
                continue;
            };
            match (key.as_str(), value.as_str()) {
                (k, "full" | "long") if k == style_key => fieldset = YMD::long(),
                (k, "medium") if k == style_key => fieldset = YMD::medium(),
                (k, "short") if k == style_key => fieldset = YMD::short(),
                (k, v) if k == style_key => warnings.push(bad_option(func, k, v)),
                (k, v) => warnings.push(unknown_option(func, k, v)),
            }
        }
        let formatter = DateTimeFormatter::try_new(self.locale.into(), fieldset)
            .map_err(|e| FormatError::Icu(format!("datetime formatter: {e}")))?;
        let formatted = formatter.format(&date).to_string();
        Ok(ResolvedValue::Date {
            date,
            formatted: Some(formatted),
        })
    }

    // ── matcher selection ───────────────────────────────────────────────

    fn select<'v>(
        &self,
        selectors: &[String],
        variants: &'v [Variant],
        fallback_variant: usize,
        env: &BTreeMap<String, ResolvedValue>,
        warnings: &mut Vec<Warning>,
    ) -> Result<&'v Variant, FormatError> {
        let rules = PluralRules::try_new_cardinal(self.locale.into())
            .map_err(|e| FormatError::Icu(format!("plural rules: {e}")))?;
        // Resolve each selector to (Decimal, PluralCategory); a
        // non-numeric or failed selector warns and matches only `*`.
        let mut resolved: Vec<Option<(Decimal, PluralCategory)>> =
            Vec::with_capacity(selectors.len());
        for name in selectors {
            match self.resolve_var(name, env, warnings) {
                ResolvedValue::Num { decimal, .. } => {
                    let category = rules.category_for(&decimal);
                    resolved.push(Some((decimal, category)));
                }
                _ => {
                    warnings.push(Warning::SelectorNotNumeric(name.clone()));
                    resolved.push(None);
                }
            }
        }
        // Score each variant: per key 0 = exact numeric match,
        // 1 = plural category, 2 = `*`; None = no match. Best is the
        // lexicographically smallest score, source order as tie-break
        // (spec: exact match beats category beats catch-all).
        let mut best: Option<(Vec<u8>, &Variant)> = None;
        for variant in variants {
            let mut score = Vec::with_capacity(variant.keys.len());
            let mut matched = true;
            for (key, sel) in variant.keys.iter().zip(&resolved) {
                let key_score = match (key, sel) {
                    (Key::CatchAll, _) => Some(2),
                    (Key::Literal(text), Some((decimal, category))) => {
                        literal_key_score(text, decimal, *category)
                    }
                    (Key::Literal(_), None) => None,
                };
                match key_score {
                    Some(s) => score.push(s),
                    None => {
                        matched = false;
                        break;
                    }
                }
            }
            if matched && best.as_ref().is_none_or(|(b, _)| score < *b) {
                best = Some((score, variant));
            }
        }
        // Compilation guarantees an all-`*` variant, so `best` is
        // always Some; the unwrap_or keeps this total regardless.
        Ok(best
            .map(|(_, variant)| variant)
            .unwrap_or(&variants[fallback_variant]))
    }

    // ── rendering ───────────────────────────────────────────────────────

    /// Final rendering of a resolved value into output text.
    fn render(&self, value: ResolvedValue) -> Result<String, FormatError> {
        match value {
            ResolvedValue::Str(s) | ResolvedValue::Fallback(s) => Ok(s),
            ResolvedValue::Num {
                formatted: Some(s), ..
            }
            | ResolvedValue::Date {
                formatted: Some(s), ..
            } => Ok(s),
            // Unannotated number: locale defaults.
            ResolvedValue::Num { decimal, .. } => {
                let formatter = DecimalFormatter::try_new(
                    self.locale.into(),
                    DecimalFormatterOptions::default(),
                )
                .map_err(|e| FormatError::Icu(format!("decimal formatter: {e}")))?;
                Ok(formatter.format(&decimal).to_string())
            }
            // Unannotated date: medium date style.
            ResolvedValue::Date { date, .. } => {
                let formatter = DateTimeFormatter::try_new(self.locale.into(), YMD::medium())
                    .map_err(|e| FormatError::Icu(format!("datetime formatter: {e}")))?;
                Ok(formatter.format(&date).to_string())
            }
        }
    }
}

// ── free helpers ────────────────────────────────────────────────────────

/// Exact numeric key first (spec: exact match beats plural category),
/// then the category names.
fn literal_key_score(text: &str, decimal: &Decimal, category: PluralCategory) -> Option<u8> {
    if let Ok(key_num) = text.parse::<Decimal>()
        && plain_digits(&key_num) == plain_digits(decimal)
    {
        return Some(0);
    }
    let matches_category = matches!(
        (text, category),
        ("zero", PluralCategory::Zero)
            | ("one", PluralCategory::One)
            | ("two", PluralCategory::Two)
            | ("few", PluralCategory::Few)
            | ("many", PluralCategory::Many)
            | ("other", PluralCategory::Other)
    );
    matches_category.then_some(1)
}

/// Canonical digit string for exact-match comparison: strips trailing
/// fraction zeros and leading integer zeros, so `1.0` == `1` == `01`
/// (Q8: pin exact-match semantics on the resolved value).
fn plain_digits(d: &Decimal) -> String {
    let mut d = d.clone();
    d.absolute.trim_end();
    d.absolute.trim_start();
    format!("{:?}{}", d.sign, d.absolute)
}

fn parse_digits(value: &str) -> Option<i16> {
    value.parse::<i16>().ok().filter(|d| (0..=20).contains(d))
}

fn parse_iso_date(s: &str) -> Result<Date<Iso>, String> {
    let mut parts = s.splitn(3, '-');
    let (Some(y), Some(m), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!("{s:?} is not YYYY-MM-DD"));
    };
    let (Ok(y), Ok(m), Ok(d)) = (y.parse(), m.parse(), d.parse()) else {
        return Err(format!("{s:?} is not YYYY-MM-DD"));
    };
    Date::try_new_iso(y, m, d).map_err(|e| format!("{s:?}: {e:?}"))
}

fn bad_option(func: &Func, option: &str, value: &str) -> Warning {
    Warning::UnsupportedOption {
        function: func.name.clone(),
        option: option.to_owned(),
        value: value.to_owned(),
    }
}

fn unknown_option(func: &Func, option: &str, _value: &str) -> Warning {
    Warning::UnknownOption {
        function: func.name.clone(),
        option: option.to_owned(),
    }
}

fn kind_of(value: &ResolvedValue) -> &'static str {
    match value {
        ResolvedValue::Str(_) => "string",
        ResolvedValue::Num { .. } => "number",
        ResolvedValue::Date { .. } => "date",
        ResolvedValue::Fallback(_) => "unresolved",
    }
}
