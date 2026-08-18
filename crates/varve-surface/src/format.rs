//! §2.6 format constraints: surface admissibility over `text`. The
//! schema type stays plain text; a mis-formatted value is
//! non-admissible, never ill-typed. Checks are structural and
//! deliberately lenient except IBAN, which has a real checksum
//! (ISO 13616 mod-97).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    Email,
    Phone,
    Iban,
    /// Author-supplied pattern (DN's "formatted" champ). Matched with
    /// the linear-time `regex` engine — author patterns run against
    /// user input, so no-backtracking is a security property (no
    /// ReDoS), not a convenience. Always full-match: wrapped
    /// `\A(?:pat)\z`, so forgotten anchors cannot accept substrings.
    /// Pattern validity is checked by `validate()` at publication.
    Regex(String),
}

/// A format ready to check many values: the custom pattern is compiled
/// once (per column, per admissibility run) instead of per cell.
pub enum CompiledFormat {
    Email,
    Phone,
    Iban,
    Regex(regex::Regex),
    /// An invalid pattern rejects nothing at runtime: it is a
    /// publication error (`validate`), not an applicant's problem.
    Invalid,
}

impl CompiledFormat {
    pub fn check(&self, value: &str) -> bool {
        match self {
            CompiledFormat::Email => email(value),
            CompiledFormat::Phone => phone(value),
            CompiledFormat::Iban => iban(value),
            CompiledFormat::Regex(re) => re.is_match(value),
            CompiledFormat::Invalid => true,
        }
    }
}

impl Format {
    pub fn compile(&self) -> CompiledFormat {
        match self {
            Format::Email => CompiledFormat::Email,
            Format::Phone => CompiledFormat::Phone,
            Format::Iban => CompiledFormat::Iban,
            Format::Regex(pattern) => match compile(pattern) {
                Ok(re) => CompiledFormat::Regex(re),
                Err(_) => CompiledFormat::Invalid,
            },
        }
    }

    /// One-off check; for many values compile once with [`Format::compile`].
    pub fn check(&self, value: &str) -> bool {
        self.compile().check(value)
    }

    /// Publication-time pattern verification for `validate()`.
    pub fn verify(&self) -> Result<(), String> {
        match self {
            Format::Regex(pattern) => compile(pattern).map(|_| ()),
            _ => Ok(()),
        }
    }
}

fn compile(pattern: &str) -> Result<regex::Regex, String> {
    regex::Regex::new(&format!(r"\A(?:{pattern})\z")).map_err(|e| e.to_string())
}

/// Structural: one `@`, non-empty local part, dotted domain, no
/// whitespace. Deliberately loose — RFC 5322 pedantry rejects real
/// addresses.
fn email(value: &str) -> bool {
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
}

/// Loose: optional leading `+`, then 6–15 digits ignoring common
/// separators (space, dot, dash, parentheses).
fn phone(value: &str) -> bool {
    let rest = value.strip_prefix('+').unwrap_or(value);
    let digits: String = rest
        .chars()
        .filter(|c| !matches!(c, ' ' | '.' | '-' | '(' | ')'))
        .collect();
    (6..=15).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit())
}

/// ISO 13616: strip spaces, uppercase, 15–34 alphanumerics, rotate the
/// first four to the end, letters as 10–35, mod 97 == 1.
fn iban(value: &str) -> bool {
    let compact: String = value
        .chars()
        .filter(|c| *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if !(15..=34).contains(&compact.len())
        || !compact.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return false;
    }
    let rotated = format!("{}{}", &compact[4..], &compact[..4]);
    let mut remainder: u32 = 0;
    for c in rotated.chars() {
        let value = c.to_digit(36).expect("alphanumeric");
        let shift = if value < 10 { 10 } else { 100 };
        remainder = (remainder * shift + value) % 97;
    }
    remainder == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert!(Format::Email.check("a@b.fr"));
        assert!(!Format::Email.check("a b@c.fr"));
        assert!(!Format::Email.check("nope"));
        assert!(Format::Phone.check("+33 6 12 34 56 78"));
        assert!(!Format::Phone.check("abc"));
        // Known-valid IBAN test vector; a corrupted digit fails.
        assert!(Format::Iban.check("GB82 WEST 1234 5698 7654 32"));
        assert!(!Format::Iban.check("GB82 WEST 1234 5698 7654 33"));
        assert!(Format::Iban.check("FR1420041010050500013M02606"));
    }

    #[test]
    fn custom_patterns_are_anchored_and_linear() {
        let code = Format::Regex("[0-9]{5}".into());
        assert!(code.check("75011"));
        // Full-match by construction: substrings do not pass.
        assert!(!code.check("x75011y"));
        assert!(!code.check("750112"));

        // The PCRE-killer pattern: compiles here and runs in linear
        // time — the engine guarantee that makes author-supplied
        // patterns safe against user input.
        let pathological = Format::Regex("(a+)+".into());
        assert!(pathological.check(&"a".repeat(1000)));
        assert!(!pathological.check(&format!("{}b", "a".repeat(1000))));

        // Backtracking-only constructs are rejected at verification —
        // DN patterns using them surface as counted residue at import.
        assert!(Format::Regex("(?=x)".into()).verify().is_err());
        assert!(Format::Regex("[0-9]{5}".into()).verify().is_ok());
    }
}
