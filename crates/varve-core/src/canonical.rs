//! Canonical bytes and content addresses (§2.13).
//!
//! Two hashing regimes: schema-side objects hash **plain** (identical
//! schemas converge on identical ids everywhere); record-side objects
//! hash as **salted commitments** — never plaintext (§2.10).
//!
//! Canonical form is JCS (RFC 8785) over JSON-shaped values: object keys
//! sorted by UTF-16 code units, minimal string escaping, ES6 number
//! serialization. Salts are inputs — Tier 0 has no randomness.

use std::collections::BTreeMap;
use std::fmt;

use sha2::{Digest, Sha256};

/// The largest integer a JSON number can carry exactly under RFC 8785:
/// JCS numbers are IEEE 754 doubles, so 2^53 − 1 (ES6
/// `Number.MAX_SAFE_INTEGER`) bounds `CanonicalValue::Int`.
pub const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// A JSON-shaped value ready for canonicalization.
///
/// Kernel scalars arrive pre-rendered per §2.13: exact numbers
/// (integers, decimals) and instants as their normalized strings. JSON
/// numbers are reserved for JCS-safe structural counts (`Int`) and
/// geometry numbers (`Float`).
#[derive(Debug, Clone, PartialEq)]
pub enum CanonicalValue {
    Null,
    Bool(bool),
    /// A JCS-safe integer: |i| ≤ [`MAX_SAFE_INTEGER`]. Anything larger
    /// is not representable as an RFC 8785 number (a verifier in any
    /// other language would round it) and errors at serialization —
    /// full-range i64 travels as a string (`Scalar::Integer`).
    Int(i64),
    /// Serialized per ES6 `Number::toString` (JCS). NaN and infinities
    /// are not representable and error at serialization.
    Float(f64),
    String(String),
    Array(Vec<CanonicalValue>),
    Object(BTreeMap<String, CanonicalValue>),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    /// JCS forbids NaN and infinities.
    #[error("JCS forbids NaN and infinities")]
    NonFiniteNumber,
    /// JCS numbers are doubles: integers beyond ±(2^53 − 1) cannot be
    /// serialized exactly and are refused rather than rounded.
    #[error("integer {0} is outside the JCS-safe range (|i| ≤ 2^53 − 1)")]
    UnsafeInteger(i64),
}

/// RFC 8785 canonical bytes.
pub fn canonical_bytes(value: &CanonicalValue) -> Result<Vec<u8>, CanonicalError> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out.into_bytes())
}

fn write_value(value: &CanonicalValue, out: &mut String) -> Result<(), CanonicalError> {
    match value {
        CanonicalValue::Null => out.push_str("null"),
        CanonicalValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        CanonicalValue::Int(i) => {
            if i.unsigned_abs() > MAX_SAFE_INTEGER as u64 {
                return Err(CanonicalError::UnsafeInteger(*i));
            }
            // Within the safe range the ES6 rendering of an integral
            // double is its plain decimal digits.
            out.push_str(&i.to_string());
        }
        CanonicalValue::Float(f) => {
            if !f.is_finite() {
                return Err(CanonicalError::NonFiniteNumber);
            }
            out.push_str(&format_es6_number(*f));
        }
        CanonicalValue::String(s) => write_string(s, out),
        CanonicalValue::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        CanonicalValue::Object(map) => {
            // JCS sorts keys by UTF-16 code units — not by Unicode code
            // points: supplementary characters (surrogate pairs) sort
            // *before* U+E000..U+FFFF.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(&map[*key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// ES6 `QuoteJSONString`: minimal escaping, literal UTF-8 for everything
/// above U+001F.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{09}' => out.push_str("\\t"),
            '\u{0a}' => out.push_str("\\n"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{0d}' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// ES6 `Number::toString` over the shortest decimal representation
/// (ryu), as required by RFC 8785.
fn format_es6_number(x: f64) -> String {
    if x == 0.0 {
        // Covers -0.0: JCS serializes negative zero as "0".
        return "0".to_string();
    }
    let negative = x.is_sign_negative();
    let mut buffer = ryu::Buffer::new();
    let shortest = buffer.format_finite(x.abs());

    // Parse ryu output (`123.456`, `1e30`, `1.5e-7`, …) into a digit
    // string and a base-10 exponent: value = digits × 10^e10.
    let (mantissa, exp) = match shortest.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().expect("ryu exponent")),
        None => (shortest, 0),
    };
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mantissa, ""),
    };
    let mut digits: String = format!("{int_part}{frac_part}");
    let mut e10 = exp - frac_part.len() as i32;
    let leading = digits.len() - digits.trim_start_matches('0').len();
    digits.drain(..leading);
    while digits.ends_with('0') {
        digits.pop();
        e10 += 1;
    }

    let k = digits.len() as i32;
    let n = e10 + k; // Position of the decimal point.
    let s = digits;
    let core = if k <= n && n <= 21 {
        format!("{s}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &s[..n as usize], &s[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{s}", "0".repeat(-n as usize))
    } else {
        let mantissa = if k == 1 {
            s.clone()
        } else {
            format!("{}.{}", &s[..1], &s[1..])
        };
        let exponent = n - 1;
        format!(
            "{mantissa}e{}{}",
            if exponent >= 0 { "+" } else { "-" },
            exponent.abs()
        )
    };
    if negative { format!("-{core}") } else { core }
}

/// Hash algorithm tag (§2.13 decision 6): one tag per address so
/// migration stays representable. Kernel addresses are SHA-256.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HashAlg {
    Sha256,
}

impl HashAlg {
    fn tag(self) -> &'static str {
        match self {
            HashAlg::Sha256 => "sha256",
        }
    }
}

/// A content address: algorithm tag plus digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash {
    pub alg: HashAlg,
    pub digest: [u8; 32],
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:", self.alg.tag())?;
        for byte in self.digest {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a content hash (expected \"sha256:<64 hex digits>\")")]
pub struct ParseHashError;

impl std::str::FromStr for ContentHash {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, ParseHashError> {
        let hex = s.strip_prefix("sha256:").ok_or(ParseHashError)?;
        // Lowercase hex only: one address, one text (`from_str_radix`
        // would also take `+` and uppercase).
        if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(ParseHashError);
        }
        let mut digest = [0u8; 32];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let chunk = std::str::from_utf8(chunk).map_err(|_| ParseHashError)?;
            digest[i] = u8::from_str_radix(chunk, 16).map_err(|_| ParseHashError)?;
        }
        Ok(ContentHash {
            alg: HashAlg::Sha256,
            digest,
        })
    }
}

fn sha256(parts: &[&[u8]]) -> ContentHash {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    ContentHash {
        alg: HashAlg::Sha256,
        digest: hasher.finalize().into(),
    }
}

/// Plain hash — the schema-side regime (§2.13 decision 1). Never use for
/// record data: values would be brute-forceable after erasure.
pub fn hash_plain(value: &CanonicalValue) -> Result<ContentHash, CanonicalError> {
    Ok(sha256(&[&canonical_bytes(value)?]))
}

/// A 32-byte salt. Always an *input*: generated at Tier 5 append time,
/// stored with the content it commits, destroyed with it (§2.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Salt(pub [u8; 32]);

/// Salted commitment — the record-side regime. `H(salt ‖ canonical)`:
/// verifiable by anyone holding content + salt, brute-force-resistant
/// once both are withheld or destroyed.
pub fn commit(salt: &Salt, value: &CanonicalValue) -> Result<ContentHash, CanonicalError> {
    Ok(sha256(&[&salt.0, &canonical_bytes(value)?]))
}

/// Vector commitment over per-op commitments (§2.13 decision 4): the
/// entry's content hash. A filtered export discloses some ops (content +
/// salt, recomputable) and withholds others (commitment only); this hash
/// verifies identically either way.
pub fn commit_vector(commitments: &[ContentHash]) -> ContentHash {
    let list = CanonicalValue::Array(
        commitments
            .iter()
            .map(|c| CanonicalValue::String(c.to_string()))
            .collect(),
    );
    sha256(&[&canonical_bytes(&list).expect("strings never fail")])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: Vec<(&str, CanonicalValue)>) -> CanonicalValue {
        CanonicalValue::Object(
            pairs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    fn bytes(value: &CanonicalValue) -> String {
        String::from_utf8(canonical_bytes(value).unwrap()).unwrap()
    }

    #[test]
    fn integers_are_jcs_safe_or_refused() {
        // Within ±(2^53 − 1) an integer renders as its digits — the same
        // bytes ES6 produces for the integral double.
        assert_eq!(bytes(&CanonicalValue::Int(MAX_SAFE_INTEGER)), "9007199254740991");
        assert_eq!(bytes(&CanonicalValue::Int(-MAX_SAFE_INTEGER)), "-9007199254740991");
        assert_eq!(bytes(&CanonicalValue::Int(0)), "0");
        assert_eq!(
            bytes(&CanonicalValue::Int(MAX_SAFE_INTEGER)),
            bytes(&CanonicalValue::Float(MAX_SAFE_INTEGER as f64))
        );
        // Beyond it, a double cannot hold the value: refuse, never round.
        for i in [MAX_SAFE_INTEGER + 1, -(MAX_SAFE_INTEGER + 1), i64::MAX, i64::MIN + 1] {
            assert_eq!(
                canonical_bytes(&CanonicalValue::Int(i)),
                Err(CanonicalError::UnsafeInteger(i))
            );
        }
        // i64::MIN: `abs` would overflow — must still be refused cleanly.
        assert_eq!(
            canonical_bytes(&CanonicalValue::Int(i64::MIN)),
            Err(CanonicalError::UnsafeInteger(i64::MIN))
        );
    }

    #[test]
    fn jcs_structure_and_escaping() {
        let value = obj(vec![
            ("b", CanonicalValue::Int(2)),
            ("a", CanonicalValue::Null),
            (
                "s",
                CanonicalValue::String("a\"b\\c\n\u{0f}€/".to_string()),
            ),
            (
                "arr",
                CanonicalValue::Array(vec![
                    CanonicalValue::Bool(true),
                    CanonicalValue::Bool(false),
                ]),
            ),
        ]);
        assert_eq!(
            bytes(&value),
            "{\"a\":null,\"arr\":[true,false],\"b\":2,\"s\":\"a\\\"b\\\\c\\n\\u000f€/\"}"
        );
    }

    #[test]
    fn jcs_key_order_is_utf16_not_codepoint() {
        // U+10000 (surrogate pair D800 DC00) sorts BEFORE U+FFFD in
        // UTF-16 order, though its code point is higher.
        let value = obj(vec![
            ("\u{FFFD}", CanonicalValue::Int(1)),
            ("\u{10000}", CanonicalValue::Int(2)),
        ]);
        let out = bytes(&value);
        let supplementary = out.find('\u{10000}').unwrap();
        let replacement = out.find('\u{FFFD}').unwrap();
        assert!(supplementary < replacement);
    }

    #[test]
    fn es6_numbers() {
        let cases: &[(f64, &str)] = &[
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.5, "-1.5"),
            (100.0, "100"),
            (123.456, "123.456"),
            (0.5, "0.5"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1.5e300, "1.5e+300"),
            (4.5, "4.5"),
            (0.002, "0.002"),
            (1e30, "1e+30"),
            (1e-27, "1e-27"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                format_es6_number(*input),
                *expected,
                "for input {input:e}"
            );
        }
        assert_eq!(
            canonical_bytes(&CanonicalValue::Float(f64::NAN)),
            Err(CanonicalError::NonFiniteNumber)
        );
    }

    #[test]
    fn es6_extreme_vectors() {
        // Boundary/extreme values with ES6-certain renderings.
        let cases: &[(f64, &str)] = &[
            (1e23, "1e+23"),
            (5e-324, "5e-324"),
            (f64::MAX, "1.7976931348623157e+308"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1.2345678901234568e20, "123456789012345680000"),
        ];
        for (input, expected) in cases {
            assert_eq!(format_es6_number(*input), *expected);
        }
    }

    #[test]
    fn content_hash_roundtrip_and_known_digest() {
        // SHA-256("abc") — FIPS 180 test vector.
        let hash = sha256(&[b"abc"]);
        assert_eq!(
            hash.to_string(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let parsed: ContentHash = hash.to_string().parse().unwrap();
        assert_eq!(parsed, hash);
        assert!("sha256:zz".parse::<ContentHash>().is_err());
        assert!("blake3:00".parse::<ContentHash>().is_err());
    }

    #[test]
    fn plain_hash_converges_and_commitments_do_not() {
        let value = obj(vec![("name", CanonicalValue::String("Dupont".into()))]);
        assert_eq!(hash_plain(&value).unwrap(), hash_plain(&value).unwrap());

        let salt_a = Salt([1; 32]);
        let salt_b = Salt([2; 32]);
        // Same content, different salts → different commitments: no
        // brute-force oracle across records.
        assert_ne!(
            commit(&salt_a, &value).unwrap(),
            commit(&salt_b, &value).unwrap()
        );
        assert_eq!(
            commit(&salt_a, &value).unwrap(),
            commit(&salt_a, &value).unwrap()
        );
    }

    mod props {
        use super::super::*;
        use proptest::prelude::*;

        proptest! {
            /// Shortest-round-trip guarantees the ES6 rendering parses
            /// back to the exact same f64 — for every finite value.
            #[test]
            fn es6_rendering_round_trips(bits in any::<u64>()) {
                let x = f64::from_bits(bits);
                prop_assume!(x.is_finite());
                let rendered = format_es6_number(x);
                prop_assert_eq!(rendered.parse::<f64>().unwrap(), x);
            }

            /// The canonicalizer never panics and is deterministic over
            /// arbitrary strings (escaping, non-ASCII, controls).
            #[test]
            fn string_canonicalization_is_total(s in "\\PC*") {
                let v = CanonicalValue::String(s);
                let once = canonical_bytes(&v).unwrap();
                prop_assert_eq!(canonical_bytes(&v).unwrap(), once);
            }
        }
    }

    #[test]
    fn filtered_export_verifies_with_withheld_ops() {
        // Entry with two ops; the export discloses op1 (content + salt)
        // and withholds op2 (commitment only).
        let op1 = obj(vec![("set", CanonicalValue::String("visible".into()))]);
        let op2 = obj(vec![("set", CanonicalValue::String("secret".into()))]);
        let (salt1, salt2) = (Salt([3; 32]), Salt([4; 32]));
        let c1 = commit(&salt1, &op1).unwrap();
        let c2 = commit(&salt2, &op2).unwrap();
        let entry_hash = commit_vector(&[c1, c2]);

        // Receiver of the filtered export: recomputes op1's commitment
        // from disclosed content, uses op2's commitment as transmitted.
        let recomputed_c1 = commit(&salt1, &op1).unwrap();
        assert_eq!(commit_vector(&[recomputed_c1, c2]), entry_hash);
    }
}
