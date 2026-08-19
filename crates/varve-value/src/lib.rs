//! Tier 1 (§7): cells, items, typed conformance, structural diff and
//! patch. Pure and stateless — the record log lives in `varve-record`.
//!
//! Stored cell state is two-valued plus absence (§2.4): a cell is absent
//! (not in the map), `Empty` (written, blank), or holds a value.
//! Reachability is derived, surface-relative, and deliberately not
//! representable here.

#![forbid(unsafe_code)]

mod conformance;
mod patch;
mod report;

pub use conformance::{ConformanceError, check};
pub use patch::{ApplyError, Op, apply, diff};
pub use report::{ElementChanges, cell_delta};

use std::collections::BTreeMap;

use varve_core::canonical::CanonicalValue;
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, GroupId, ItemId, OptionId, RowPath};

/// One scalar value — the value side of the nine `ScalarType`s.
/// (`PartialEq` only: geometry features carry JSON numbers.)
#[derive(Debug, Clone, PartialEq)]
pub enum Scalar {
    Text(String),
    Boolean(bool),
    Integer(i64),
    Decimal(Decimal),
    Date(Date),
    Datetime(Instant),
    /// A member of the column's nomenclature; cells store option ids,
    /// labels live in the revision (§2.11).
    Enum(OptionId),
    /// Boxed, like geometry: the two large scalars — keeps every cell
    /// small (§2.4).
    Attachment(Box<AttachmentRef>),
    Geometry(Box<Feature>),
}

/// Content-addressed attachment reference (§2.15). `id` is the element
/// identity used by diff (§2.4) — two identical files in one cell stay
/// distinguishable. `content_type` and `byte_size` are *claims*,
/// checked by conformance with zero IO and verified against bytes by
/// the Tier 5 store at ingest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRef {
    pub id: String,
    pub hash: varve_core::canonical::ContentHash,
    pub filename: String,
    pub content_type: String,
    pub byte_size: u64,
}

/// One GeoJSON Feature (RFC 7946), validated at construction. A feature
/// set is an arity `many` cell — never a FeatureCollection (M0 residue).
///
/// Validated through `geojson`; the crate never appears in the public
/// API and the parsed structure is not retained — the kernel never
/// computes geometry. The feature's **canonical form** (§2.13 decision 3) is its JSON tree
/// as a [`CanonicalValue`] with every number a double — JCS semantics,
/// so coordinates and property numbers render per ES6 (`1.0` → `1`,
/// `-0.0` → `0`). Equality and `Display` are defined over that form:
/// two features are equal iff their canonical bytes are, and
/// `to_string()` *is* the JCS text.
#[derive(Debug, Clone)]
pub struct Feature {
    canonical: CanonicalValue,
    /// GeoJSON's native feature id, normalized to text (numeric ids are
    /// rendered per ES6): the element identity for diff (§2.4).
    id: Option<String>,
}

impl PartialEq for Feature {
    fn eq(&self, other: &Self) -> bool {
        self.canonical == other.canonical
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GeometryError {
    /// Not parseable as GeoJSON at all.
    #[error("not parseable as GeoJSON")]
    Malformed,
    /// Valid GeoJSON, but a bare geometry or a FeatureCollection — a
    /// geometry cell holds Features only.
    #[error("a geometry cell holds Features only, not bare geometries or FeatureCollections")]
    NotAFeature,
}

impl Feature {
    pub fn parse(s: &str) -> Result<Self, GeometryError> {
        match s.parse::<geojson::GeoJson>() {
            Ok(geojson::GeoJson::Feature(feature)) => Ok(Self::from_geojson(feature)),
            Ok(_) => Err(GeometryError::NotAFeature),
            Err(_) => Err(GeometryError::Malformed),
        }
    }

    /// Decode from canonical form (the inverse of [`Feature::to_canonical`]).
    /// Any JSON-shaped value is accepted; it must be a Feature.
    pub fn from_canonical(value: &CanonicalValue) -> Result<Self, GeometryError> {
        match geojson::GeoJson::from_json_value(canonical_to_json(value)) {
            Ok(geojson::GeoJson::Feature(feature)) => Ok(Self::from_geojson(feature)),
            Ok(_) => Err(GeometryError::NotAFeature),
            Err(_) => Err(GeometryError::Malformed),
        }
    }

    fn from_geojson(feature: geojson::Feature) -> Self {
        let object = geojson::JsonObject::from(&feature);
        let canonical = json_to_canonical(&serde_json::Value::Object(object));
        let id = match &canonical {
            CanonicalValue::Object(m) => match m.get("id") {
                Some(CanonicalValue::String(s)) => Some(s.clone()),
                Some(v @ CanonicalValue::Float(_)) => Some(canonical_text(v)),
                _ => None,
            },
            _ => None,
        };
        Self { canonical, id }
    }

    /// The canonical form (§2.13): the Feature as a JSON value, numbers
    /// as doubles. This is what entries commit to and what the wire
    /// carries — never a stringified blob.
    pub fn to_canonical(&self) -> &CanonicalValue {
        &self.canonical
    }

    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }
}

impl std::fmt::Display for Feature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&canonical_text(&self.canonical))
    }
}

/// JCS text of a value that came from JSON — never NaN/∞, never an
/// unsafe integer (numbers are doubles), so serialization cannot fail.
fn canonical_text(value: &CanonicalValue) -> String {
    let bytes = varve_core::canonical::canonical_bytes(value)
        .expect("JSON-derived values are finite doubles");
    String::from_utf8(bytes).expect("JCS output is UTF-8")
}

/// JSON tree → canonical form. Every number becomes a double: JCS
/// numbers *are* doubles, so `1` and `1.0` (and `-0.0` and `0`) are one
/// value with one rendering — equality on the canonical form is byte
/// equality.
fn json_to_canonical(v: &serde_json::Value) -> CanonicalValue {
    match v {
        serde_json::Value::Null => CanonicalValue::Null,
        serde_json::Value::Bool(b) => CanonicalValue::Bool(*b),
        serde_json::Value::Number(n) => {
            CanonicalValue::Float(n.as_f64().expect("serde_json numbers are finite"))
        }
        serde_json::Value::String(s) => CanonicalValue::String(s.clone()),
        serde_json::Value::Array(a) => {
            CanonicalValue::Array(a.iter().map(json_to_canonical).collect())
        }
        serde_json::Value::Object(o) => CanonicalValue::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), json_to_canonical(v)))
                .collect(),
        ),
    }
}

fn canonical_to_json(v: &CanonicalValue) -> serde_json::Value {
    match v {
        CanonicalValue::Null => serde_json::Value::Null,
        CanonicalValue::Bool(b) => serde_json::Value::Bool(*b),
        CanonicalValue::Int(i) => serde_json::Value::from(*i),
        // Finite by construction upstream; a NaN would only arise from
        // in-process misuse and is mapped to null rather than panicking.
        CanonicalValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        CanonicalValue::String(s) => serde_json::Value::String(s.clone()),
        CanonicalValue::Array(a) => {
            serde_json::Value::Array(a.iter().map(canonical_to_json).collect())
        }
        CanonicalValue::Object(o) => serde_json::Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), canonical_to_json(v)))
                .collect(),
        ),
    }
}

impl Scalar {
    /// Value-internal element identity (§2.4): used by diff only, opaque
    /// to the logic language. Enum options are self-identifying.
    pub fn element_id(&self) -> Option<&str> {
        match self {
            Scalar::Enum(id) => Some(id.as_str()),
            Scalar::Attachment(a) => Some(&a.id),
            Scalar::Geometry(f) => f.id(),
            _ => None,
        }
    }
}

/// The value of a written, non-blank cell.
#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    One(Scalar),
    /// A list *value* (§2.2): contributes nothing to the row path.
    Many(Vec<Scalar>),
}

/// Stored state of a written cell. Absence is "not in the map".
#[derive(Debug, Clone, PartialEq)]
pub enum CellState {
    /// Written, blank (§2.4) — distinct from absent.
    Empty,
    Value(CellValue),
}

/// Addressing identity of a cell: `(column_id, row_path)` (§2.4).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CellAddr {
    pub column: ColumnId,
    pub path: RowPath,
}

/// Where a `many` group's item list lives: the group plus the row path of
/// its enclosing scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ItemsAddr {
    pub group: GroupId,
    pub parent: RowPath,
}

/// The flat value store of one record (§2.5: group values are views over
/// cells sharing their prefix — nothing composite is stored).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordValues {
    pub cells: BTreeMap<CellAddr, CellState>,
    /// Ordered item lists, one per `many`-group instance.
    pub items: BTreeMap<ItemsAddr, Vec<ItemId>>,
}

impl RecordValues {
    pub fn new() -> Self {
        Self::default()
    }
}
