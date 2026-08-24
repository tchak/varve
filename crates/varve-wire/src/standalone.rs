//! A schema **outside a stream**: the canonical bytes of one `Schema`,
//! for a caller that needs to park an unpublished schema somewhere
//! (a platform draft, design/platform.md P.4) and get the identical
//! value back. The encoding is exactly the `schema` body of a
//! `revision` line (§5), so a draft serialized here and the revision
//! it later publishes as are the same bytes — and `revision_id` of
//! the decoded value is the id publication will assign.

use varve_core::canonical::{CanonicalValue, canonical_bytes};
use varve_schema::{Schema, schema_canonical, schema_from_canonical};

use crate::read::{JsonLine, ReadError};

/// The JCS canonical bytes of `schema` (its `revision`-line body).
///
/// Infallible: schemas carry no floats, the only value JCS can refuse
/// (the same invariant `revision_id` relies on).
pub fn schema_bytes(schema: &Schema) -> Vec<u8> {
    canonical_bytes(&schema_canonical(schema)).expect("schemas contain no floats")
}

/// Decodes bytes produced by [`schema_bytes`]. Strict like the stream
/// reader: any malformation is a [`ReadError`] (reported as line 1).
pub fn schema_from_bytes(bytes: &[u8]) -> Result<Schema, ReadError> {
    let value = canonical_from_bytes(bytes)?;
    schema_from_canonical(&value).map_err(|e| ReadError::Malformed {
        line: 1,
        reason: e.to_string(),
    })
}

/// Decodes one standalone canonical value from its JCS bytes — the
/// parse half every opaque line body shares (§5). For bodies whose
/// codec another crate owns (surfaces, block defaults —
/// `varve_surface::canon`), a store pairs this with that codec's
/// `*_from`; this crate stays ignorant of the body's shape.
pub fn canonical_from_bytes(bytes: &[u8]) -> Result<CanonicalValue, ReadError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ReadError::Malformed {
        line: 1,
        reason: "not UTF-8".into(),
    })?;
    match serde_json::from_str::<JsonLine>(text) {
        Ok(JsonLine(v)) => Ok(v),
        Err(e) if e.classify() == serde_json::error::Category::Data => Err(ReadError::Malformed {
            line: 1,
            reason: e.to_string(),
        }),
        Err(_) => Err(ReadError::Json { line: 1 }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use varve_core::ColumnId;
    use varve_schema::{Arity, Column, Element, ScalarType, revision_id};

    fn schema() -> Schema {
        Schema {
            root: vec![Element::Column(Column {
                id: ColumnId::new("nom"),
                label: "Nom".into(),
                ty: ScalarType::Text,
                arity: Arity::One,
            })],
            resolvers: vec![],
        }
    }

    #[test]
    fn round_trips_and_keeps_the_revision_id() {
        let s = schema();
        let bytes = schema_bytes(&s);
        let back = schema_from_bytes(&bytes).unwrap();
        assert_eq!(back, s);
        assert_eq!(revision_id(&back), revision_id(&s));
        // Byte-stable: re-encoding the decoded value is the identity.
        assert_eq!(schema_bytes(&back), bytes);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(
            schema_from_bytes(b"{not json").unwrap_err(),
            ReadError::Json { line: 1 }
        );
        assert!(matches!(
            schema_from_bytes(b"{\"elements\": 1}").unwrap_err(),
            ReadError::Malformed { line: 1, .. }
        ));
        assert!(matches!(
            schema_from_bytes(&[0xff]).unwrap_err(),
            ReadError::Malformed { line: 1, .. }
        ));
    }
}
