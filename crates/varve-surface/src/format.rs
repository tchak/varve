//! §2.6 format constraints: surface admissibility over `text`. The
//! schema type stays plain text; a mis-formatted value is
//! non-admissible, never ill-typed. Checks are structural and
//! deliberately lenient except IBAN, which has a real checksum
//! (ISO 13616 mod-97).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Email,
    Phone,
    Iban,
}

impl Format {
    pub fn check(self, value: &str) -> bool {
        match self {
            Format::Email => email(value),
            Format::Phone => phone(value),
            Format::Iban => iban(value),
        }
    }
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
}
