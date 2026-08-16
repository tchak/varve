//! Tier 1 (§7): cells, items, typed conformance, structural diff and
//! patch. Pure and stateless — the record log lives in `strata-record`.
//!
//! Stored cell state is two-valued plus absence (§2.4): a cell is absent
//! (not in the map), `Empty` (written, blank), or holds a value.
//! Reachability is derived, surface-relative, and deliberately not
//! representable here.

#![forbid(unsafe_code)]

mod conformance;
mod patch;
mod report;

pub use conformance::{ConformanceError, NomenclatureTable, check};
pub use patch::{ApplyError, Op, apply, diff};
pub use report::{ElementChanges, cell_delta};

use std::collections::BTreeMap;

use strata_core::primitives::{Date, Decimal, Instant};
use strata_core::{ColumnId, GroupId, ItemId, OptionId, RowPath};

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
    Attachment(AttachmentRef),
    Geometry(Feature),
}

/// Content-addressed attachment reference. `id` is the element identity
/// used by diff (§2.4) — two identical files in one cell stay
/// distinguishable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentRef {
    pub id: String,
    pub sha256: String,
    pub filename: String,
}

/// One GeoJSON Feature (RFC 7946), validated at construction. A feature
/// set is an arity `many` cell — never a FeatureCollection (M0 residue).
///
/// Wraps `geojson::Feature`; the crate never appears in the public API.
/// Equality is semantic (parsed structure, not text), and serialization
/// is key-sorted, deterministic.
#[derive(Debug, Clone, PartialEq)]
pub struct Feature {
    feature: geojson::Feature,
    /// GeoJSON's native feature id, normalized to text (numeric ids are
    /// rendered): the element identity for diff (§2.4).
    id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeometryError {
    /// Not parseable as GeoJSON at all.
    Malformed,
    /// Valid GeoJSON, but a bare geometry or a FeatureCollection — a
    /// geometry cell holds Features only.
    NotAFeature,
}

impl Feature {
    pub fn parse(s: &str) -> Result<Self, GeometryError> {
        match s.parse::<geojson::GeoJson>() {
            Ok(geojson::GeoJson::Feature(feature)) => {
                let id = feature.id.as_ref().map(|id| match id {
                    geojson::feature::Id::String(s) => s.clone(),
                    geojson::feature::Id::Number(n) => n.to_string(),
                });
                Ok(Self { feature, id })
            }
            Ok(_) => Err(GeometryError::NotAFeature),
            Err(_) => Err(GeometryError::Malformed),
        }
    }

    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }
}

impl std::fmt::Display for Feature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.feature)
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
