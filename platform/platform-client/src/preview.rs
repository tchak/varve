//! The fillable preview (`design/graphql.md` G.12): the preview
//! page's read — the draft's elements with the scratch value bag and
//! its admissibility findings — and the `updatePreview` batch write,
//! the `updateCells` pilot. Exact numbers travel as strings (the §5
//! wire precedent); row paths are `{group, item}` segment chains.

use crate::revision_draft::{DraftOrganization, Element};
use crate::schema;

/// Variables of [`ProcedurePreviewQuery`].
#[derive(cynic::QueryVariables, Debug)]
pub struct ProcedurePreviewVariables {
    pub id: cynic::Id,
}

/// `query($id: ID!) { procedure(id: $id) { … revisionDraft { elements
/// preview { cells findings } } } }`; `None` for an absent or
/// invisible procedure (G.6).
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "ProcedurePreviewVariables")]
pub struct ProcedurePreviewQuery {
    #[arguments(id: $id)]
    pub procedure: Option<ProcedurePreview>,
}

/// A procedure reduced to what the preview page renders — the shape
/// [`UpdatePreview`] answers with too.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Procedure")]
pub struct ProcedurePreview {
    pub id: cynic::Id,
    pub title: String,
    pub organization: DraftOrganization,
    pub revision_draft: PreviewDraft,
}

/// The draft as the preview reads it: the authored tree plus the
/// value bag.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "RevisionDraft")]
pub struct PreviewDraft {
    /// The authored tree, document order (the form the preview
    /// renders).
    pub elements: Vec<Element>,
    /// Whether a stored working buffer exists (G.7 virtual draft).
    pub in_progress: bool,
    /// The scratch values and what admissibility makes of them.
    pub preview: Preview,
}

/// The scratch value bag (G.12): empty until filled, cleared by
/// discard and by publication.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Preview")]
pub struct Preview {
    pub cells: Vec<Cell>,
    /// Every `many` group's ordered item list — a freshly added
    /// item's server-minted id is read here.
    pub items: Vec<ItemList>,
    pub findings: Vec<AdmissibilityFinding>,
}

/// One `many` group instance's ordered items.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ItemList")]
pub struct ItemList {
    pub group_id: cynic::Id,
    /// The enclosing scope's row path; empty = the root scope.
    pub parent: Vec<RowSegment>,
    /// Document order.
    pub item_ids: Vec<cynic::Id>,
}

/// A written cell: one member per value kind plus written-blank
/// (`EmptyCell` — distinct from absent, DESIGN §2.4).
#[derive(cynic::InlineFragments, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Cell")]
pub enum Cell {
    Text(TextCell),
    Boolean(BooleanCell),
    Integer(IntegerCell),
    Decimal(DecimalCell),
    Date(DateCell),
    Datetime(DatetimeCell),
    Enum(EnumCell),
    Empty(EmptyCell),
    #[cynic(fallback)]
    Unknown,
}

impl Cell {
    /// The cell's address: column id and row path.
    pub fn address(&self) -> Option<(&cynic::Id, &[RowSegment])> {
        Some(match self {
            Cell::Text(c) => (&c.column_id, &c.path),
            Cell::Boolean(c) => (&c.column_id, &c.path),
            Cell::Integer(c) => (&c.column_id, &c.path),
            Cell::Decimal(c) => (&c.column_id, &c.path),
            Cell::Date(c) => (&c.column_id, &c.path),
            Cell::Datetime(c) => (&c.column_id, &c.path),
            Cell::Enum(c) => (&c.column_id, &c.path),
            Cell::Empty(c) => (&c.column_id, &c.path),
            Cell::Unknown => return None,
        })
    }
}

/// One row-path segment: which item of which `many` group.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "RowSegment")]
pub struct RowSegment {
    pub group_id: cynic::Id,
    pub item_id: cynic::Id,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "TextCell")]
pub struct TextCell {
    pub column_id: cynic::Id,
    pub path: Vec<RowSegment>,
    pub value: String,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "BooleanCell")]
pub struct BooleanCell {
    pub column_id: cynic::Id,
    pub path: Vec<RowSegment>,
    pub value: bool,
}

/// An exact integer, as a decimal string.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "IntegerCell")]
pub struct IntegerCell {
    pub column_id: cynic::Id,
    pub path: Vec<RowSegment>,
    pub value: String,
}

/// An exact decimal, as its normalized string.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "DecimalCell")]
pub struct DecimalCell {
    pub column_id: cynic::Id,
    pub path: Vec<RowSegment>,
    pub value: String,
}

/// An ISO 8601 date.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "DateCell")]
pub struct DateCell {
    pub column_id: cynic::Id,
    pub path: Vec<RowSegment>,
    pub value: String,
}

/// An RFC 3339 instant, UTC.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "DatetimeCell")]
pub struct DatetimeCell {
    pub column_id: cynic::Id,
    pub path: Vec<RowSegment>,
    pub value: String,
}

/// A choice cell: the selected option ids (one element on a
/// single-select column).
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "EnumCell")]
pub struct EnumCell {
    pub column_id: cynic::Id,
    pub path: Vec<RowSegment>,
    pub option_ids: Vec<cynic::Id>,
}

/// Written blank — distinct from absent.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "EmptyCell")]
pub struct EmptyCell {
    pub column_id: cynic::Id,
    pub path: Vec<RowSegment>,
}

/// One admissibility finding, tagged with the surface it holds on
/// (`applicant` / `reviewer`).
#[derive(cynic::InlineFragments, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "AdmissibilityFinding")]
pub enum AdmissibilityFinding {
    MissingRequired(MissingRequiredFinding),
    FormatViolation(FormatViolationFinding),
    #[cynic(fallback)]
    Unknown,
}

/// A reachable, required cell that is not filled.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "MissingRequiredFinding")]
pub struct MissingRequiredFinding {
    pub surface: cynic::Id,
    pub column_id: cynic::Id,
    pub path: Vec<RowSegment>,
}

/// A filled text cell violating the surface's format constraint.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "FormatViolationFinding")]
pub struct FormatViolationFinding {
    pub surface: cynic::Id,
    pub column_id: cynic::Id,
    pub path: Vec<RowSegment>,
}

/// Variables of [`UpdatePreview`].
#[derive(cynic::QueryVariables, Debug)]
pub struct UpdatePreviewVariables {
    pub input: UpdatePreviewInput,
}

/// `mutation($input: UpdatePreviewInput!) { updatePreview(input:
/// $input) { … } }` — all-or-nothing; `INVALID_WRITE` refuses the
/// batch with the reason.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "UpdatePreviewVariables")]
pub struct UpdatePreview {
    #[arguments(input: $input)]
    pub update_preview: ProcedurePreview,
}

/// `updatePreview` input: an ordered batch of writes.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct UpdatePreviewInput {
    pub procedure_id: cynic::Id,
    pub writes: Vec<CellWriteInput>,
}

/// One write (`@oneOf`): set exactly one member — use the
/// constructors.
#[derive(cynic::InputObject, Debug, Clone, Default)]
pub struct CellWriteInput {
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub set: Option<SetCellInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub unset: Option<UnsetCellInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub add_item: Option<AddItemInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub remove_item: Option<RemoveItemInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub reorder: Option<ReorderItemsInput>,
}

impl CellWriteInput {
    pub fn set(column_id: cynic::Id, path: Vec<RowSegmentInput>, state: CellStateInput) -> Self {
        Self {
            set: Some(SetCellInput {
                column_id,
                path,
                state,
            }),
            ..Default::default()
        }
    }
    pub fn unset(column_id: cynic::Id, path: Vec<RowSegmentInput>) -> Self {
        Self {
            unset: Some(UnsetCellInput { column_id, path }),
            ..Default::default()
        }
    }
    pub fn add_item(
        group_id: cynic::Id,
        parent: Vec<RowSegmentInput>,
        before_item_id: Option<cynic::Id>,
    ) -> Self {
        Self {
            add_item: Some(AddItemInput {
                group_id,
                parent,
                before_item_id,
            }),
            ..Default::default()
        }
    }
    pub fn remove_item(
        group_id: cynic::Id,
        parent: Vec<RowSegmentInput>,
        item_id: cynic::Id,
    ) -> Self {
        Self {
            remove_item: Some(RemoveItemInput {
                group_id,
                parent,
                item_id,
            }),
            ..Default::default()
        }
    }
    pub fn reorder(
        group_id: cynic::Id,
        parent: Vec<RowSegmentInput>,
        order: Vec<cynic::Id>,
    ) -> Self {
        Self {
            reorder: Some(ReorderItemsInput {
                group_id,
                parent,
                order,
            }),
            ..Default::default()
        }
    }
}

/// Writes one cell's state.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct SetCellInput {
    pub column_id: cynic::Id,
    pub path: Vec<RowSegmentInput>,
    pub state: CellStateInput,
}

/// Takes a cell back to absent — distinct from setting it empty.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct UnsetCellInput {
    pub column_id: cynic::Id,
    pub path: Vec<RowSegmentInput>,
}

/// Adds an item to a `many` group's list, in front of
/// `before_item_id` (omitted appends); the id is server-minted.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct AddItemInput {
    pub group_id: cynic::Id,
    pub parent: Vec<RowSegmentInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub before_item_id: Option<cynic::Id>,
}

/// Removes an item and everything under it.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct RemoveItemInput {
    pub group_id: cynic::Id,
    pub parent: Vec<RowSegmentInput>,
    pub item_id: cynic::Id,
}

/// Reorders a `many` group's list; `order` must be a permutation.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct ReorderItemsInput {
    pub group_id: cynic::Id,
    pub parent: Vec<RowSegmentInput>,
    pub order: Vec<cynic::Id>,
}

/// One row-path segment on input.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct RowSegmentInput {
    pub group_id: cynic::Id,
    pub item_id: cynic::Id,
}

/// A cell state (`@oneOf`): set exactly one member — use the
/// constructors.
#[derive(cynic::InputObject, Debug, Clone, Default)]
pub struct CellStateInput {
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub empty: Option<bool>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub boolean: Option<bool>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub integer: Option<String>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub decimal: Option<String>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<String>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    #[cynic(rename = "enum")]
    pub enum_: Option<cynic::Id>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub enum_list: Option<Vec<cynic::Id>>,
}

impl CellStateInput {
    /// Written blank — distinct from [`CellWriteInput::unset`].
    pub fn empty() -> Self {
        Self {
            empty: Some(true),
            ..Default::default()
        }
    }
    pub fn text(value: impl Into<String>) -> Self {
        Self {
            text: Some(value.into()),
            ..Default::default()
        }
    }
    pub fn boolean(value: bool) -> Self {
        Self {
            boolean: Some(value),
            ..Default::default()
        }
    }
    /// An exact integer as a decimal string.
    pub fn integer(value: impl Into<String>) -> Self {
        Self {
            integer: Some(value.into()),
            ..Default::default()
        }
    }
    /// An exact decimal as a string (`1.5`).
    pub fn decimal(value: impl Into<String>) -> Self {
        Self {
            decimal: Some(value.into()),
            ..Default::default()
        }
    }
    /// An ISO 8601 date (`2026-08-26`).
    pub fn date(value: impl Into<String>) -> Self {
        Self {
            date: Some(value.into()),
            ..Default::default()
        }
    }
    /// An RFC 3339 instant.
    pub fn datetime(value: impl Into<String>) -> Self {
        Self {
            datetime: Some(value.into()),
            ..Default::default()
        }
    }
    /// A single selected option.
    pub fn enum_option(id: cynic::Id) -> Self {
        Self {
            enum_: Some(id),
            ..Default::default()
        }
    }
    /// The selected options of a `multiple` choice column.
    pub fn enum_options(ids: Vec<cynic::Id>) -> Self {
        Self {
            enum_list: Some(ids),
            ..Default::default()
        }
    }
}
