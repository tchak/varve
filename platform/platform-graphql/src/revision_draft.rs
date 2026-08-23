//! `RevisionDraft` and the schema it carries (G.6, revision-draft
//! slice): the kernel `Schema` flattened into a list of elements with
//! `parentId`, in document order — no recursive output type (G.2,
//! G.5 Q1), and the tree rebuilds in one pass. Column types are a
//! union from the kernel's `ScalarType` (G.2.5: facts live where they
//! are meaningful — no nullable `unit` on a text column), and on input
//! the mirror `@oneOf` `ColumnTypeInput`, so the validator — not a
//! resolver — keeps a unit off a text column.

use async_graphql::{Enum, ID, InputObject, OneofObject, SimpleObject, Union};
use platform_core::{ElementId, Parent, Placement};
use varve_core::{ColumnId, GroupId, OptionId};
use varve_schema::{
    AttachmentConstraints, Column, Element, Group, NomenclatureRef, OptionRow, ScalarType, Schema,
};

use crate::error::invalid_input;

/// The draft of a procedure's next revision.
#[derive(SimpleObject)]
pub struct RevisionDraft {
    /// The published revision this draft forks from; `null` until the
    /// procedure has one.
    pub base: Option<ID>,
    /// The schema under edit.
    pub schema: DraftSchema,
}

impl RevisionDraft {
    pub fn new(base: Option<&str>, schema: &Schema) -> Self {
        Self {
            base: base.map(ID::from),
            schema: DraftSchema::flatten(schema),
        }
    }
}

/// The schema as a flat list.
#[derive(SimpleObject)]
pub struct DraftSchema {
    /// Every element, **document order** (a group precedes its
    /// children; siblings in their order), each naming its parent.
    pub elements: Vec<SchemaElement>,
}

impl DraftSchema {
    fn flatten(schema: &Schema) -> Self {
        let mut elements = Vec::new();
        push_elements(&mut elements, None, &schema.root);
        Self { elements }
    }
}

fn push_elements(out: &mut Vec<SchemaElement>, parent: Option<&GroupId>, elements: &[Element]) {
    for element in elements {
        let parent_id = parent.map(|g| ID::from(g.as_str()));
        match element {
            Element::Column(c) => out.push(SchemaElement::Column(SchemaColumn {
                id: ID::from(c.id.as_str()),
                parent_id,
                label: c.label.clone(),
                ty: column_type(&c.ty, c.arity),
            })),
            Element::Group(g) => {
                out.push(SchemaElement::Group(SchemaGroup {
                    id: ID::from(g.id.as_str()),
                    parent_id,
                    label: g.label.clone(),
                    cardinality: g.cardinality.into(),
                }));
                push_elements(out, Some(&g.id), &g.children);
            }
        }
    }
}

/// A column or a group.
#[derive(Union)]
pub enum SchemaElement {
    Column(SchemaColumn),
    Group(SchemaGroup),
}

/// A typed field (DESIGN §2.1).
#[derive(SimpleObject)]
pub struct SchemaColumn {
    pub id: ID,
    /// The containing group; `null` at the root.
    pub parent_id: Option<ID>,
    pub label: String,
    #[graphql(name = "type")]
    pub ty: ColumnType,
}

/// An ordered container of elements (DESIGN §2.1).
#[derive(SimpleObject)]
pub struct SchemaGroup {
    pub id: ID,
    /// The containing group; `null` at the root.
    pub parent_id: Option<ID>,
    pub label: String,
    pub cardinality: Cardinality,
}

/// A group holds one row or many (DESIGN §2.2).
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
#[graphql(remote = "varve_schema::Cardinality")]
pub enum Cardinality {
    One,
    Many,
}

/// A number's unit (DESIGN §2.14); plain numbers carry none.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
#[graphql(remote = "varve_schema::Unit")]
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

/// The type constructors, carried by every output union member as
/// `kind` for clients that only need the constructor.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
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

/// A column's type: one object per constructor, carrying only the
/// facts that constructor has — including whether it holds **many
/// values** (`multiple`, the kernel's arity, DESIGN §2.2), which only
/// choices, attachments and geometries offer (platform P.4: in the DN
/// corpus `many` occurs nowhere else), so only those members carry it.
#[derive(Union)]
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
}

#[derive(SimpleObject)]
pub struct TextType {
    pub kind: ColumnTypeKind,
}

#[derive(SimpleObject)]
pub struct BooleanType {
    pub kind: ColumnTypeKind,
}

#[derive(SimpleObject)]
pub struct IntegerType {
    pub kind: ColumnTypeKind,
    pub unit: Option<Unit>,
}

#[derive(SimpleObject)]
pub struct DecimalType {
    pub kind: ColumnTypeKind,
    pub unit: Option<Unit>,
}

#[derive(SimpleObject)]
pub struct DateType {
    pub kind: ColumnTypeKind,
}

#[derive(SimpleObject)]
pub struct DatetimeType {
    pub kind: ColumnTypeKind,
}

/// An enum backed by an inline nomenclature (DESIGN §2.12) — the only
/// backing the editor offers until published nomenclatures have a
/// platform home.
#[derive(SimpleObject)]
pub struct EnumType {
    pub kind: ColumnTypeKind,
    /// Several options may be selected (a multi-select).
    pub multiple: bool,
    pub options: Vec<EnumOption>,
}

/// One option; identity is the `id` (DESIGN §2.11).
#[derive(SimpleObject)]
pub struct EnumOption {
    pub id: ID,
    pub label: String,
}

/// A file column with its representability constraints (DESIGN §2.15).
#[derive(SimpleObject)]
pub struct AttachmentType {
    pub kind: ColumnTypeKind,
    /// Several files (multi-file).
    pub multiple: bool,
    /// IANA media-type patterns (`application/pdf`, `image/*`); empty =
    /// unrestricted.
    pub accept: Vec<String>,
    /// Per-file byte limit; `null` = unlimited.
    pub max_bytes: Option<u64>,
}

#[derive(SimpleObject)]
pub struct GeometryType {
    pub kind: ColumnTypeKind,
    /// Several features (a feature set).
    pub multiple: bool,
}

/// The GraphQL type of a kernel column: its `ScalarType` plus, for the
/// members that carry it, the arity as `multiple`.
pub fn column_type(ty: &ScalarType, arity: varve_schema::Arity) -> ColumnType {
    let multiple = arity == varve_schema::Arity::Many;
    {
        match ty {
            ScalarType::Text => ColumnType::Text(TextType {
                kind: ColumnTypeKind::Text,
            }),
            ScalarType::Boolean => ColumnType::Boolean(BooleanType {
                kind: ColumnTypeKind::Boolean,
            }),
            ScalarType::Integer(unit) => ColumnType::Integer(IntegerType {
                kind: ColumnTypeKind::Integer,
                unit: unit.map(Into::into),
            }),
            ScalarType::Decimal(unit) => ColumnType::Decimal(DecimalType {
                kind: ColumnTypeKind::Decimal,
                unit: unit.map(Into::into),
            }),
            ScalarType::Date => ColumnType::Date(DateType {
                kind: ColumnTypeKind::Date,
            }),
            ScalarType::Datetime => ColumnType::Datetime(DatetimeType {
                kind: ColumnTypeKind::Datetime,
            }),
            ScalarType::Enum(backing) => ColumnType::Enum(EnumType {
                kind: ColumnTypeKind::Enum,
                multiple,
                options: match backing {
                    NomenclatureRef::Inline(rows) => rows
                        .iter()
                        .map(|row| EnumOption {
                            id: ID::from(row.id.as_str()),
                            label: row.label.clone(),
                        })
                        .collect(),
                    // Not producible through this API yet; shown as
                    // an enum with no inline options rather than hidden.
                    NomenclatureRef::Published { .. } => Vec::new(),
                },
            }),
            ScalarType::Attachment(constraints) => ColumnType::Attachment(AttachmentType {
                kind: ColumnTypeKind::Attachment,
                multiple,
                accept: constraints.accept.clone(),
                max_bytes: constraints.max_bytes,
            }),
            ScalarType::Geometry => ColumnType::Geometry(GeometryType {
                kind: ColumnTypeKind::Geometry,
                multiple,
            }),
        }
    }
}

/// A column type on input: `@oneOf`, one member per constructor.
/// Constructors without facts are `Boolean` markers (`{ text: true }`;
/// `false` is `INVALID_INPUT`); the others carry their own input.
#[derive(OneofObject, Debug, Clone)]
pub enum ColumnTypeInput {
    Text(bool),
    Boolean(bool),
    Integer(NumberTypeInput),
    Decimal(NumberTypeInput),
    Date(bool),
    Datetime(bool),
    Enum(EnumTypeInput),
    Attachment(AttachmentTypeInput),
    Geometry(GeometryTypeInput),
}

/// `INTEGER` / `DECIMAL`: an optional unit (DESIGN §2.14).
#[derive(InputObject, Debug, Clone, Default)]
pub struct NumberTypeInput {
    pub unit: Option<Unit>,
}

/// `ENUM`: the inline options (possibly none yet, in a draft), and
/// whether several may be selected.
#[derive(InputObject, Debug, Clone)]
pub struct EnumTypeInput {
    #[graphql(default = false)]
    pub multiple: bool,
    pub options: Vec<EnumOptionInput>,
}

/// An enum option on input. Pass an existing option's `id` to keep
/// its identity across edits; omit it for a new option and the server
/// mints one.
#[derive(InputObject, Debug, Clone)]
pub struct EnumOptionInput {
    pub id: Option<ID>,
    pub label: String,
}

/// `ATTACHMENT`: representability constraints (DESIGN §2.15) and
/// whether several files are accepted.
#[derive(InputObject, Debug, Clone, Default)]
pub struct AttachmentTypeInput {
    #[graphql(default = false)]
    pub multiple: bool,
    /// IANA media-type patterns; omitted or empty = unrestricted.
    pub accept: Option<Vec<String>>,
    /// Per-file byte limit; omitted = unlimited.
    pub max_bytes: Option<u64>,
}

/// `GEOMETRY`: one feature, or a feature set.
#[derive(InputObject, Debug, Clone, Default)]
pub struct GeometryTypeInput {
    #[graphql(default = false)]
    pub multiple: bool,
}

impl ColumnTypeInput {
    /// The kernel type and arity this input names.
    pub fn into_column_type(self) -> async_graphql::Result<(ScalarType, varve_schema::Arity)> {
        let multiple = match &self {
            ColumnTypeInput::Enum(e) => e.multiple,
            ColumnTypeInput::Attachment(a) => a.multiple,
            ColumnTypeInput::Geometry(g) => g.multiple,
            _ => false,
        };
        let arity = if multiple {
            varve_schema::Arity::Many
        } else {
            varve_schema::Arity::One
        };
        Ok((self.into_scalar_type()?, arity))
    }

    fn into_scalar_type(self) -> async_graphql::Result<ScalarType> {
        let marker = |name: &str, set: bool, ty: ScalarType| {
            if set {
                Ok(ty)
            } else {
                Err(invalid_input(format!("{name} must be true")))
            }
        };
        Ok(match self {
            ColumnTypeInput::Text(set) => marker("text", set, ScalarType::Text)?,
            ColumnTypeInput::Boolean(set) => marker("boolean", set, ScalarType::Boolean)?,
            ColumnTypeInput::Date(set) => marker("date", set, ScalarType::Date)?,
            ColumnTypeInput::Datetime(set) => marker("datetime", set, ScalarType::Datetime)?,
            ColumnTypeInput::Geometry(_) => ScalarType::Geometry,
            ColumnTypeInput::Integer(n) => ScalarType::Integer(n.unit.map(Into::into)),
            ColumnTypeInput::Decimal(n) => ScalarType::Decimal(n.unit.map(Into::into)),
            // An enum with no options yet is a legitimate draft state —
            // the editor builds the list option by option; publication
            // is where an empty choice is refused.
            ColumnTypeInput::Enum(e) => {
                let rows = e
                    .options
                    .into_iter()
                    .map(|option| {
                        if option.label.trim().is_empty() {
                            return Err(invalid_input("an option label must not be empty"));
                        }
                        Ok(OptionRow {
                            id: match option.id {
                                Some(id) => OptionId::new(id.as_str()),
                                None => platform_core::new_option_id(),
                            },
                            label: option.label.trim().to_owned(),
                            fields: Vec::new(),
                        })
                    })
                    .collect::<async_graphql::Result<Vec<_>>>()?;
                let mut ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
                ids.sort_unstable();
                if ids.windows(2).any(|w| w[0] == w[1]) {
                    return Err(invalid_input("option ids must be distinct"));
                }
                ScalarType::Enum(NomenclatureRef::Inline(rows))
            }
            ColumnTypeInput::Attachment(a) => ScalarType::Attachment(AttachmentConstraints {
                accept: a.accept.unwrap_or_default(),
                max_bytes: a.max_bytes,
            }),
        })
    }
}

/// Where to put an element: `parentId` names a group (`null` = root),
/// `beforeId` a sibling to insert in front of (`null` = append).
#[derive(InputObject, Debug, Clone, Default)]
pub struct PlacementInput {
    pub parent_id: Option<ID>,
    pub before_id: Option<ID>,
}

impl PlacementInput {
    /// Resolves the anchor against `schema`: `beforeId` must name an
    /// element in the draft (which kind is looked up, since the API
    /// carries one `ID` for both). An unknown anchor is reported by
    /// the edit as [`platform_core::EditError::UnknownElement`].
    pub fn resolve(&self, schema: &Schema) -> Result<Placement, platform_core::EditError> {
        Ok(Placement {
            parent: match &self.parent_id {
                None => Parent::Root,
                Some(id) => Parent::Group(GroupId::new(id.as_str())),
            },
            before: match &self.before_id {
                None => None,
                Some(id) => Some(element_id(schema, id)?),
            },
        })
    }
}

/// The kernel identity behind a draft `ID`: column or group, whichever
/// the draft holds under that string.
pub fn element_id(schema: &Schema, id: &ID) -> Result<ElementId, platform_core::EditError> {
    fn find(elements: &[Element], id: &str) -> Option<ElementId> {
        elements.iter().find_map(|e| match e {
            Element::Column(c) if c.id.as_str() == id => Some(ElementId::Column(c.id.clone())),
            Element::Group(g) if g.id.as_str() == id => Some(ElementId::Group(g.id.clone())),
            Element::Group(g) => find(&g.children, id),
            Element::Column(_) => None,
        })
    }
    find(&schema.root, id.as_str()).ok_or_else(|| {
        platform_core::EditError::UnknownElement(ElementId::Column(ColumnId::new(id.as_str())))
    })
}

/// A new column as `addColumn` builds it.
pub fn new_column(label: String, ty: ScalarType, arity: varve_schema::Arity) -> Element {
    Element::Column(Column {
        id: platform_core::new_column_id(),
        label,
        ty,
        arity,
    })
}

/// A new group as `addGroup` builds it.
pub fn new_group(label: String, cardinality: Cardinality) -> Element {
    Element::Group(Group {
        id: platform_core::new_group_id(),
        label,
        cardinality: cardinality.into(),
        children: Vec::new(),
        included_from: None,
    })
}
