//! `Slug`: an organization's URL/API handle as a scalar of its own.
//! Parsing is normalization plus validation, so a resolver receives
//! a slug already in the stored form and never validates one — the
//! 8.0 stance that a format is a type, not a predicate on `String`.

use async_graphql::{InputValueError, InputValueResult, Scalar, ScalarType, Value};

use crate::error::Code;

/// A normalized, valid slug: non-empty, `[a-z0-9-]` only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slug(String);

impl Slug {
    /// Normalizes (`platform_core::normalize_slug`: trim, lowercase)
    /// then validates. The one constructor; `Slug` cannot hold an
    /// invalid value.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let slug = platform_core::normalize_slug(raw);
        let ok = !slug.is_empty()
            && slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if ok {
            Ok(Self(slug))
        } else {
            Err("slug must be non-empty and contain only a-z, 0-9, and '-'".to_owned())
        }
    }

    /// The stored form.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&platform_core::Organization> for Slug {
    /// A stored slug is normalized and valid by construction
    /// (`platform-core` only ever stores what [`Slug::parse`]
    /// accepted), so this never re-validates.
    fn from(organization: &platform_core::Organization) -> Self {
        Self(organization.slug.clone())
    }
}

/// URL/API handle of an organization: `[a-z0-9-]`, non-empty. Input
/// is trimmed and lowercased before validation.
#[Scalar(name = "Slug")]
impl ScalarType for Slug {
    fn parse(value: Value) -> InputValueResult<Self> {
        let Value::String(raw) = &value else {
            return Err(InputValueError::expected_type(value));
        };
        Slug::parse(raw).map_err(|message| {
            InputValueError::custom(message).with_extension("code", Code::InvalidInput.as_str())
        })
    }

    fn to_value(&self) -> Value {
        Value::String(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_normalizes_then_validates() {
        assert_eq!(Slug::parse("  Ville-2  ").unwrap().as_str(), "ville-2");
        for bad in ["", "   ", "Not Valid!", "é", "a_b"] {
            assert!(Slug::parse(bad).is_err(), "{bad:?}");
        }
    }
}
