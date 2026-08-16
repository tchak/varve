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

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PrimitiveError {
        Malformed(&'static str),
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
        pub fn parse(s: &str) -> Result<Self, PrimitiveError> {
            s.parse()
                .map(Self)
                .map_err(|_| PrimitiveError::Malformed("expected RFC 3339 instant"))
        }

        /// The lossy datetime→date cast (§3): the UTC calendar date.
        /// Display is normalized UTC RFC 3339, so the first ten
        /// characters are exactly the date.
        pub fn utc_date(&self) -> Date {
            Date::parse(&self.to_string()[..10]).expect("normalized display")
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
        fn date_validates() {
            assert!(Date::parse("2024-02-29").is_ok());
            assert!(Date::parse("2023-02-29").is_err());
            assert!(Date::parse("2023-13-01").is_err());
            assert!(Date::parse("2023-1-01").is_err());
            assert_eq!(Date::parse("2024-02-29").unwrap().to_string(), "2024-02-29");
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
