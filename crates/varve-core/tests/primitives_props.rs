//! Property layer for the scalar primitives (§7, §2.13 decision 3):
//! one value, one text. `Display` is the canonical rendering the wire
//! and every commitment carry, so `parse ∘ Display` must be the
//! identity, ordering must be the exact rational order, and instants
//! must stay inside the four-digit year range whatever offset they
//! were spelled with.

use std::cmp::Ordering;

use proptest::prelude::*;
use varve_core::primitives::{Date, Decimal, Instant, MAX_YEAR};

// ---------------------------------------------------------------------
// Decimal

/// A decimal built from an i64 mantissa and a scale ≤ 12: `mantissa ×
/// 10⁻ˢᶜᵃˡᵉ`, formatted then parsed (so it is normalized).
fn decimal() -> impl Strategy<Value = (Decimal, i128, u32)> {
    (any::<i64>(), 0u32..=12).prop_map(|(mantissa, scale)| {
        let digits = mantissa.unsigned_abs().to_string();
        let sign = if mantissa < 0 { "-" } else { "" };
        let text = if scale == 0 {
            format!("{sign}{digits}")
        } else {
            let padded = format!("{digits:0>width$}", width = scale as usize + 1);
            let (int, frac) = padded.split_at(padded.len() - scale as usize);
            format!("{sign}{int}.{frac}")
        };
        (Decimal::parse(&text).unwrap(), i128::from(mantissa), scale)
    })
}

/// Exact rational comparison of `a × 10⁻ˢᵃ` and `b × 10⁻ˢᵇ`: cross-
/// multiply on i128 (|mantissa| < 2⁶³, scale ≤ 12 → < 2¹⁰³, fits).
fn rational_cmp(a: i128, sa: u32, b: i128, sb: u32) -> Ordering {
    (a * 10i128.pow(sb)).cmp(&(b * 10i128.pow(sa)))
}

proptest! {
    /// `parse ∘ Display` is the identity, and `Display` is normalized:
    /// no trailing fraction zeros, no `-0`, no leading zeros.
    #[test]
    fn decimal_display_round_trips_and_is_normalized((d, mantissa, _) in decimal()) {
        let text = d.to_string();
        prop_assert_eq!(Decimal::parse(&text).unwrap(), d.clone());
        prop_assert_eq!(&Decimal::parse(&text).unwrap().to_string(), &text);
        if let Some((_, frac)) = text.split_once('.') {
            prop_assert!(!frac.ends_with('0'), "trailing fraction zero in {text}");
            prop_assert!(!frac.is_empty(), "bare dot in {text}");
        }
        prop_assert_ne!(text.as_str(), "-0");
        prop_assert!(!text.starts_with("-0") || text.starts_with("-0."), "{text}");
        let unsigned = text.trim_start_matches('-');
        prop_assert!(!unsigned.starts_with('0') || unsigned == "0" || unsigned.starts_with("0."), "leading zero in {text}");
        if mantissa == 0 {
            prop_assert_eq!(text.as_str(), "0");
        }
    }

    /// `Ord` is the exact rational order.
    #[test]
    fn decimal_order_is_the_rational_order(
        (a, ma, sa) in decimal(), (b, mb, sb) in decimal(),
    ) {
        prop_assert_eq!(a.cmp(&b), rational_cmp(ma, sa, mb, sb));
        prop_assert_eq!(a.partial_cmp(&b), Some(a.cmp(&b)));
        prop_assert_eq!(a == b, rational_cmp(ma, sa, mb, sb) == Ordering::Equal);
    }

    /// `to_i64` is exact-or-nothing: `Some` iff the value is a whole
    /// number in i64 range, and then it is exactly that number.
    #[test]
    fn decimal_to_i64_is_exact_or_nothing((d, mantissa, scale) in decimal()) {
        let whole = mantissa % 10i128.pow(scale) == 0;
        match d.to_i64() {
            Some(i) => {
                prop_assert!(whole);
                prop_assert_eq!(Decimal::from_i64(i), d);
                prop_assert_eq!(i128::from(i) * 10i128.pow(scale), mantissa);
            }
            None => prop_assert!(!whole),
        }
    }

    /// `× 1 ⁄ 1` is the identity; `× n ⁄ 1` then `× 1 ⁄ n` comes back
    /// exactly whenever both steps are representable (§2.14
    /// exact-or-nothing: no rounding to lose on the way).
    #[test]
    fn mul_div_exact_identity_and_round_trip((d, _, _) in decimal(), n in 1u64..=1000) {
        prop_assert_eq!(d.mul_div_exact(1, 1), Some(d.clone()));
        if let Some(scaled) = d.mul_div_exact(n, 1)
            && let Some(back) = scaled.mul_div_exact(1, n)
        {
            prop_assert_eq!(back, d);
        }
    }
}

#[test]
fn decimal_parse_accepts_what_people_type_and_normalizes() {
    // The checked text→decimal cast (§3) is deliberately liberal on
    // input and strict on output.
    for (input, rendered) in [
        (".5", "0.5"),
        ("5.", "5"),
        ("007", "7"),
        ("-0", "0"),
        ("-0.0", "0"),
        ("1.50", "1.5"),
        ("00.10", "0.1"),
        ("-007.700", "-7.7"),
    ] {
        let d = Decimal::parse(input).unwrap();
        assert_eq!(d.to_string(), rendered, "{input}");
        assert_eq!(Decimal::parse(rendered).unwrap(), d);
    }
    for refused in ["", "-", ".", "-.", "1,5", "+1", "1e3", " 1", "1 ", "1.2.3", "0x10"] {
        assert!(Decimal::parse(refused).is_err(), "{refused:?} should be refused");
    }
}

// ---------------------------------------------------------------------
// Date

fn days_in_month(year: i64, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap { 29 } else { 28 }
        }
    }
}

/// A valid calendar date in the shared `Date`/`Instant` range
/// (`0000-01-01` through `9998-12-31`, `MAX_YEAR`), as components.
fn civil() -> impl Strategy<Value = (i64, u8, u8)> {
    (0i64..=i64::from(MAX_YEAR), 1u8..=12, 1u8..=31).prop_map(|(y, m, d)| (y, m, d.min(days_in_month(y, m))))
}

fn date_text(y: i64, m: u8, d: u8) -> String {
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 of a proleptic Gregorian civil date
/// (Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: u8, d: u8) -> i64 {
    let (m, d) = (i64::from(m), i64::from(d));
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

proptest! {
    /// `parse ∘ Display` is the identity on every valid date, and the
    /// text is exactly `YYYY-MM-DD`.
    #[test]
    fn date_display_round_trips((y, m, d) in civil()) {
        let text = date_text(y, m, d);
        let date = Date::parse(&text).unwrap();
        prop_assert_eq!(&date.to_string(), &text);
        prop_assert_eq!(Date::parse(&date.to_string()).unwrap(), date);
    }

    /// The date→datetime embedding is injective and its left inverse
    /// is the datetime→date narrowing (§3): `utc_date ∘
    /// at_midnight_utc = id`, and `utc_date` is the first ten
    /// characters of the instant's rendering.
    #[test]
    fn date_embeds_at_midnight_and_narrows_back((y, m, d) in civil()) {
        let date = Date::parse(&date_text(y, m, d)).unwrap();
        let instant = date.at_midnight_utc();
        prop_assert_eq!(instant.utc_date(), date);
        prop_assert_eq!(&instant.to_string()[..10], &date.to_string());
        prop_assert_eq!(&instant.to_string()[10..], "T00:00:00Z");
    }

    /// Only the ten-character strict form is a date: any other length
    /// is refused before jiff sees it.
    #[test]
    fn date_refuses_every_other_length(s in "[0-9-]{0,9}|[0-9-]{11,14}") {
        prop_assert!(Date::parse(&s).is_err());
    }
}

#[test]
fn date_refuses_unpadded_and_out_of_range_forms() {
    for refused in [
        "2024-2-29", "2024-02-9", "24-02-29", "2024/02/29", "2024-02-30", "2024-00-10",
        "2024-13-01", "10000-01-01", "-001-01-01", "2024-02-29T", "2024-02-29 ",
        // Past MAX_YEAR: a valid calendar date the shared range leaves out.
        "9999-01-01", "9999-12-31",
    ] {
        assert!(Date::parse(refused).is_err(), "{refused:?} should be refused");
    }
    assert!(Date::parse("0000-01-01").is_ok());
    assert!(Date::parse("9998-12-31").is_ok());
    assert!(Date::parse("2000-02-29").is_ok());
    assert!(Date::parse("1900-02-29").is_err());
}

// ---------------------------------------------------------------------
// Instant

/// A strictly-spelled RFC 3339 instant with valid civil components,
/// optional fraction, and either `Z` or a `±HH:MM` offset — plus what
/// its UTC-normalized time is, in seconds from the 1970 epoch.
#[derive(Debug, Clone)]
struct Spelled {
    text: String,
    utc_seconds: i64,
    offset_minutes: i64,
}

fn spelled() -> impl Strategy<Value = Spelled> {
    (
        civil(),
        0u8..24,
        0u8..60,
        0u8..60,
        proptest::option::of("[0-9]{1,9}"),
        proptest::option::of((any::<bool>(), 0i64..24, 0i64..60)),
    )
        .prop_map(|((y, m, d), hh, mm, ss, fraction, offset)| {
            let mut text = format!("{}T{hh:02}:{mm:02}:{ss:02}", date_text(y, m, d));
            if let Some(f) = &fraction {
                text.push('.');
                text.push_str(f);
            }
            let offset_minutes = match offset {
                None => {
                    text.push('Z');
                    0
                }
                Some((negative, oh, om)) => {
                    text.push(if negative { '-' } else { '+' });
                    text.push_str(&format!("{oh:02}:{om:02}"));
                    let minutes = oh * 60 + om;
                    if negative { -minutes } else { minutes }
                }
            };
            let local = days_from_civil(y, m, d) * 86_400
                + i64::from(hh) * 3_600
                + i64::from(mm) * 60
                + i64::from(ss);
            Spelled { text, utc_seconds: local - offset_minutes * 60, offset_minutes }
        })
}

/// `0000-01-01T00:00:00Z` in epoch seconds: the bottom of the range.
const YEAR_0000: i64 = -62_167_219_200;
/// `9998-12-31T23:59:59Z` in epoch seconds: the last whole second of
/// the shared range (`MAX_YEAR`).
const LAST_SECOND: i64 = 253_370_764_799;

proptest! {
    /// A strictly-spelled instant parses iff its UTC normalization
    /// stays in the shared `Date` range (§2.13: the canonical form has
    /// no rendering outside four-digit years; `MAX_YEAR` keeps the
    /// two casts total); when it does, `Display` is the normalized
    /// `…Z` form with a four-digit year and re-parses to an equal
    /// value.
    #[test]
    fn instant_parses_iff_utc_year_has_four_digits(s in spelled()) {
        let in_range = (YEAR_0000..=LAST_SECOND).contains(&s.utc_seconds);
        match Instant::parse(&s.text) {
            Ok(instant) => {
                prop_assert!(in_range, "{} normalizes outside 0000–9998", s.text);
                let rendered = instant.to_string();
                prop_assert!(rendered.ends_with('Z'), "{rendered}");
                prop_assert!(rendered.as_bytes()[..4].iter().all(u8::is_ascii_digit), "{rendered}");
                prop_assert_eq!(rendered.as_bytes()[10], b'T');
                prop_assert_eq!(Instant::parse(&rendered).unwrap(), instant);
                prop_assert_eq!(&Instant::parse(&rendered).unwrap().to_string(), &rendered);
                // The rendered date is the UTC date, and `utc_date`
                // agrees with it.
                prop_assert_eq!(&instant.utc_date().to_string(), &rendered[..10]);
                let utc_days = s.utc_seconds.div_euclid(86_400);
                let (y, m, d) = {
                    // Inverse of days_from_civil, checked against Date.
                    let date = instant.utc_date();
                    let t = date.to_string();
                    (t[..4].parse::<i64>().unwrap(), t[5..7].parse::<u8>().unwrap(), t[8..].parse::<u8>().unwrap())
                };
                prop_assert_eq!(days_from_civil(y, m, d), utc_days);
            }
            Err(_) => prop_assert!(!in_range, "{} refused although in range", s.text),
        }
    }

    /// Two spellings of one instant are one value: `Z` and `+00:00`,
    /// and any offset shifted onto the clock time (§2.13 — equality is
    /// semantic; the offset is not retained).
    #[test]
    fn instant_equality_is_semantic_across_offsets(s in spelled(), shift in -1439i64..=1439) {
        let Ok(instant) = Instant::parse(&s.text) else { return Ok(()) };
        // Same clock reading, `+00:00` for `Z`.
        if s.text.ends_with('Z') {
            let plus_zero = format!("{}+00:00", &s.text[..s.text.len() - 1]);
            prop_assert_eq!(Instant::parse(&plus_zero).unwrap(), instant);
        }
        // Re-spell in another offset: local = utc + shift.
        let local = s.utc_seconds + shift * 60;
        let days = local.div_euclid(86_400);
        let secs = local.rem_euclid(86_400);
        // civil_from_days (Hinnant).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        if !(0..=9999).contains(&y) {
            return Ok(()); // the re-spelling would need a five-digit local year
        }
        let fraction = s.text[19..].split(['Z', '+', '-']).next().unwrap_or("");
        let sign = if shift < 0 { '-' } else { '+' };
        let respelled = format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}{fraction}{sign}{:02}:{:02}",
            secs / 3600, (secs / 60) % 60, secs % 60, shift.abs() / 60, shift.abs() % 60,
        );
        prop_assert_eq!(Instant::parse(&respelled).unwrap(), instant, "{}", respelled);
        prop_assert_eq!(Instant::parse(&respelled).unwrap().to_string(), instant.to_string());
        let _ = s.offset_minutes;
    }
}

#[test]
fn instant_year_range_edges() {
    // Refused: an offset that carries the UTC date out of the shared
    // range (`MAX_YEAR`), and anything in 9999 or beyond — including
    // instants the backing type could represent: the range is a
    // contract, not the backing type's limit.
    for refused in [
        "0000-01-01T00:00:00+01:00",
        "0000-01-01T00:00:00+00:01",
        "9998-12-31T23:30:00-01:00",
        "9998-12-31T23:59:59-00:01",
        "9999-01-01T00:00:00Z",
        "9999-06-15T12:00:00Z",
        "9999-12-31T23:59:59Z",
        "10000-01-01T00:00:00Z",
    ] {
        assert!(Instant::parse(refused).is_err(), "{refused:?} should be refused");
    }
    // Accepted: the very edges, with and without a fraction.
    for (accepted, rendered) in [
        ("0000-01-01T00:00:00Z", "0000-01-01T00:00:00Z"),
        ("0000-01-01T01:00:00+01:00", "0000-01-01T00:00:00Z"),
        ("9998-12-31T23:59:59Z", "9998-12-31T23:59:59Z"),
        ("9999-01-01T00:59:59+01:00", "9998-12-31T23:59:59Z"),
        ("9998-12-31T23:59:59.999999999Z", "9998-12-31T23:59:59.999999999Z"),
    ] {
        let instant = Instant::parse(accepted).unwrap_or_else(|e| panic!("{accepted}: {e}"));
        assert_eq!(instant.to_string(), rendered, "{accepted}");
        assert_eq!(instant.utc_date().to_string(), &rendered[..10]);
    }
}
