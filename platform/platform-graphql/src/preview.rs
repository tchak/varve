//! The fillable preview (`design/graphql.md` G.12): the draft's
//! scratch value bag and its admissibility findings — the deliberate
//! **pilot of the case-file models**: the `Cell` union here is G.1's
//! `cells` read shape, and `CellWriteInput` is the input `updateCells`
//! will take, both fixed against scratch values first.
//!
//! Cells are addressed by `columnId` plus the row path — the
//! `{group, item}` segment chain of DESIGN §2.4. Exact numbers travel
//! as strings (integer, decimal): the kernel's wire precedent (§5) —
//! a JSON double would round the far half of `i64`.

use async_graphql::{Enum, ID, InputObject, OneofObject, SimpleObject, Union};
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RowPath};
use varve_surface::Finding;
use varve_value::{CellState, CellValue, RecordValues, Scalar};

use crate::error::invalid_input;

/// The draft's preview (G.12): scratch values an administrator fills
/// the form with, and what admissibility makes of them. Empty until
/// filled; cleared by discard and by publication.
#[derive(SimpleObject)]
pub struct Preview {
    /// Every written cell, in address order.
    pub cells: Vec<Cell>,
    /// Every `many` group's ordered item list (the row identities of
    /// DESIGN §2.4). Not derivable from `cells`: a freshly added item
    /// has no cells yet, and its server-minted id is what the next
    /// write addresses.
    pub items: Vec<ItemList>,
    /// Admissibility of the values against the draft, evaluated per
    /// compiled surface (`applicant` / `reviewer`) — the point of the
    /// preview: required and format findings are watched, never
    /// refused. Eligibility is not evaluated (no lifecycle in a
    /// preview).
    pub findings: Vec<AdmissibilityFinding>,
}

/// One `many` group instance's ordered items.
#[derive(SimpleObject)]
pub struct ItemList {
    pub group_id: ID,
    /// The enclosing scope's row path; empty = the root scope.
    pub parent: Vec<RowSegment>,
    /// Document order — `reorder`'s subject.
    pub item_ids: Vec<ID>,
}

/// One segment of a row path: which item of which `many` group
/// (DESIGN §2.4).
#[derive(SimpleObject, Clone)]
pub struct RowSegment {
    pub group_id: ID,
    pub item_id: ID,
}

/// A written cell (G.1's record read model, piloted here): one member
/// per value kind plus the written-blank state — `EMPTY` is a value
/// state, distinct from absence (DESIGN §2.4).
#[derive(Union)]
pub enum Cell {
    Text(TextCell),
    Boolean(BooleanCell),
    Integer(IntegerCell),
    Decimal(DecimalCell),
    Date(DateCell),
    Datetime(DatetimeCell),
    Enum(EnumCell),
    Empty(EmptyCell),
}

/// The cell constructors, carried by every member as `kind` (the
/// G.7.2 precedent).
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum CellKind {
    Text,
    Boolean,
    Integer,
    Decimal,
    Date,
    Datetime,
    Enum,
    Empty,
}

#[derive(SimpleObject)]
pub struct TextCell {
    pub kind: CellKind,
    pub column_id: ID,
    pub path: Vec<RowSegment>,
    pub value: String,
}

#[derive(SimpleObject)]
pub struct BooleanCell {
    pub kind: CellKind,
    pub column_id: ID,
    pub path: Vec<RowSegment>,
    pub value: bool,
}

/// An exact integer, as a decimal string (the §5 wire precedent —
/// full-range `i64` does not survive a JSON double).
#[derive(SimpleObject)]
pub struct IntegerCell {
    pub kind: CellKind,
    pub column_id: ID,
    pub path: Vec<RowSegment>,
    pub value: String,
}

/// An exact decimal, as its normalized string (`1.5`, never `1.50`).
#[derive(SimpleObject)]
pub struct DecimalCell {
    pub kind: CellKind,
    pub column_id: ID,
    pub path: Vec<RowSegment>,
    pub value: String,
}

/// An ISO 8601 date (`2026-08-26`).
#[derive(SimpleObject)]
pub struct DateCell {
    pub kind: CellKind,
    pub column_id: ID,
    pub path: Vec<RowSegment>,
    pub value: String,
}

/// An RFC 3339 instant, UTC.
#[derive(SimpleObject)]
pub struct DatetimeCell {
    pub kind: CellKind,
    pub column_id: ID,
    pub path: Vec<RowSegment>,
    pub value: String,
}

/// A choice cell: the selected option ids (DESIGN §2.11 — cells store
/// ids, labels live in the revision). One element on a single-select
/// column; the column's `multiple` says which.
#[derive(SimpleObject)]
pub struct EnumCell {
    pub kind: CellKind,
    pub column_id: ID,
    pub path: Vec<RowSegment>,
    pub option_ids: Vec<ID>,
}

/// Written blank (DESIGN §2.4) — distinct from an absent cell, which
/// is simply not in `cells`.
#[derive(SimpleObject)]
pub struct EmptyCell {
    pub kind: CellKind,
    pub column_id: ID,
    pub path: Vec<RowSegment>,
}

/// One admissibility finding, tagged with the compiled surface it
/// holds on.
#[derive(Union)]
pub enum AdmissibilityFinding {
    MissingRequired(MissingRequiredFinding),
    FormatViolation(FormatViolationFinding),
}

/// The finding constructors, carried by every member as `kind`.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum FindingKind {
    MissingRequired,
    FormatViolation,
}

/// A reachable, required cell that is not filled (DESIGN §2.6).
#[derive(SimpleObject)]
pub struct MissingRequiredFinding {
    pub kind: FindingKind,
    /// Which surface: `applicant` or `reviewer` (P.4's fixed pair).
    pub surface: ID,
    pub column_id: ID,
    pub path: Vec<RowSegment>,
}

/// A filled text cell violating the surface's format constraint
/// (DESIGN §2.6): non-admissible, never ill-typed.
#[derive(SimpleObject)]
pub struct FormatViolationFinding {
    pub kind: FindingKind,
    /// Which surface: `applicant` or `reviewer` (P.4's fixed pair).
    pub surface: ID,
    pub column_id: ID,
    pub path: Vec<RowSegment>,
}

/// `updatePreview` input (G.12): an ordered batch, all-or-nothing.
#[derive(InputObject)]
pub struct UpdatePreviewInput {
    /// The procedure whose draft preview is written; the viewer must
    /// administer it.
    pub procedure_id: ID,
    /// Applied in order; the first refused write refuses the batch.
    pub writes: Vec<CellWriteInput>,
}

/// One preview write (`@oneOf`): the kernel's five-op patch set
/// (DESIGN §5/§2.9) — the `updateCells` pilot.
#[derive(OneofObject)]
pub enum CellWriteInput {
    Set(SetCellInput),
    Unset(UnsetCellInput),
    AddItem(AddItemInput),
    RemoveItem(RemoveItemInput),
    Reorder(ReorderItemsInput),
}

/// Writes one cell's state.
#[derive(InputObject)]
pub struct SetCellInput {
    pub column_id: ID,
    /// The row path; omitted = the root scope.
    #[graphql(default)]
    pub path: Vec<RowSegmentInput>,
    pub state: CellStateInput,
}

/// Takes a cell back to absent — distinct from setting it empty
/// (DESIGN §2.4).
#[derive(InputObject)]
pub struct UnsetCellInput {
    pub column_id: ID,
    /// The row path; omitted = the root scope.
    #[graphql(default)]
    pub path: Vec<RowSegmentInput>,
}

/// Adds an item to a `many` group's list. Placement is a **sibling
/// anchor**, never an index (the G.7.3 argument), and the item id is
/// server-minted — read it back off the answered preview.
#[derive(InputObject)]
pub struct AddItemInput {
    pub group_id: ID,
    /// The enclosing scope's row path; omitted = the root scope.
    #[graphql(default)]
    pub parent: Vec<RowSegmentInput>,
    /// The new item lands in front of this one; omitted appends.
    pub before_item_id: Option<ID>,
}

/// Removes an item — and everything under it: its cells and any
/// nested item lists.
#[derive(InputObject)]
pub struct RemoveItemInput {
    pub group_id: ID,
    /// The enclosing scope's row path; omitted = the root scope.
    #[graphql(default)]
    pub parent: Vec<RowSegmentInput>,
    pub item_id: ID,
}

/// Reorders a `many` group's list; `order` must be a permutation of
/// the current items.
#[derive(InputObject)]
pub struct ReorderItemsInput {
    pub group_id: ID,
    /// The enclosing scope's row path; omitted = the root scope.
    #[graphql(default)]
    pub parent: Vec<RowSegmentInput>,
    pub order: Vec<ID>,
}

/// One row-path segment on input.
#[derive(InputObject)]
pub struct RowSegmentInput {
    pub group_id: ID,
    pub item_id: ID,
}

/// A cell state on input (`@oneOf`): written-blank, or one value
/// constructor per scalar kind — the `ColumnTypeInput` idiom.
/// Attachment and geometry values are not writable in the preview
/// (G.12; G.5 Q5 holds the residual). Exact numbers are strings, the
/// output's mirror.
#[derive(OneofObject)]
pub enum CellStateInput {
    /// Written blank (`true`; `false` is `INVALID_INPUT`) — distinct
    /// from `unset`.
    Empty(bool),
    Text(String),
    Boolean(bool),
    /// An exact integer as a decimal string.
    Integer(String),
    /// An exact decimal as a string (`1.5`).
    Decimal(String),
    /// An ISO 8601 date (`2026-08-26`).
    Date(String),
    /// An RFC 3339 instant.
    Datetime(String),
    /// A single selected option.
    Enum(ID),
    /// The selected options of a `multiple` choice column.
    EnumList(Vec<ID>),
}

impl CellWriteInput {
    /// The platform-core write; scalar literals parse here, so a
    /// malformed one is `INVALID_INPUT` before any use case runs
    /// (G.6.4) — a write refused *against the draft* is
    /// `INVALID_WRITE`, downstream.
    pub fn into_write(self) -> async_graphql::Result<platform_core::PreviewWrite> {
        Ok(match self {
            CellWriteInput::Set(set) => platform_core::PreviewWrite::Set {
                column: ColumnId::new(set.column_id.as_str()),
                path: row_path(set.path),
                state: set.state.into_state()?,
            },
            CellWriteInput::Unset(unset) => platform_core::PreviewWrite::Unset {
                column: ColumnId::new(unset.column_id.as_str()),
                path: row_path(unset.path),
            },
            CellWriteInput::AddItem(add) => platform_core::PreviewWrite::AddItem {
                group: GroupId::new(add.group_id.as_str()),
                parent: row_path(add.parent),
                before: add.before_item_id.map(|id| ItemId::new(id.as_str())),
            },
            CellWriteInput::RemoveItem(remove) => platform_core::PreviewWrite::RemoveItem {
                group: GroupId::new(remove.group_id.as_str()),
                parent: row_path(remove.parent),
                item: ItemId::new(remove.item_id.as_str()),
            },
            CellWriteInput::Reorder(reorder) => platform_core::PreviewWrite::Reorder {
                group: GroupId::new(reorder.group_id.as_str()),
                parent: row_path(reorder.parent),
                order: reorder
                    .order
                    .into_iter()
                    .map(|id| ItemId::new(id.as_str()))
                    .collect(),
            },
        })
    }
}

impl CellStateInput {
    fn into_state(self) -> async_graphql::Result<CellState> {
        let one = |scalar| CellState::Value(CellValue::One(scalar));
        Ok(match self {
            CellStateInput::Empty(set) => {
                if !set {
                    return Err(invalid_input("empty must be true"));
                }
                CellState::Empty
            }
            CellStateInput::Text(value) => one(Scalar::Text(value)),
            CellStateInput::Boolean(value) => one(Scalar::Boolean(value)),
            CellStateInput::Integer(text) => match text.parse::<i64>() {
                Ok(i) if i.to_string() == text => one(Scalar::Integer(i)),
                _ => {
                    return Err(invalid_input("integer must be a normalized decimal string"));
                }
            },
            CellStateInput::Decimal(text) => one(Scalar::Decimal(
                Decimal::parse(&text).map_err(|e| invalid_input(format!("decimal: {e}")))?,
            )),
            CellStateInput::Date(text) => one(Scalar::Date(
                Date::parse(&text).map_err(|e| invalid_input(format!("date: {e}")))?,
            )),
            CellStateInput::Datetime(text) => one(Scalar::Datetime(
                Instant::parse(&text).map_err(|e| invalid_input(format!("datetime: {e}")))?,
            )),
            CellStateInput::Enum(id) => one(Scalar::Enum(OptionId::new(id.as_str()))),
            CellStateInput::EnumList(ids) => CellState::Value(CellValue::Many(
                ids.into_iter()
                    .map(|id| Scalar::Enum(OptionId::new(id.as_str())))
                    .collect(),
            )),
        })
    }
}

fn row_path(segments: Vec<RowSegmentInput>) -> RowPath {
    let mut path = RowPath::root();
    for segment in segments {
        path = path.child(PathSeg {
            group: GroupId::new(segment.group_id.as_str()),
            item: ItemId::new(segment.item_id.as_str()),
        });
    }
    path
}

/// The preview of a draft's values and findings, resolved server-side
/// (`platform_core::preview_values` + `preview_findings`).
pub fn preview(values: &RecordValues, findings: Vec<platform_core::PreviewFinding>) -> Preview {
    let mut cells = Vec::with_capacity(values.cells.len());
    for (addr, state) in &values.cells {
        let column_id = ID::from(addr.column.as_str());
        let path = segments(&addr.path);
        cells.push(match state {
            CellState::Empty => Cell::Empty(EmptyCell {
                kind: CellKind::Empty,
                column_id,
                path,
            }),
            CellState::Value(CellValue::One(scalar)) => match scalar {
                Scalar::Text(value) => Cell::Text(TextCell {
                    kind: CellKind::Text,
                    column_id,
                    path,
                    value: value.clone(),
                }),
                Scalar::Boolean(value) => Cell::Boolean(BooleanCell {
                    kind: CellKind::Boolean,
                    column_id,
                    path,
                    value: *value,
                }),
                Scalar::Integer(value) => Cell::Integer(IntegerCell {
                    kind: CellKind::Integer,
                    column_id,
                    path,
                    value: value.to_string(),
                }),
                Scalar::Decimal(value) => Cell::Decimal(DecimalCell {
                    kind: CellKind::Decimal,
                    column_id,
                    path,
                    value: value.to_string(),
                }),
                Scalar::Date(value) => Cell::Date(DateCell {
                    kind: CellKind::Date,
                    column_id,
                    path,
                    value: value.to_string(),
                }),
                Scalar::Datetime(value) => Cell::Datetime(DatetimeCell {
                    kind: CellKind::Datetime,
                    column_id,
                    path,
                    value: value.to_string(),
                }),
                Scalar::Enum(option) => Cell::Enum(EnumCell {
                    kind: CellKind::Enum,
                    column_id,
                    path,
                    option_ids: vec![ID::from(option.as_str())],
                }),
                // Not writable in the preview (G.12) — pruning keeps
                // them out of a conforming bag; skipped, not lied
                // about, should one ever appear.
                Scalar::Attachment(_) | Scalar::Geometry(_) => continue,
            },
            CellState::Value(CellValue::Many(list)) => Cell::Enum(EnumCell {
                kind: CellKind::Enum,
                column_id,
                path,
                // Only choice columns hold many values among the
                // preview-writable kinds (the platform's structural
                // rule, G.7.2); conformance keeps the list
                // homogeneous.
                option_ids: list
                    .iter()
                    .filter_map(Scalar::element_id)
                    .map(ID::from)
                    .collect(),
            }),
        });
    }
    let items = values
        .items
        .iter()
        .map(|(addr, list)| ItemList {
            group_id: ID::from(addr.group.as_str()),
            parent: segments(&addr.parent),
            item_ids: list.iter().map(|item| ID::from(item.as_str())).collect(),
        })
        .collect();
    let findings = findings
        .into_iter()
        .filter_map(|f| {
            let surface = ID::from(f.surface);
            Some(match f.finding {
                Finding::MissingRequired { column, path } => {
                    AdmissibilityFinding::MissingRequired(MissingRequiredFinding {
                        kind: FindingKind::MissingRequired,
                        surface,
                        column_id: ID::from(column.as_str()),
                        path: segments(&path),
                    })
                }
                Finding::FormatViolation { column, path, .. } => {
                    AdmissibilityFinding::FormatViolation(FormatViolationFinding {
                        kind: FindingKind::FormatViolation,
                        surface,
                        column_id: ID::from(column.as_str()),
                        path: segments(&path),
                    })
                }
                // Not evaluated in a preview (no lifecycle, G.12).
                Finding::Ineligible { .. } => return None,
            })
        })
        .collect();
    Preview {
        cells,
        items,
        findings,
    }
}

fn segments(path: &RowPath) -> Vec<RowSegment> {
    path.segments()
        .iter()
        .map(|seg| RowSegment {
            group_id: ID::from(seg.group.as_str()),
            item_id: ID::from(seg.item.as_str()),
        })
        .collect()
}
