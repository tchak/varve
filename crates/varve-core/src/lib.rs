//! Tier 0 (§7 of DESIGN.md): identifiers, row paths, scalar primitives,
//! canonical serialization and content hashing. Depends on nothing.
//!
//! Deterministic by construction: no IO, no clock, no async.

#![forbid(unsafe_code)]

use std::fmt;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(
    /// Stable identity of a typed field (§2.1). Cells are addressed by
    /// `(column_id, row_path)`; stability across revisions is what makes
    /// cells revision-agnostic (§3).
    ColumnId
);
id_type!(
    /// Identity of an ordered container of columns (§2.1).
    GroupId
);
id_type!(
    /// Identity of a record — a long-lived case file (§2.9).
    RecordId
);
id_type!(
    /// Identity of one instance of a `many` group (§2.1).
    ItemId
);
id_type!(
    /// Content-address of an immutable published schema version (§2.1).
    RevisionId
);
id_type!(
    /// Identity of a published, reusable group definition (§2.1).
    BlockId
);
id_type!(
    /// Identity of a published nomenclature (§2.12). Inline nomenclatures
    /// have no identity — they version with their containing revision.
    NomenclatureId
);
id_type!(
    /// Identity of a resolver declaration (§2.7).
    ResolverId
);
id_type!(
    /// Identity of an enum option within a nomenclature (§2.11). Cells
    /// store option ids; labels live in the revision.
    OptionId
);
id_type!(
    /// Identity of a surface (§2.1): a presentation + admissibility
    /// tree over a revision.
    SurfaceId
);

/// One segment of a row path: which item of which `many` group.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PathSeg {
    pub group: GroupId,
    pub item: ItemId,
}

/// A possibly-empty sequence of segments (§2.3).
///
/// Storage and addressing work at depth N; `depth <= 1` is a schema
/// validation *policy* (`varve-schema`), deliberately never encoded in
/// this type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct RowPath(Vec<PathSeg>);

impl RowPath {
    /// The empty path: root scope.
    pub fn root() -> Self {
        Self::default()
    }

    pub fn depth(&self) -> usize {
        self.0.len()
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn child(&self, seg: PathSeg) -> Self {
        let mut segs = self.0.clone();
        segs.push(seg);
        Self(segs)
    }

    pub fn segments(&self) -> &[PathSeg] {
        &self.0
    }

    pub fn starts_with(&self, prefix: &RowPath) -> bool {
        self.0.len() >= prefix.0.len() && self.0[..prefix.0.len()] == prefix.0
    }
}

pub mod primitives {
    //! Scalar primitives (§7): exact decimal, dates, RFC 3339 instants.
    //! No floats anywhere — §5: strings for exact decimals, RFC 3339 for
    //! instants.

    use std::fmt;

    /// Exact decimal: integer mantissa and scale, normalized (no trailing
    /// fraction zeros), so `1.50` and `1.5` are the same value.
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub struct Decimal {
        mantissa: i128,
        scale: u32,
    }

    #[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
    pub enum PrimitiveError {
        #[error("{0}")]
        Malformed(&'static str),
        #[error("out of range: {0}")]
        OutOfRange(&'static str),
    }

    impl Decimal {
        pub fn from_i64(value: i64) -> Self {
            Self {
                mantissa: i128::from(value),
                scale: 0,
            }
        }

        /// Exact-or-nothing: `Some` only for whole numbers fitting i64 —
        /// no silent truncation (§3 checked cast).
        pub fn to_i64(&self) -> Option<i64> {
            if self.scale != 0 {
                return None;
            }
            i64::try_from(self.mantissa).ok()
        }

        /// Magnitude as (integer digits, fraction digits), normalized.
        fn magnitude(&self) -> (String, String) {
            let digits = self.mantissa.unsigned_abs().to_string();
            let scale = self.scale as usize;
            if digits.len() > scale {
                let (int, frac) = digits.split_at(digits.len() - scale);
                (int.to_string(), frac.to_string())
            } else {
                ("0".to_string(), format!("{digits:0>scale$}"))
            }
        }

        /// Exact `self × num ⁄ den`, or `None` when the result has no
        /// finite decimal representation (or overflows). The §2.14 unit
        /// conversion workhorse: exact-or-nothing, never rounds.
        pub fn mul_div_exact(&self, num: u64, den: u64) -> Option<Decimal> {
            if den == 0 {
                return None;
            }
            // Reduce before multiplying: cancel `den` against `num` and
            // against the mantissa first, so a result that fits is
            // never lost to an intermediate overflow.
            let mut num = u128::from(num);
            let mut den = u128::from(den);
            let g = gcd(num, den);
            num /= g;
            den /= g;
            let g = gcd(self.mantissa.unsigned_abs(), den);
            let reduced_mantissa = self.mantissa / i128::try_from(g).ok()?;
            den /= g;
            let mut mantissa = reduced_mantissa.checked_mul(i128::try_from(num).ok()?)?;
            let mut scale = self.scale;
            let mut den = i128::try_from(den).ok()?;
            // A finite decimal exists iff the reduced denominator is
            // 2^a·5^b: clear each factor by scaling the mantissa.
            while den % 2 == 0 {
                mantissa = mantissa.checked_mul(5)?;
                scale += 1;
                den /= 2;
            }
            while den % 5 == 0 {
                mantissa = mantissa.checked_mul(2)?;
                scale += 1;
                den /= 5;
            }
            if den != 1 {
                return None;
            }
            while scale > 0 && mantissa % 10 == 0 {
                mantissa /= 10;
                scale -= 1;
            }
            if mantissa == 0 {
                scale = 0;
            }
            Some(Decimal { mantissa, scale })
        }

        /// Parse `[-]digits[.digits]`. Deliberately accepts what people
        /// type — `.5`, `5.`, `007`, `-0`, `1.50` — and normalizes: this
        /// is the entry point of the checked text→decimal cast (§3),
        /// where refusing `1.50` would be pedantry. Decoders that need
        /// one-value-one-text (the wire, §5) require the parsed value's
        /// `Display` to equal the input; `parse` alone is not that check.
        pub fn parse(s: &str) -> Result<Self, PrimitiveError> {
            let (neg, rest) = match s.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, s),
            };
            let (int_part, frac_part) = match rest.split_once('.') {
                Some((i, f)) => (i, f),
                None => (rest, ""),
            };
            if int_part.is_empty() && frac_part.is_empty() {
                return Err(PrimitiveError::Malformed("empty decimal"));
            }
            if !int_part.bytes().all(|b| b.is_ascii_digit())
                || !frac_part.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(PrimitiveError::Malformed("non-digit in decimal"));
            }
            let frac = frac_part.trim_end_matches('0');
            let mut mantissa: i128 = 0;
            for b in int_part.bytes().chain(frac.bytes()) {
                mantissa = mantissa
                    .checked_mul(10)
                    .and_then(|m| m.checked_add(i128::from(b - b'0')))
                    .ok_or(PrimitiveError::OutOfRange("decimal too large"))?;
            }
            if neg {
                mantissa = -mantissa;
            }
            let scale = if mantissa == 0 { 0 } else { frac.len() as u32 };
            Ok(Self { mantissa, scale })
        }
    }

    impl PartialOrd for Decimal {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }

    /// Total, exact, overflow-free ordering: sign, then integer-part
    /// length, then digitwise.
    impl Ord for Decimal {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            use std::cmp::Ordering;
            let sign = |d: &Decimal| d.mantissa.signum();
            match sign(self).cmp(&sign(other)) {
                Ordering::Equal => {}
                unequal => return unequal,
            }
            let (a_int, a_frac) = self.magnitude();
            let (b_int, b_frac) = other.magnitude();
            let magnitude = a_int
                .len()
                .cmp(&b_int.len())
                .then_with(|| a_int.cmp(&b_int))
                .then_with(|| {
                    let width = a_frac.len().max(b_frac.len());
                    let mut a = a_frac.clone();
                    let mut b = b_frac.clone();
                    a.extend(std::iter::repeat_n('0', width - a_frac.len()));
                    b.extend(std::iter::repeat_n('0', width - b_frac.len()));
                    a.cmp(&b)
                });
            if self.mantissa < 0 {
                magnitude.reverse()
            } else {
                magnitude
            }
        }
    }

    impl fmt::Display for Decimal {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            if self.scale == 0 {
                return write!(f, "{}", self.mantissa);
            }
            let sign = if self.mantissa < 0 { "-" } else { "" };
            let digits = self.mantissa.unsigned_abs().to_string();
            let scale = self.scale as usize;
            if digits.len() > scale {
                let (int_part, frac_part) = digits.split_at(digits.len() - scale);
                write!(f, "{sign}{int_part}.{frac_part}")
            } else {
                write!(f, "{sign}0.{digits:0>scale$}")
            }
        }
    }

    fn gcd(mut a: u128, mut b: u128) -> u128 {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a.max(1)
    }

    /// A calendar date. Wraps `jiff::civil::Date`; jiff never appears in
    /// the public API, so the backing crate stays swappable.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct Date(jiff::civil::Date);

    impl Date {
        /// Strict `YYYY-MM-DD`.
        pub fn parse(s: &str) -> Result<Self, PrimitiveError> {
            if s.len() != 10 {
                return Err(PrimitiveError::Malformed("expected YYYY-MM-DD"));
            }
            s.parse()
                .map(Self)
                .map_err(|_| PrimitiveError::Malformed("expected YYYY-MM-DD"))
        }
    }

    impl Date {
        /// The injective embedding of the date→datetime widening cast
        /// (§3): midnight UTC.
        pub fn at_midnight_utc(&self) -> Instant {
            Instant::parse(&format!("{self}T00:00:00Z")).expect("valid by construction")
        }
    }

    impl fmt::Display for Date {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    /// An RFC 3339 instant. Wraps `jiff::Timestamp`: equality and order
    /// are *semantic* — `…Z` and `…+00:00` are the same instant. The
    /// original offset is not retained; `Display` emits the normalized
    /// UTC form, which is also the canonical wire form (§5).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct Instant(jiff::Timestamp);

    impl Instant {
        /// Strict RFC 3339: `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)`,
        /// uppercase `T`/`Z`, seconds mandatory, no leap second (`:60`
        /// has no normalized form), no time-zone annotation, and the
        /// same year range as `Date` (0000–9999) — so a datetime always
        /// narrows to a date (§3) and never leaves the four-digit year
        /// the canonical form pins (§2.13). jiff's own parser is more
        /// liberal (space separators, lowercase, `[Europe/Paris]`
        /// annotations); those are one instant with several spellings,
        /// which a strict decoder must refuse.
        pub fn parse(s: &str) -> Result<Self, PrimitiveError> {
            const ERR: PrimitiveError = PrimitiveError::Malformed("expected RFC 3339 instant");
            let b = s.as_bytes();
            let digits = |range: std::ops::Range<usize>| {
                b.get(range).is_some_and(|d| d.iter().all(u8::is_ascii_digit))
            };
            // Date + 'T' + HH:MM:SS.
            let ok = b.len() >= 20
                && digits(0..4)
                && b[4] == b'-'
                && digits(5..7)
                && b[7] == b'-'
                && digits(8..10)
                && b[10] == b'T'
                && digits(11..13)
                && b[13] == b':'
                && digits(14..16)
                && b[16] == b':'
                && digits(17..19)
                && &b[17..19] != b"60";
            if !ok {
                return Err(ERR);
            }
            let mut i = 19;
            if b[i] == b'.' {
                let start = i + 1;
                while i + 1 < b.len() && b[i + 1].is_ascii_digit() {
                    i += 1;
                }
                if i + 1 == start {
                    return Err(ERR); // a bare '.'
                }
                i += 1;
            }
            let offset_ok = match b.get(i) {
                Some(b'Z') => i + 1 == b.len(),
                Some(b'+' | b'-') => {
                    i + 6 == b.len() && digits(i + 1..i + 3) && b[i + 3] == b':' && digits(i + 4..i + 6)
                }
                _ => false,
            };
            if !offset_ok {
                return Err(ERR);
            }
            let ts: jiff::Timestamp = s.parse().map_err(|_| ERR)?;
            let instant = Self(ts);
            // Normalizing to UTC can carry a date past 9999 or before
            // 0000 (an offset near the range's edge): refuse it, the
            // canonical form has no rendering for it.
            let rendered = instant.to_string();
            if rendered.len() < 10 || !rendered.as_bytes()[..4].iter().all(u8::is_ascii_digit) {
                return Err(ERR);
            }
            Ok(instant)
        }

        /// The lossy datetime→date cast (§3): the UTC calendar date.
        /// Display is normalized UTC RFC 3339 with a four-digit year
        /// (guaranteed by `parse`), so the first ten characters are
        /// exactly the date.
        pub fn utc_date(&self) -> Date {
            Date::parse(&self.to_string()[..10]).expect("four-digit year guaranteed by parse")
        }
    }

    impl fmt::Display for Instant {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn decimal_normalizes() {
            let a = Decimal::parse("1.50").unwrap();
            let b = Decimal::parse("1.5").unwrap();
            assert_eq!(a, b);
            assert_eq!(a.to_string(), "1.5");
            assert_eq!(Decimal::parse("-0.05").unwrap().to_string(), "-0.05");
            assert_eq!(Decimal::parse("0.0").unwrap().to_string(), "0");
            assert!(Decimal::parse("1,5").is_err());
            assert!(Decimal::parse("").is_err());
        }

        #[test]
        fn mul_div_exact_is_exact_or_nothing() {
            let d = |s: &str| Decimal::parse(s).unwrap();
            // 1500 m → km (×1000/1e6)
            assert_eq!(d("1500").mul_div_exact(1_000, 1_000_000), Some(d("1.5")));
            // 90 min → h (×1/60)
            assert_eq!(d("90").mul_div_exact(1, 60), Some(d("1.5")));
            // 100 min → h: 5/3 has no finite decimal — refused.
            assert_eq!(d("100").mul_div_exact(1, 60), None);
            // 2.5 h → min
            assert_eq!(d("2.5").mul_div_exact(60, 1), Some(d("150")));
            assert_eq!(d("0").mul_div_exact(1, 60), Some(d("0")));
            assert_eq!(d("1").mul_div_exact(1, 0), None);
        }

        #[test]
        fn date_validates() {
            assert!(Date::parse("2024-02-29").is_ok());
            assert!(Date::parse("2023-02-29").is_err());
            assert!(Date::parse("2023-13-01").is_err());
            assert!(Date::parse("2023-1-01").is_err());
            assert_eq!(Date::parse("2024-02-29").unwrap().to_string(), "2024-02-29");
        }

        #[test]
        fn mul_div_exact_reduces_before_multiplying() {
            // 10^36 × 1000 overflows i128 as an intermediate, but the
            // exact result 10^33 fits: reduce first, never lose a
            // representable answer to an intermediate.
            let big = Decimal::parse("1000000000000000000000000000000000000").unwrap();
            assert_eq!(
                big.mul_div_exact(1000, 1_000_000).unwrap().to_string(),
                "1000000000000000000000000000000000"
            );
            // Still exact-or-nothing: 1 / 3 has no finite decimal.
            assert_eq!(Decimal::parse("1").unwrap().mul_div_exact(1, 3), None);
            // And still refuses a genuine overflow.
            assert_eq!(big.mul_div_exact(u64::MAX, 1), None);
        }

        #[test]
        fn instant_equality_is_semantic() {
            let z = Instant::parse("2026-08-16T12:00:00Z").unwrap();
            let offset = Instant::parse("2026-08-16T14:00:00+02:00").unwrap();
            assert_eq!(z, offset);
            assert_eq!(offset.to_string(), "2026-08-16T12:00:00Z");
            assert!(Instant::parse("2026-08-16T12:00:00.123+02:00").is_ok());
            assert!(Instant::parse("2026-08-16T24:00:00Z").is_err());
        }

        #[test]
        fn instant_parse_is_strict_rfc_3339_within_the_date_year_range() {
            // One instant, one spelling: the liberal forms jiff accepts
            // are refused, so a decoder cannot be fed two texts for one
            // value.
            for liberal in [
                "2026-08-16 12:00:00Z",       // space separator
                "2026-08-16t12:00:00z",       // lowercase
                "2026-08-16T12:00Z",          // no seconds
                "2026-08-16T12:00:00,5Z",     // comma fraction
                "2026-08-16T12:00:00.Z",      // bare dot
                "2026-08-16T12:00:00+02:00[Europe/Paris]",
                "2026-08-16T23:59:60Z",       // leap second: no normalized form
                "2026-08-16T12:00:00+0200",   // offset without colon
                "-000001-06-01T00:00:00Z",    // signed six-digit year
                "+012026-08-16T12:00:00Z",
                "9999-12-31T23:00:00-02:00",  // normalizes past 9999
            ] {
                assert!(Instant::parse(liberal).is_err(), "{liberal} should be refused");
            }
            for strict in ["2026-08-16T12:00:00Z", "0000-01-01T00:00:00Z", "9999-12-30T12:00:00.5Z"] {
                let i = Instant::parse(strict).unwrap();
                // Never panics, always the first ten characters.
                assert_eq!(i.utc_date().to_string(), &strict[..10]);
            }
        }
    }
}

pub mod canonical;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_path_depth() {
        let root = RowPath::root();
        assert!(root.is_root());
        assert_eq!(root.depth(), 0);

        let item = root.child(PathSeg {
            group: GroupId::new("g1"),
            item: ItemId::new("i1"),
        });
        assert_eq!(item.depth(), 1);
        assert!(!item.is_root());
        assert!(root.is_root(), "child must not mutate the parent");
    }
}
