//! `RevisionDraft` and the authored tree it carries (G.7, amended
//! 2026-08-24): the tree flattened into a list of elements with
//! `parentId`, in document order — no recursive output type (G.2,
//! G.5 Q1), and the tree rebuilds in one pass. Four element kinds —
//! column, group, section, note — each carrying its `audience`
//! (platform P.4: `REVIEWER` is DN's private; the effective audience
//! is the narrowest along the ancestor path). Column types are a
//! union from the kernel's `ScalarType` (G.2.5: facts live where
//! they are meaningful — no nullable `unit` on a text column), and on
//! input the mirror `@oneOf` `ColumnTypeInput`, so the validator —
//! not a resolver — keeps a unit off a text column.

use async_graphql::{Enum, ID, InputObject, OneofObject, SimpleObject, Union};
use platform_core::{
    ElementId, Parent, Placement, Tree, TreeColumn, TreeElement, TreeGroup, TreeNote, TreeSection,
};
use varve_core::OptionId;
use varve_schema::{AttachmentConstraints, NomenclatureRef, OptionRow, ScalarType};
use varve_surface::Format;

use crate::error::{internal, invalid_input};

/// The draft of a procedure's next revision — *head until touched*
/// (G.7 virtual draft): always the tree the next edit operates on,
/// whether or not a working buffer is stored; `inProgress` tells
/// which.
#[derive(SimpleObject)]
#[graphql(complex)]
pub struct RevisionDraft {
    /// The published revision this draft forks from (or would fork
    /// from — the head, until touched); `null` until the procedure
    /// has one.
    pub base: Option<ID>,
    /// Every element of the authored tree, **document order** (a
    /// container precedes its children; siblings in their order),
    /// each naming its parent.
    pub elements: Vec<Element>,
    /// Whether a stored working buffer exists — unpublished work is
    /// in progress. Not inferable from an empty `report`: edits that
    /// never touch the derived schema (a label rename, a note) still
    /// store a draft. `publishRevision` refuses a pristine draft
    /// (`INVALID_DRAFT`); `discardRevisionDraft` on one is a no-op.
    pub in_progress: bool,
    /// The authored tree, kept for [`Self::report`] — the kernel
    /// schema is derived there, only when the report is selected.
    #[graphql(skip)]
    tree: Tree,
}

impl RevisionDraft {
    pub fn new(base: Option<&str>, tree: Tree, in_progress: bool) -> Self {
        let mut elements = Vec::new();
        push_elements(&mut elements, None, &tree.elements);
        Self {
            base: base.map(ID::from),
            elements,
            in_progress,
            tree,
        }
    }
}

#[async_graphql::ComplexObject]
impl RevisionDraft {
    /// The report `publishRevision` would return, computed at read
    /// time against the draft's base (G.10 *RevisionDraft.report*):
    /// impact is visible while editing, without attempting a
    /// publication. A base-less draft classifies against the empty
    /// schema — every column `ADDED`, `SAFE`.
    async fn report(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<crate::impact::ImpactReport> {
        use varve_store::RevisionStore;
        let (_, mut db) = crate::session(ctx)?;
        let base = match &self.base {
            Some(id) => {
                let shared: platform_core::SharedExecutor =
                    tokio::sync::Mutex::new(&mut db as &mut dyn toasty::Executor);
                let store = platform_store::PlatformStore::new(&shared);
                Some(
                    store
                        .schema(&varve_core::RevisionId::new(id.as_str()))
                        .await
                        .map_err(internal)?
                        .ok_or_else(|| internal("draft's base revision is not in the store"))?,
                )
            }
            None => None,
        };
        let report =
            platform_core::draft_report(base.as_ref(), &self.tree.schema()).map_err(internal)?;
        Ok(crate::impact::ImpactReport::from(&report))
    }
}

fn push_elements(out: &mut Vec<Element>, parent: Option<&ID>, elements: &[TreeElement]) {
    for element in elements {
        let parent_id = parent.cloned();
        match element {
            TreeElement::Column(c) => out.push(Element::Column(Column {
                id: ID::from(c.id.as_str()),
                parent_id,
                label: c.label.clone(),
                ty: column_type(&c.ty, c.arity, c.format.as_ref()),
                required: c.required,
                audience: c.audience.into(),
            })),
            TreeElement::Group(g) => {
                let id = ID::from(g.id.as_str());
                out.push(Element::Group(Group {
                    id: id.clone(),
                    parent_id,
                    label: g.label.clone(),
                    cardinality: g.cardinality.into(),
                    audience: g.audience.into(),
                }));
                push_elements(out, Some(&id), &g.children);
            }
            TreeElement::Section(s) => {
                let id = ID::from(s.id.as_str());
                out.push(Element::Section(Section {
                    id: id.clone(),
                    parent_id,
                    title: s.title.clone(),
                    help: s.help.clone(),
                    audience: s.audience.into(),
                }));
                push_elements(out, Some(&id), &s.children);
            }
            TreeElement::Note(n) => out.push(Element::Note(Note {
                id: ID::from(n.id.as_str()),
                parent_id,
                title: n.title.clone(),
                body: n.body.clone(),
                audience: n.audience.into(),
            })),
        }
    }
}

/// A column, a group, a section or a note.
#[derive(Union)]
pub enum Element {
    Column(Column),
    Group(Group),
    Section(Section),
    Note(Note),
}

/// Who sees an element (platform P.4): `REVIEWER` is DN's private.
/// The *effective* audience of an element is the narrowest along its
/// ancestor path — an `ALL` element inside a `REVIEWER` section is
/// reviewer-only.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
#[graphql(remote = "platform_core::Audience")]
pub enum Audience {
    /// Every surface: the applicant form and the reviewer screen.
    All,
    /// The reviewer surface only.
    Reviewer,
}

/// A typed field (DESIGN §2.1).
#[derive(SimpleObject)]
pub struct Column {
    pub id: ID,
    /// The containing group or section; `null` at the root.
    pub parent_id: Option<ID>,
    pub label: String,
    #[graphql(name = "type")]
    pub ty: ColumnType,
    /// §2.6 requiredness, its two constant cases (G.7 *Required on
    /// columns*); conditional rules arrive with the rule editor.
    pub required: bool,
    pub audience: Audience,
}

/// An ordered container of elements (DESIGN §2.1).
#[derive(SimpleObject)]
pub struct Group {
    pub id: ID,
    /// The containing group or section; `null` at the root.
    pub parent_id: Option<ID>,
    pub label: String,
    pub cardinality: Cardinality,
    pub audience: Audience,
}

/// A header section: presentation, may contain elements (DESIGN
/// §2.6). Its id is a kernel `NodeId` (§2.6, surface node identity).
#[derive(SimpleObject)]
pub struct Section {
    pub id: ID,
    /// The containing group or section; `null` at the root.
    pub parent_id: Option<ID>,
    pub title: String,
    /// Help text under the title; `null` when none.
    pub help: Option<String>,
    pub audience: Audience,
}

/// An explication: prose, no data. A `REVIEWER` note is authored
/// guidance for instructors.
#[derive(SimpleObject)]
pub struct Note {
    pub id: ID,
    /// The containing group or section; `null` at the root.
    pub parent_id: Option<ID>,
    /// Heading; `null` when none.
    pub title: Option<String>,
    pub body: String,
    pub audience: Audience,
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
    /// The §2.6 format constraint; `null` = unconstrained.
    pub format: Option<TextFormat>,
}

/// A text column's format constraint (DESIGN §2.6): admissibility
/// over text, checked per surface — never a type.
#[derive(Union)]
pub enum TextFormat {
    Email(EmailFormat),
    Phone(PhoneFormat),
    Iban(IbanFormat),
    Regex(RegexFormat),
}

/// The format constructors, carried by every member as `kind`.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum TextFormatKind {
    Email,
    Phone,
    Iban,
    Regex,
}

#[derive(SimpleObject)]
pub struct EmailFormat {
    pub kind: TextFormatKind,
}

#[derive(SimpleObject)]
pub struct PhoneFormat {
    pub kind: TextFormatKind,
}

#[derive(SimpleObject)]
pub struct IbanFormat {
    pub kind: TextFormatKind,
}

/// An author-supplied pattern, run full-match on the linear-time
/// engine (no backtracking — DESIGN §2.6).
#[derive(SimpleObject)]
pub struct RegexFormat {
    pub kind: TextFormatKind,
    pub pattern: String,
}

fn text_format(format: &Format) -> TextFormat {
    match format {
        Format::Email => TextFormat::Email(EmailFormat {
            kind: TextFormatKind::Email,
        }),
        Format::Phone => TextFormat::Phone(PhoneFormat {
            kind: TextFormatKind::Phone,
        }),
        Format::Iban => TextFormat::Iban(IbanFormat {
            kind: TextFormatKind::Iban,
        }),
        Format::Regex(pattern) => TextFormat::Regex(RegexFormat {
            kind: TextFormatKind::Regex,
            pattern: pattern.clone(),
        }),
    }
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
pub fn column_type(
    ty: &ScalarType,
    arity: varve_schema::Arity,
    format: Option<&Format>,
) -> ColumnType {
    let multiple = arity == varve_schema::Arity::Many;
    match ty {
        ScalarType::Text => ColumnType::Text(TextType {
            kind: ColumnTypeKind::Text,
            format: format.map(text_format),
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

/// A column type on input: `@oneOf`, one member per constructor.
/// Constructors without facts are `Boolean` markers (`{ text: true }`;
/// `false` is `INVALID_INPUT`); the others carry their own input.
#[derive(OneofObject, Debug, Clone)]
pub enum ColumnTypeInput {
    Text(TextTypeInput),
    Boolean(bool),
    Integer(NumberTypeInput),
    Decimal(NumberTypeInput),
    Date(bool),
    Datetime(bool),
    Enum(EnumTypeInput),
    Attachment(AttachmentTypeInput),
    Geometry(GeometryTypeInput),
}

/// `TEXT`: the optional §2.6 format constraint.
#[derive(InputObject, Debug, Clone, Default)]
pub struct TextTypeInput {
    pub format: Option<TextFormatInput>,
}

/// A format on input (`@oneOf`): the built-ins are `Boolean` markers
/// (`false` is `INVALID_INPUT`), the custom pattern its own input.
#[derive(OneofObject, Debug, Clone)]
pub enum TextFormatInput {
    Email(bool),
    Phone(bool),
    Iban(bool),
    Regex(RegexFormatInput),
}

/// `REGEX`: the pattern, verified on the linear-time engine at edit
/// time.
#[derive(InputObject, Debug, Clone)]
pub struct RegexFormatInput {
    pub pattern: String,
}

impl TextFormatInput {
    fn into_format(self) -> async_graphql::Result<Format> {
        let marker = |name: &str, set: bool, format: Format| {
            if set {
                Ok(format)
            } else {
                Err(invalid_input(format!("{name} must be true")))
            }
        };
        Ok(match self {
            TextFormatInput::Email(set) => marker("email", set, Format::Email)?,
            TextFormatInput::Phone(set) => marker("phone", set, Format::Phone)?,
            TextFormatInput::Iban(set) => marker("iban", set, Format::Iban)?,
            TextFormatInput::Regex(regex) => Format::Regex(regex.pattern),
        })
    }
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
    /// The kernel type, arity and format constraint this input names
    /// (the format rides the `TEXT` constructor on the wire but sits
    /// beside the type in the tree — §2.6).
    pub fn into_column_type(
        self,
    ) -> async_graphql::Result<(ScalarType, varve_schema::Arity, Option<Format>)> {
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
        let format = match &self {
            ColumnTypeInput::Text(text) => text
                .format
                .clone()
                .map(TextFormatInput::into_format)
                .transpose()?,
            _ => None,
        };
        Ok((self.into_scalar_type()?, arity, format))
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
            ColumnTypeInput::Text(_) => ScalarType::Text,
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

/// Where to put an element: `parentId` names a group or a section
/// (`null` = root), `beforeId` a sibling to insert in front of
/// (`null` = append).
#[derive(InputObject, Debug, Clone, Default)]
pub struct PlacementInput {
    pub parent_id: Option<ID>,
    pub before_id: Option<ID>,
}

impl PlacementInput {
    /// Resolves parent and anchor against `tree`: both must name
    /// elements in the draft (which kind is looked up, since the API
    /// carries one `ID` for all four). An unknown id is
    /// [`platform_core::EditError::UnknownId`]; a parent that exists
    /// but is not a group or section is
    /// [`platform_core::EditError::ParentNotContainer`].
    pub fn resolve(&self, tree: &Tree) -> Result<Placement, platform_core::EditError> {
        Ok(Placement {
            parent: match &self.parent_id {
                None => Parent::Root,
                Some(id) => match element_id(tree, id)? {
                    ElementId::Group(group) => Parent::Group(group),
                    ElementId::Section(section) => Parent::Section(section),
                    other => return Err(platform_core::EditError::ParentNotContainer(other)),
                },
            },
            before: match &self.before_id {
                None => None,
                Some(id) => Some(element_id(tree, id)?),
            },
        })
    }
}

/// The kernel identity behind a draft `ID`: whichever kind the draft
/// holds under that string; a miss is
/// [`platform_core::EditError::UnknownId`].
pub fn element_id(tree: &Tree, id: &ID) -> Result<ElementId, platform_core::EditError> {
    fn find(elements: &[TreeElement], id: &str) -> Option<ElementId> {
        elements.iter().find_map(|e| match e {
            TreeElement::Column(c) if c.id.as_str() == id => Some(ElementId::Column(c.id.clone())),
            TreeElement::Group(g) if g.id.as_str() == id => Some(ElementId::Group(g.id.clone())),
            TreeElement::Section(s) if s.id.as_str() == id => {
                Some(ElementId::Section(s.id.clone()))
            }
            TreeElement::Note(n) if n.id.as_str() == id => Some(ElementId::Note(n.id.clone())),
            TreeElement::Group(g) => find(&g.children, id),
            TreeElement::Section(s) => find(&s.children, id),
            _ => None,
        })
    }
    find(&tree.elements, id.as_str())
        .ok_or_else(|| platform_core::EditError::UnknownId(id.as_str().to_owned()))
}

/// A new column as `addColumn` builds it.
pub fn new_column(
    label: String,
    ty: ScalarType,
    arity: varve_schema::Arity,
    format: Option<Format>,
    required: bool,
    audience: platform_core::Audience,
) -> TreeElement {
    TreeElement::Column(TreeColumn {
        id: platform_core::new_column_id(),
        label,
        ty,
        arity,
        format,
        required,
        audience,
    })
}

/// A new group as `addGroup` builds it.
pub fn new_group(
    label: String,
    cardinality: Cardinality,
    audience: platform_core::Audience,
) -> TreeElement {
    TreeElement::Group(TreeGroup {
        id: platform_core::new_group_id(),
        label,
        cardinality: cardinality.into(),
        audience,
        children: Vec::new(),
    })
}

/// A new section as `addSection` builds it.
pub fn new_section(
    title: String,
    help: Option<String>,
    audience: platform_core::Audience,
) -> TreeElement {
    TreeElement::Section(TreeSection {
        id: platform_core::new_node_id(),
        title,
        help,
        audience,
        children: Vec::new(),
    })
}

/// A new note as `addNote` builds it.
pub fn new_note(
    title: Option<String>,
    body: String,
    audience: platform_core::Audience,
) -> TreeElement {
    TreeElement::Note(TreeNote {
        id: platform_core::new_node_id(),
        title,
        body,
        audience,
    })
}
