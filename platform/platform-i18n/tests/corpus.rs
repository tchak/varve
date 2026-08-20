//! The Q8 spike corpus, ported to the compiled-template API, plus the
//! productionization tests: compile/format split, declarations
//! evaluated once, warn-and-continue fallbacks, NFC name lookup,
//! exact-match canonicalization, and the catalog fallback chain.
//!
//! French assertions are byte-exact on purpose (Q8: treat formatted
//! output as opaque): CLDR French groups digits with U+202F NARROW
//! NO-BREAK SPACE, not U+0020 — a naive "1 000 000" snapshot is
//! wrong, and these literals document the real bytes.

use platform_i18n::{
    ArgValue, Args, Catalog, Catalogs, CompileError, FormatError, Formatted, MessageTemplate,
    Warning, locale,
};

fn args(pairs: &[(&str, ArgValue)]) -> Args {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn fmt(source: &str, loc: &str, a: &Args) -> Formatted {
    MessageTemplate::compile(source)
        .expect("message compiles")
        .format(&locale(loc).expect("valid locale"), a)
        .expect("message formats")
}

/// Format and require a warning-free result.
fn clean(source: &str, loc: &str, a: &Args) -> String {
    let out = fmt(source, loc, a);
    assert!(
        out.warnings.is_empty(),
        "unexpected warnings: {:?}",
        out.warnings
    );
    out.text
}

// ── interpolation (spike tests 1–2) ─────────────────────────────────────────

#[test]
fn hello_en() {
    let out = clean(
        "Hello, {$name}!",
        "en",
        &args(&[("name", ArgValue::from("Ada"))]),
    );
    assert_eq!(out, "Hello, Ada!");
}

#[test]
fn hello_fr() {
    let out = clean(
        "Bonjour, {$name} !",
        "fr",
        &args(&[("name", ArgValue::from("Ada"))]),
    );
    assert_eq!(out, "Bonjour, Ada !");
}

// ── :number (spike tests 3–6) ───────────────────────────────────────────────

#[test]
fn number_en_grouping() {
    let out = clean(
        "{$count :number} items",
        "en",
        &args(&[("count", ArgValue::from(1_234_567))]),
    );
    assert_eq!(out, "1,234,567 items");
}

#[test]
fn number_fr_grouping() {
    let out = clean(
        "{$count :number} items",
        "fr",
        &args(&[("count", ArgValue::from(1_234_567))]),
    );
    // Byte-exact: CLDR French groups with U+202F NARROW NO-BREAK
    // SPACE (Q8's "NNBSP will break naive snapshots").
    assert_eq!(out, "1\u{202f}234\u{202f}567 items");
}

#[test]
fn number_min_fraction_digits_fr() {
    let out = clean(
        "{$price :number minimumFractionDigits=2}",
        "fr",
        &args(&[("price", ArgValue::from(3.5))]),
    );
    // Byte-exact: the French decimal separator is a comma.
    assert_eq!(out, "3,50");
}

#[test]
fn number_max_fraction_digits_en() {
    let out = clean(
        "{$x :number maximumFractionDigits=1}",
        "en",
        &args(&[("x", ArgValue::from(2.345))]),
    );
    assert_eq!(out, "2.3");
}

// ── .match plural selection (spike tests 7–12) ──────────────────────────────

const PLURAL_EN: &str = "\
.input {$count :number}
.match $count
one {{You have {$count} item}}
many {{You have {$count} items (many)}}
* {{You have {$count} items}}";

#[test]
fn plural_en_one() {
    let out = clean(PLURAL_EN, "en", &args(&[("count", ArgValue::from(1))]));
    assert_eq!(out, "You have 1 item");
}

#[test]
fn plural_en_other() {
    let out = clean(PLURAL_EN, "en", &args(&[("count", ArgValue::from(5))]));
    assert_eq!(out, "You have 5 items");
}

const PLURAL_FR: &str = "\
.input {$count :number}
.match $count
one {{Vous avez {$count} élément}}
many {{Vous avez {$count} éléments (many)}}
* {{Vous avez {$count} éléments}}";

#[test]
fn plural_fr_one() {
    let out = clean(PLURAL_FR, "fr", &args(&[("count", ArgValue::from(1))]));
    assert_eq!(out, "Vous avez 1 élément");
}

#[test]
fn plural_fr_million_is_many() {
    // Modern CLDR: French has a `many` cardinal category for the
    // millions (1e6). Byte-exact U+202F separators again.
    let out = clean(
        PLURAL_FR,
        "fr",
        &args(&[("count", ArgValue::from(1_000_000))]),
    );
    assert_eq!(out, "Vous avez 1\u{202f}000\u{202f}000 éléments (many)");
}

#[test]
fn plural_exact_match_beats_category() {
    let msg = "\
.input {$count :number}
.match $count
1 {{exactly one}}
one {{category one}}
* {{other}}";
    let out = clean(msg, "en", &args(&[("count", ArgValue::from(1))]));
    assert_eq!(out, "exactly one");
}

#[test]
fn local_declaration() {
    let msg = "\
.local $price = {$amount :number minimumFractionDigits=2}
{{Total: {$price}}}";
    let out = clean(msg, "en", &args(&[("amount", ArgValue::from(7))]));
    assert_eq!(out, "Total: 7.00");
}

// ── :datetime / :date (spike tests 13–14) ───────────────────────────────────

#[test]
fn datetime_long_en_fr() {
    let arguments = args(&[("d", ArgValue::date(2026, 8, 20).unwrap())]);
    let en = clean("{$d :datetime dateStyle=long}", "en", &arguments);
    let fr = clean("{$d :datetime dateStyle=long}", "fr", &arguments);
    assert_eq!(en, "August 20, 2026");
    // Byte-exact: lowercase month, U+0020 separators (French long
    // dates use plain spaces; only digit grouping uses U+202F).
    assert_eq!(fr, "20 août 2026");
}

#[test]
fn date_style_from_string_operand() {
    let arguments = args(&[("d", ArgValue::from("2026-08-20"))]);
    let en = clean("{$d :date style=medium}", "en", &arguments);
    assert!(!en.is_empty());
}

// ── failure modes (spike tests 15–18, adapted to the Q8 split) ──────────────

#[test]
fn malformed_message_is_compile_error() {
    // The spike returned Err at format time; the split moves this to
    // compile time, carrying the parser diagnostics.
    let err = MessageTemplate::compile("Hello, {$name").unwrap_err();
    match err {
        CompileError::Syntax(diags) => assert!(!diags.is_empty()),
        other => panic!("expected syntax error, got {other:?}"),
    }
}

#[test]
fn missing_argument_falls_back_with_warning() {
    // Spec fallback behavior (Q8): the format succeeds, the output
    // carries the fallback representation, and a warning records it.
    let out = fmt("Hello, {$name}!", "en", &Args::new());
    assert_eq!(out.text, "Hello, {$name}!");
    assert_eq!(
        out.warnings,
        vec![Warning::UnresolvedVariable("name".into())]
    );
}

#[test]
fn unknown_function_falls_back_with_warning() {
    let out = fmt("{$x :frobnicate}", "en", &args(&[("x", ArgValue::from(1))]));
    // Fallback of an expression with a variable operand is `{$var}`.
    assert_eq!(out.text, "{$x}");
    assert_eq!(
        out.warnings,
        vec![Warning::UnknownFunction("frobnicate".into())]
    );
}

#[test]
fn bad_locale_is_err() {
    assert!(locale("not a locale!!").is_err());
    assert!(locale("fr-CH").is_ok());
}

// ── productionization: compile once, format many ────────────────────────────

#[test]
fn compile_once_format_many_is_deterministic() {
    let template = MessageTemplate::compile(PLURAL_FR).unwrap();
    let fr = locale("fr").unwrap();
    let one = args(&[("count", ArgValue::from(1))]);
    let first = template.format(&fr, &one).unwrap();
    let second = template.format(&fr, &one).unwrap();
    assert_eq!(first, second);
    // No state leaks between calls: different args, then the
    // original args again.
    let million = args(&[("count", ArgValue::from(1_000_000))]);
    let big = template.format(&fr, &million).unwrap();
    assert_eq!(
        big.text,
        "Vous avez 1\u{202f}000\u{202f}000 éléments (many)"
    );
    assert_eq!(template.format(&fr, &one).unwrap(), first);
}

#[test]
fn declarations_evaluate_once_in_order() {
    // `.local $twice = {$n}` references the `.input`-declared $n and
    // must see the *resolved* value — annotated formatting included —
    // not re-evaluate the raw argument.
    let msg = "\
.input {$n :number minimumFractionDigits=2}
.local $twice = {$n}
{{{$n} and {$twice}}}";
    let out = clean(msg, "en", &args(&[("n", ArgValue::from(7))]));
    assert_eq!(out, "7.00 and 7.00");
}

#[test]
fn input_self_reference_reads_the_argument() {
    // `.input {$count :number}` mentions $count inside its own
    // expression; declaration-order evaluation resolves the inner
    // reference from the caller's arguments, no recursion.
    let out = clean(
        ".input {$count :number}\n{{{$count}}}",
        "en",
        &args(&[("count", ArgValue::from(1234))]),
    );
    assert_eq!(out, "1,234");
}

// ── productionization: warn-and-continue ────────────────────────────────────

#[test]
fn unknown_option_warns_and_continues() {
    // Per spec (Q8): an unknown option is ignored, the value still
    // formats, and the format does not fail.
    let out = fmt(
        "{$n :number signDisplay=always}",
        "en",
        &args(&[("n", ArgValue::from(5))]),
    );
    assert_eq!(out.text, "5");
    assert_eq!(
        out.warnings,
        vec![Warning::UnknownOption {
            function: "number".into(),
            option: "signDisplay".into(),
        }]
    );
}

#[test]
fn operandless_unknown_function_falls_back() {
    let out = fmt("before {:frobnicate} after", "en", &Args::new());
    // Fallback of a bare function expression is `{:func}`.
    assert_eq!(out.text, "before {:frobnicate} after");
    assert_eq!(
        out.warnings,
        vec![Warning::UnknownFunction("frobnicate".into())]
    );
}

#[test]
fn missing_argument_under_annotation_falls_back_once() {
    // The annotation does not double-report: the unresolved operand
    // warns once and its fallback propagates through `:number`.
    let out = fmt("{$count :number} items", "en", &Args::new());
    assert_eq!(out.text, "{$count} items");
    assert_eq!(
        out.warnings,
        vec![Warning::UnresolvedVariable("count".into())]
    );
}

#[test]
fn bad_operand_falls_back_with_warning() {
    let out = fmt(
        "{$x :number}",
        "en",
        &args(&[("x", ArgValue::from("not a number"))]),
    );
    assert_eq!(out.text, "{$x}");
    assert!(matches!(
        out.warnings.as_slice(),
        [Warning::BadOperand { function, .. }] if function == "number"
    ));
}

// ── productionization: NFC and exact-match canonicalization ─────────────────

#[test]
fn nfc_name_lookup() {
    // Message uses composed U+00E9; the caller's argument key is
    // decomposed (e + U+0301 COMBINING ACUTE ACCENT). MF2 compares
    // names after NFC normalization (Q8), so they must meet.
    let composed_msg = "Au {$caf\u{e9}} !";
    let decomposed_key = "cafe\u{301}";
    let out = clean(
        composed_msg,
        "fr",
        &args(&[(decomposed_key, ArgValue::from("Procope"))]),
    );
    assert_eq!(out, "Au Procope !");

    // And the mirror: decomposed in the message source, composed in
    // the argument map.
    let decomposed_msg = "Au {$cafe\u{301}} !";
    let out = clean(
        decomposed_msg,
        "fr",
        &args(&[("caf\u{e9}", ArgValue::from("Procope"))]),
    );
    assert_eq!(out, "Au Procope !");
}

#[test]
fn exact_match_canonicalizes_digits() {
    // Key `1` vs argument 1.0: exact-match comparison runs on the
    // canonicalized digit string of the resolved value (trailing
    // fraction zeros trimmed — the spike's Decimal-trim semantics,
    // kept per Q8), so 1.0 hits the `1` variant, not `one`/`*`.
    let msg = "\
.input {$count :number}
.match $count
1 {{exactly one}}
one {{category one}}
* {{other}}";
    let out = clean(msg, "en", &args(&[("count", ArgValue::from(1.0))]));
    assert_eq!(out, "exactly one");
}

// ── compile-time data model errors ──────────────────────────────────────────

#[test]
fn matcher_without_fallback_variant_is_compile_error() {
    let msg = "\
.input {$count :number}
.match $count
one {{one}}
other {{other}}";
    assert_eq!(
        MessageTemplate::compile(msg).unwrap_err(),
        CompileError::MissingFallbackVariant
    );
}

#[test]
fn duplicate_declaration_is_compile_error() {
    let msg = "\
.input {$x :number}
.local $x = {1}
{{{$x}}}";
    assert_eq!(
        MessageTemplate::compile(msg).unwrap_err(),
        CompileError::DuplicateDeclaration("x".into())
    );
}

// ── catalogs ────────────────────────────────────────────────────────────────

#[test]
fn catalog_fallback_chain_hit_and_miss() {
    let fr = Catalog::from_pairs([("greeting", "Bonjour, {$name} !")]).unwrap();
    let en = Catalog::from_pairs([("greeting", "Hello, {$name}!"), ("only-en", "English only")])
        .unwrap();
    let mut catalogs = Catalogs::new(vec![locale("fr").unwrap(), locale("en").unwrap()]);
    catalogs.insert(locale("fr").unwrap(), fr);
    catalogs.insert(locale("en").unwrap(), en);

    let fr_ch = locale("fr-CH").unwrap();
    let ada = args(&[("name", ArgValue::from("Ada"))]);
    // fr-CH has no catalog: the chain walks fr-CH → fr → en, and the
    // fr text wins.
    let out = catalogs.format(&fr_ch, "greeting", &ada).unwrap();
    assert_eq!(out.text, "Bonjour, Ada !");
    // Missing in fr, present in en: falls through the whole chain.
    let out = catalogs.format(&fr_ch, "only-en", &Args::new()).unwrap();
    assert_eq!(out.text, "English only");
    // Missing everywhere: an Err carrying the id.
    assert_eq!(
        catalogs.format(&fr_ch, "nope", &Args::new()).unwrap_err(),
        FormatError::UnknownMessage { id: "nope".into() }
    );
}

#[test]
fn catalog_reports_all_compile_errors_with_ids() {
    let err = Catalog::from_pairs([
        ("ok", "Hello"),
        ("bad-unclosed", "Hello, {$name"),
        (
            "bad-no-fallback",
            ".input {$n :number}\n.match $n\none {{x}}",
        ),
    ])
    .unwrap_err();
    // Every failing message reports, with its id, not just the first.
    let ids: Vec<&str> = err.errors.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["bad-unclosed", "bad-no-fallback"]);
    assert!(matches!(err.errors[0].1, CompileError::Syntax(_)));
    assert_eq!(err.errors[1].1, CompileError::MissingFallbackVariant);
}
