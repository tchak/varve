//! A procedure's revision draft: the schema editor's read and its
//! element mutations (`design/graphql.md` G.6, revision-draft slice).
//! Every mutation answers with the procedure's draft as stored, so an
//! editor needs one round-trip per edit.

use crate::schema;

/// Variables of [`ProcedureRevisionDraftQuery`].
#[derive(cynic::QueryVariables, Debug)]
pub struct ProcedureRevisionDraftVariables {
    pub id: cynic::Id,
}

/// `query($id: ID!) { procedure(id: $id) { id title revisionDraft { … } } }`;
/// `None` for an absent or invisible procedure (G.6).
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "ProcedureRevisionDraftVariables")]
pub struct ProcedureRevisionDraftQuery {
    #[arguments(id: $id)]
    pub procedure: Option<ProcedureRevisionDraft>,
}

/// A procedure reduced to its identity and its revision draft — the
/// shape every draft mutation answers with too.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "Procedure")]
pub struct ProcedureRevisionDraft {
    pub id: cynic::Id,
    pub title: String,
    /// `None` when no draft is in progress.
    pub revision_draft: Option<RevisionDraft>,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "RevisionDraft")]
pub struct RevisionDraft {
    pub base: Option<cynic::Id>,
    pub schema: DraftSchema,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "DraftSchema")]
pub struct DraftSchema {
    /// Document order; each element names its parent group.
    pub elements: Vec<SchemaElement>,
}

#[derive(cynic::InlineFragments, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "SchemaElement")]
pub enum SchemaElement {
    Column(SchemaColumn),
    Group(SchemaGroup),
    #[cynic(fallback)]
    Unknown,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "SchemaColumn")]
pub struct SchemaColumn {
    pub id: cynic::Id,
    pub parent_id: Option<cynic::Id>,
    pub label: String,
    #[cynic(rename = "type")]
    pub ty: ColumnType,
    pub arity: Arity,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "SchemaGroup")]
pub struct SchemaGroup {
    pub id: cynic::Id,
    pub parent_id: Option<cynic::Id>,
    pub label: String,
    pub cardinality: Cardinality,
}

#[derive(cynic::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[cynic(graphql_type = "Arity")]
pub enum Arity {
    One,
    Many,
}

#[derive(cynic::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[cynic(graphql_type = "Cardinality")]
pub enum Cardinality {
    One,
    Many,
}

#[derive(cynic::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[cynic(graphql_type = "Unit")]
pub enum Unit {
    Millimetre,
    Centimetre,
    Metre,
    Kilometre,
    Gram,
    Kilogram,
    Tonne,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
    SquareMetre,
    Hectare,
    SquareKilometre,
    Litre,
    CubicMetre,
    Percent,
}

#[derive(cynic::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[cynic(graphql_type = "ColumnTypeKind")]
pub enum ColumnTypeKind {
    Text,
    Boolean,
    Integer,
    Decimal,
    Date,
    Datetime,
    Enum,
    Attachment,
    Geometry,
}

/// A column's type, one member per constructor.
#[derive(cynic::InlineFragments, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "ColumnType")]
pub enum ColumnType {
    Text(TextType),
    Boolean(BooleanType),
    Integer(IntegerType),
    Decimal(DecimalType),
    Date(DateType),
    Datetime(DatetimeType),
    Enum(EnumType),
    Attachment(AttachmentType),
    Geometry(GeometryType),
    #[cynic(fallback)]
    Unknown,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "TextType")]
pub struct TextType {
    pub kind: ColumnTypeKind,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "BooleanType")]
pub struct BooleanType {
    pub kind: ColumnTypeKind,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "IntegerType")]
pub struct IntegerType {
    pub kind: ColumnTypeKind,
    pub unit: Option<Unit>,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "DecimalType")]
pub struct DecimalType {
    pub kind: ColumnTypeKind,
    pub unit: Option<Unit>,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "DateType")]
pub struct DateType {
    pub kind: ColumnTypeKind,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "DatetimeType")]
pub struct DatetimeType {
    pub kind: ColumnTypeKind,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "EnumType")]
pub struct EnumType {
    pub kind: ColumnTypeKind,
    pub options: Vec<EnumOption>,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "EnumOption")]
pub struct EnumOption {
    pub id: cynic::Id,
    pub label: String,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "AttachmentType")]
pub struct AttachmentType {
    pub kind: ColumnTypeKind,
    pub accept: Vec<String>,
    pub max_bytes: Option<i32>,
}

#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "GeometryType")]
pub struct GeometryType {
    pub kind: ColumnTypeKind,
}

/// A column type on input (`@oneOf`): set exactly one member. Marker
/// constructors are `Some(true)`.
#[derive(cynic::InputObject, Debug, Clone, Default)]
pub struct ColumnTypeInput {
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub text: Option<bool>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub boolean: Option<bool>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub integer: Option<NumberTypeInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub decimal: Option<NumberTypeInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub date: Option<bool>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub datetime: Option<bool>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    #[cynic(rename = "enum")]
    pub enum_: Option<EnumTypeInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<AttachmentTypeInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub geometry: Option<bool>,
}

impl ColumnTypeInput {
    pub fn text() -> Self {
        Self {
            text: Some(true),
            ..Default::default()
        }
    }
    pub fn boolean() -> Self {
        Self {
            boolean: Some(true),
            ..Default::default()
        }
    }
    pub fn integer(unit: Option<Unit>) -> Self {
        Self {
            integer: Some(NumberTypeInput { unit }),
            ..Default::default()
        }
    }
    pub fn decimal(unit: Option<Unit>) -> Self {
        Self {
            decimal: Some(NumberTypeInput { unit }),
            ..Default::default()
        }
    }
    pub fn date() -> Self {
        Self {
            date: Some(true),
            ..Default::default()
        }
    }
    pub fn datetime() -> Self {
        Self {
            datetime: Some(true),
            ..Default::default()
        }
    }
    pub fn enumeration(options: Vec<EnumOptionInput>) -> Self {
        Self {
            enum_: Some(EnumTypeInput { options }),
            ..Default::default()
        }
    }
    pub fn attachment(accept: Vec<String>, max_bytes: Option<i32>) -> Self {
        Self {
            attachment: Some(AttachmentTypeInput {
                accept: Some(accept),
                max_bytes,
            }),
            ..Default::default()
        }
    }
    pub fn geometry() -> Self {
        Self {
            geometry: Some(true),
            ..Default::default()
        }
    }
}

#[derive(cynic::InputObject, Debug, Clone, Default)]
pub struct NumberTypeInput {
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub unit: Option<Unit>,
}

#[derive(cynic::InputObject, Debug, Clone)]
pub struct EnumTypeInput {
    pub options: Vec<EnumOptionInput>,
}

/// An enum option on input; keep an existing option's `id`, omit it
/// for a new one.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct EnumOptionInput {
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub id: Option<cynic::Id>,
    pub label: String,
}

#[derive(cynic::InputObject, Debug, Clone, Default)]
pub struct AttachmentTypeInput {
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub accept: Option<Vec<String>>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<i32>,
}

/// Where an element goes: `parent_id` a group (`None` = root),
/// `before_id` a sibling to insert in front of (`None` = append).
#[derive(cynic::InputObject, Debug, Clone, Default)]
pub struct PlacementInput {
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<cynic::Id>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub before_id: Option<cynic::Id>,
}

#[derive(cynic::InputObject, Debug, Clone)]
pub struct AddColumnInput {
    pub procedure_id: cynic::Id,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub placement: Option<PlacementInput>,
    pub label: String,
    #[cynic(rename = "type")]
    pub ty: ColumnTypeInput,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub arity: Option<Arity>,
}

#[derive(cynic::InputObject, Debug, Clone)]
pub struct AddGroupInput {
    pub procedure_id: cynic::Id,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub placement: Option<PlacementInput>,
    pub label: String,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub cardinality: Option<Cardinality>,
}

/// An omitted field is left as it is.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct UpdateColumnInput {
    pub procedure_id: cynic::Id,
    pub id: cynic::Id,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[cynic(rename = "type", skip_serializing_if = "Option::is_none")]
    pub ty: Option<ColumnTypeInput>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub arity: Option<Arity>,
}

/// An omitted field is left as it is.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct UpdateGroupInput {
    pub procedure_id: cynic::Id,
    pub id: cynic::Id,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    pub cardinality: Option<Cardinality>,
}

#[derive(cynic::InputObject, Debug, Clone)]
pub struct MoveElementInput {
    pub procedure_id: cynic::Id,
    pub id: cynic::Id,
    pub placement: PlacementInput,
}

#[derive(cynic::InputObject, Debug, Clone)]
pub struct RemoveElementInput {
    pub procedure_id: cynic::Id,
    pub id: cynic::Id,
}

#[derive(cynic::InputObject, Debug, Clone)]
pub struct DiscardRevisionDraftInput {
    pub procedure_id: cynic::Id,
}

// Each mutation: `mutation($input: <Input>!) { <field>(input: $input) { …draft } }`.
// Errors: `FORBIDDEN` (procedure absent or not administered),
// `INVALID_INPUT`, `INVALID_EDIT` (the draft or the kernel refused the
// edit), `CONFLICT` (the draft changed underneath; re-read and retry).

/// Variables of [`AddColumn`].
#[derive(cynic::QueryVariables, Debug)]
pub struct AddColumnVariables {
    pub input: AddColumnInput,
}

/// `addColumn`: the new column is the element before
/// `placement.before_id` in its parent, or the parent's last child.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "AddColumnVariables")]
pub struct AddColumn {
    #[arguments(input: $input)]
    pub add_column: ProcedureRevisionDraft,
}

/// Variables of [`AddGroup`].
#[derive(cynic::QueryVariables, Debug)]
pub struct AddGroupVariables {
    pub input: AddGroupInput,
}

/// `addGroup`: placed like `addColumn`.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "AddGroupVariables")]
pub struct AddGroup {
    #[arguments(input: $input)]
    pub add_group: ProcedureRevisionDraft,
}

/// Variables of [`UpdateColumn`].
#[derive(cynic::QueryVariables, Debug)]
pub struct UpdateColumnVariables {
    pub input: UpdateColumnInput,
}

/// `updateColumn`: label, type, or arity; the id never changes.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "UpdateColumnVariables")]
pub struct UpdateColumn {
    #[arguments(input: $input)]
    pub update_column: ProcedureRevisionDraft,
}

/// Variables of [`UpdateGroup`].
#[derive(cynic::QueryVariables, Debug)]
pub struct UpdateGroupVariables {
    pub input: UpdateGroupInput,
}

/// `updateGroup`: label or cardinality.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "UpdateGroupVariables")]
pub struct UpdateGroup {
    #[arguments(input: $input)]
    pub update_group: ProcedureRevisionDraft,
}

/// Variables of [`MoveElement`].
#[derive(cynic::QueryVariables, Debug)]
pub struct MoveElementVariables {
    pub input: MoveElementInput,
}

/// `moveElement`: a column or a group with its subtree.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "MoveElementVariables")]
pub struct MoveElement {
    #[arguments(input: $input)]
    pub move_element: ProcedureRevisionDraft,
}

/// Variables of [`RemoveElement`].
#[derive(cynic::QueryVariables, Debug)]
pub struct RemoveElementVariables {
    pub input: RemoveElementInput,
}

/// `removeElement`: a column or a group with its subtree.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "RemoveElementVariables")]
pub struct RemoveElement {
    #[arguments(input: $input)]
    pub remove_element: ProcedureRevisionDraft,
}

/// Variables of [`DiscardRevisionDraft`].
#[derive(cynic::QueryVariables, Debug)]
pub struct DiscardRevisionDraftVariables {
    pub input: DiscardRevisionDraftInput,
}

/// `discardRevisionDraft`: `revision_draft` is `None` afterwards.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "DiscardRevisionDraftVariables")]
pub struct DiscardRevisionDraft {
    #[arguments(input: $input)]
    pub discard_revision_draft: ProcedureRevisionDraft,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cynic::{MutationBuilder, QueryBuilder};

    #[test]
    fn builds_the_draft_read_and_an_edit() {
        let read = ProcedureRevisionDraftQuery::build(ProcedureRevisionDraftVariables {
            id: cynic::Id::new("abc"),
        });
        let document = serde_json::to_value(&read).unwrap();
        let query = document["query"].as_str().unwrap();
        assert!(query.contains("revisionDraft"), "{query}");
        assert!(query.contains("... on SchemaColumn"), "{query}");
        assert!(query.contains("... on IntegerType"), "{query}");

        let add = AddColumn::build(AddColumnVariables {
            input: AddColumnInput {
                procedure_id: cynic::Id::new("abc"),
                placement: None,
                label: "Nom".into(),
                ty: ColumnTypeInput::integer(Some(Unit::SquareMetre)),
                arity: None,
            },
        });
        let document = serde_json::to_value(&add).unwrap();
        assert!(
            document["query"]
                .as_str()
                .unwrap()
                .contains("addColumn(input: $input)")
        );
        let input = &document["variables"]["input"];
        assert_eq!(
            input["type"],
            serde_json::json!({ "integer": { "unit": "SQUARE_METRE" } })
        );
        assert!(input.get("placement").is_none());
    }
}
