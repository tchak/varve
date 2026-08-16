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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// One GeoJSON Feature, opaque to the kernel; a feature set is an arity
/// `many` cell, never a FeatureCollection (§2.12 discussion, M0 residue).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    /// GeoJSON's native feature id: the element identity for diff.
    pub id: Option<String>,
    pub geojson: String,
}

impl Scalar {
    /// Value-internal element identity (§2.4): used by diff only, opaque
    /// to the logic language. Enum options are self-identifying.
    pub fn element_id(&self) -> Option<&str> {
        match self {
            Scalar::Enum(id) => Some(id.as_str()),
            Scalar::Attachment(a) => Some(&a.id),
            Scalar::Geometry(f) => f.id.as_deref(),
            _ => None,
        }
    }
}

/// The value of a written, non-blank cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellValue {
    One(Scalar),
    /// A list *value* (§2.2): contributes nothing to the row path.
    Many(Vec<Scalar>),
}

/// Stored state of a written cell. Absence is "not in the map".
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
