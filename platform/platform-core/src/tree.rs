//! The **authored tree** (design/platform.md P.4, *The authored tree
//! is the draft's single source*): the one tree the editor edits —
//! columns and groups interleaved with sections and notes, each
//! element carrying its audience. The draft stores this tree;
//! publication *derives* everything from it: the kernel [`Schema`]
//! (via [`Tree::schema`]), then the compiled surfaces (pruned per
//! audience) against the published revision.
//!
//! Stored as platform-owned JSON ([`Tree::to_bytes`] /
//! [`Tree::from_bytes`]): the tree holds audiences and presentation
//! nodes, which no kernel object carries, so the kernel's wire canon
//! cannot hold it. The guarantee is "published is deterministically
//! derived from stored" (P.4), not "stored = hashed".

use serde_json::{Value, json};
use varve_core::{ColumnId, GroupId, NodeId, OptionId};
use varve_schema::{
    Arity, AttachmentConstraints, Cardinality, Column, Element, Group, NomenclatureRef, OptionRow,
    ScalarType, Schema, Unit,
};
use varve_surface::Format;

/// Who sees an element (platform P.4, amended 2026-08-24): `Reviewer`
/// is DN's *annotation privée*. The effective audience of an element
/// is the **narrowest along its ancestor path** — moving an element
/// into a reviewer-only section narrows it without rewriting markers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Audience {
    /// Every surface: the applicant form and the reviewer screen.
    #[default]
    All,
    /// The reviewer surface only.
    Reviewer,
}

impl Audience {
    /// The narrower of the two.
    pub fn narrowest(self, other: Audience) -> Audience {
        match (self, other) {
            (Audience::All, Audience::All) => Audience::All,
            _ => Audience::Reviewer,
        }
    }

    /// Strictly wider than `other` (`all` is wider than `reviewer`).
    pub fn wider_than(self, other: Audience) -> bool {
        self == Audience::All && other == Audience::Reviewer
    }
}

/// The authored tree: what a [`crate::procedure::RevisionDraft`]
/// holds, and the single source the schema and the surfaces are
/// derived from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tree {
    pub elements: Vec<TreeElement>,
}

/// One authored element. Columns and groups carry the kernel facts
/// (ids are identity, DESIGN §3); sections and notes are presentation
/// nodes with kernel [`NodeId`]s (DESIGN §2.6, surface node
/// identity). Every element carries its [`Audience`].
#[derive(Debug, Clone, PartialEq)]
pub enum TreeElement {
    Column(TreeColumn),
    Group(TreeGroup),
    Section(TreeSection),
    Note(TreeNote),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TreeColumn {
    pub id: ColumnId,
    pub label: String,
    pub ty: ScalarType,
    pub arity: Arity,
    /// §2.6 requiredness, its two constant cases (G.7 *Required on
    /// columns*): `true` compiles to the vacuous always-required
    /// rule at publication, `false` to no rule. Conditional
    /// requiredness arrives with the rule editor.
    pub required: bool,
    /// §2.6 format constraint — admissibility over text, beside the
    /// type, never in it (G.7 *Format on text columns*). `Some` on a
    /// text column only ([`crate::tree_edit`]'s backstop).
    pub format: Option<Format>,
    pub audience: Audience,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TreeGroup {
    pub id: GroupId,
    pub label: String,
    pub cardinality: Cardinality,
    pub audience: Audience,
    pub children: Vec<TreeElement>,
}

/// A header section: presentation, may contain elements (the kernel
/// mirror nests, DESIGN §2.6 — DN's flat headers import as
/// containers).
#[derive(Debug, Clone, PartialEq)]
pub struct TreeSection {
    pub id: NodeId,
    pub title: String,
    pub help: Option<String>,
    pub audience: Audience,
    pub children: Vec<TreeElement>,
}

/// An explication: prose, no data. A reviewer-only note is authored
/// guidance DN's annotations privées never had.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeNote {
    pub id: NodeId,
    pub title: Option<String>,
    pub body: String,
    pub audience: Audience,
}

impl TreeElement {
    pub fn audience(&self) -> Audience {
        match self {
            TreeElement::Column(c) => c.audience,
            TreeElement::Group(g) => g.audience,
            TreeElement::Section(s) => s.audience,
            TreeElement::Note(n) => n.audience,
        }
    }
}

impl Tree {
    /// The kernel schema this tree derives to (P.4): presentation
    /// nodes stripped, a section's children lifted into the enclosing
    /// scope, document order preserved. **Audience plays no part** —
    /// a reviewer-only column is still a schema column; audiences
    /// prune *surfaces*, never the schema.
    pub fn schema(&self) -> Schema {
        fn collect(elements: &[TreeElement], out: &mut Vec<Element>) {
            for element in elements {
                match element {
                    TreeElement::Column(c) => out.push(Element::Column(Column {
                        id: c.id.clone(),
                        label: c.label.clone(),
                        ty: c.ty.clone(),
                        arity: c.arity,
                    })),
                    TreeElement::Group(g) => {
                        let mut children = Vec::new();
                        collect(&g.children, &mut children);
                        out.push(Element::Group(Group {
                            id: g.id.clone(),
                            label: g.label.clone(),
                            cardinality: g.cardinality,
                            children,
                            included_from: None,
                        }));
                    }
                    TreeElement::Section(s) => collect(&s.children, out),
                    TreeElement::Note(_) => {}
                }
            }
        }
        let mut root = Vec::new();
        collect(&self.elements, &mut root);
        Schema {
            root,
            resolvers: vec![],
        }
    }

    /// The tree as its stored JSON bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let value = json!({ "elements": elements_to_json(&self.elements) });
        serde_json::to_vec(&value).expect("json! values serialize")
    }

    /// Decodes stored bytes; refuses anything [`Tree::to_bytes`] does
    /// not produce.
    pub fn from_bytes(bytes: &[u8]) -> Result<Tree, TreeDecodeError> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|e| TreeDecodeError(e.to_string()))?;
        let root = as_object(&value)?;
        Ok(Tree {
            elements: elements_from_json(field(root, "elements")?)?,
        })
    }
}

/// The stored tree no longer decodes — never produced by this crate's
/// writes; database-level corruption to surface, not silently replace.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed tree data: {0}")]
pub struct TreeDecodeError(pub String);

// ------------------------------------------------------------ encode

fn elements_to_json(elements: &[TreeElement]) -> Value {
    Value::Array(elements.iter().map(element_to_json).collect())
}

fn element_to_json(element: &TreeElement) -> Value {
    match element {
        TreeElement::Column(c) => json!({
            "kind": "column",
            "id": c.id.as_str(),
            "label": c.label,
            "required": c.required,
            "format": match &c.format {
                None => Value::Null,
                Some(Format::Email) => json!("email"),
                Some(Format::Phone) => json!("phone"),
                Some(Format::Iban) => json!("iban"),
                Some(Format::Regex(pattern)) => json!({ "regex": pattern }),
            },
            "audience": audience_str(c.audience),
            "type": type_to_json(&c.ty, c.arity),
        }),
        TreeElement::Group(g) => json!({
            "kind": "group",
            "id": g.id.as_str(),
            "label": g.label,
            "cardinality": match g.cardinality {
                Cardinality::One => "one",
                Cardinality::Many => "many",
            },
            "audience": audience_str(g.audience),
            "children": elements_to_json(&g.children),
        }),
        TreeElement::Section(s) => json!({
            "kind": "section",
            "id": s.id.as_str(),
            "title": s.title,
            "help": s.help,
            "audience": audience_str(s.audience),
            "children": elements_to_json(&s.children),
        }),
        TreeElement::Note(n) => json!({
            "kind": "note",
            "id": n.id.as_str(),
            "title": n.title,
            "body": n.body,
            "audience": audience_str(n.audience),
        }),
    }
}

fn type_to_json(ty: &ScalarType, arity: Arity) -> Value {
    let multiple = arity == Arity::Many;
    match ty {
        ScalarType::Text => json!({ "kind": "text" }),
        ScalarType::Boolean => json!({ "kind": "boolean" }),
        ScalarType::Integer(unit) => json!({ "kind": "integer", "unit": unit.map(unit_str) }),
        ScalarType::Decimal(unit) => json!({ "kind": "decimal", "unit": unit.map(unit_str) }),
        ScalarType::Date => json!({ "kind": "date" }),
        ScalarType::Datetime => json!({ "kind": "datetime" }),
        ScalarType::Enum(backing) => match backing {
            NomenclatureRef::Inline(rows) => json!({
                "kind": "enum",
                "multiple": multiple,
                "options": rows.iter().map(|row| json!({
                    "id": row.id.as_str(),
                    "label": row.label,
                    "fields": row.fields,
                })).collect::<Vec<_>>(),
            }),
            NomenclatureRef::Published { id, version } => json!({
                "kind": "enum",
                "multiple": multiple,
                "published": { "id": id.as_str(), "version": version },
            }),
        },
        ScalarType::Attachment(constraints) => json!({
            "kind": "attachment",
            "multiple": multiple,
            "accept": constraints.accept,
            "max_bytes": constraints.max_bytes,
        }),
        ScalarType::Geometry => json!({ "kind": "geometry", "multiple": multiple }),
    }
}

fn audience_str(audience: Audience) -> &'static str {
    match audience {
        Audience::All => "all",
        Audience::Reviewer => "reviewer",
    }
}

fn unit_str(unit: Unit) -> &'static str {
    match unit {
        Unit::Millimetre => "mm",
        Unit::Centimetre => "cm",
        Unit::Metre => "m",
        Unit::Kilometre => "km",
        Unit::Gram => "g",
        Unit::Kilogram => "kg",
        Unit::Tonne => "t",
        Unit::Minute => "minute",
        Unit::Hour => "hour",
        Unit::Day => "day",
        Unit::Week => "week",
        Unit::Month => "month",
        Unit::Year => "year",
        Unit::SquareMetre => "m2",
        Unit::Hectare => "ha",
        Unit::SquareKilometre => "km2",
        Unit::Litre => "l",
        Unit::CubicMetre => "m3",
        Unit::Percent => "percent",
    }
}

// ------------------------------------------------------------ decode

fn err<T>(msg: impl Into<String>) -> Result<T, TreeDecodeError> {
    Err(TreeDecodeError(msg.into()))
}

fn as_object(v: &Value) -> Result<&serde_json::Map<String, Value>, TreeDecodeError> {
    v.as_object()
        .ok_or_else(|| TreeDecodeError("expected an object".into()))
}

fn field<'a>(
    m: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a Value, TreeDecodeError> {
    m.get(key)
        .ok_or_else(|| TreeDecodeError(format!("missing '{key}'")))
}

fn str_field(m: &serde_json::Map<String, Value>, key: &str) -> Result<String, TreeDecodeError> {
    match field(m, key)? {
        Value::String(s) => Ok(s.clone()),
        _ => err(format!("'{key}' must be a string")),
    }
}

fn opt_str_field(
    m: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, TreeDecodeError> {
    match field(m, key)? {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => err(format!("'{key}' must be a string or null")),
    }
}

fn bool_field(m: &serde_json::Map<String, Value>, key: &str) -> Result<bool, TreeDecodeError> {
    match field(m, key)? {
        Value::Bool(b) => Ok(*b),
        _ => err(format!("'{key}' must be a boolean")),
    }
}

fn audience_field(m: &serde_json::Map<String, Value>) -> Result<Audience, TreeDecodeError> {
    match str_field(m, "audience")?.as_str() {
        "all" => Ok(Audience::All),
        "reviewer" => Ok(Audience::Reviewer),
        other => err(format!("unknown audience '{other}'")),
    }
}

fn elements_from_json(v: &Value) -> Result<Vec<TreeElement>, TreeDecodeError> {
    match v {
        Value::Array(items) => items.iter().map(element_from_json).collect(),
        _ => err("'elements'/'children' must be an array"),
    }
}

fn element_from_json(v: &Value) -> Result<TreeElement, TreeDecodeError> {
    let m = as_object(v)?;
    Ok(match str_field(m, "kind")?.as_str() {
        "column" => {
            let (ty, arity) = type_from_json(field(m, "type")?)?;
            let audience = audience_field(m)?;
            TreeElement::Column(TreeColumn {
                id: ColumnId::new(str_field(m, "id")?),
                label: str_field(m, "label")?,
                ty,
                arity,
                // Missing in drafts stored before the field existed:
                // default as creation would (public required).
                required: match m.get("required") {
                    Some(Value::Bool(required)) => *required,
                    None => audience == Audience::All,
                    _ => return err("'required' must be a boolean"),
                },
                // Missing in drafts stored before the field existed.
                format: match m.get("format") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(name)) => Some(match name.as_str() {
                        "email" => Format::Email,
                        "phone" => Format::Phone,
                        "iban" => Format::Iban,
                        other => return err(format!("unknown format '{other}'")),
                    }),
                    Some(Value::Object(f)) => Some(Format::Regex(str_field(f, "regex")?)),
                    _ => return err("'format' must be a name, an object or null"),
                },
                audience,
            })
        }
        "group" => TreeElement::Group(TreeGroup {
            id: GroupId::new(str_field(m, "id")?),
            label: str_field(m, "label")?,
            cardinality: match str_field(m, "cardinality")?.as_str() {
                "one" => Cardinality::One,
                "many" => Cardinality::Many,
                other => return err(format!("unknown cardinality '{other}'")),
            },
            audience: audience_field(m)?,
            children: elements_from_json(field(m, "children")?)?,
        }),
        "section" => TreeElement::Section(TreeSection {
            id: NodeId::new(str_field(m, "id")?),
            title: str_field(m, "title")?,
            help: opt_str_field(m, "help")?,
            audience: audience_field(m)?,
            children: elements_from_json(field(m, "children")?)?,
        }),
        "note" => TreeElement::Note(TreeNote {
            id: NodeId::new(str_field(m, "id")?),
            title: opt_str_field(m, "title")?,
            body: str_field(m, "body")?,
            audience: audience_field(m)?,
        }),
        other => return err(format!("unknown element kind '{other}'")),
    })
}

fn type_from_json(v: &Value) -> Result<(ScalarType, Arity), TreeDecodeError> {
    let m = as_object(v)?;
    let multiple = m.contains_key("multiple") && bool_field(m, "multiple")?;
    let arity = if multiple { Arity::Many } else { Arity::One };
    let unit = |m: &serde_json::Map<String, Value>| -> Result<Option<Unit>, TreeDecodeError> {
        match opt_str_field(m, "unit")? {
            None => Ok(None),
            Some(token) => Ok(Some(unit_from_str(&token)?)),
        }
    };
    let ty = match str_field(m, "kind")?.as_str() {
        "text" => ScalarType::Text,
        "boolean" => ScalarType::Boolean,
        "integer" => ScalarType::Integer(unit(m)?),
        "decimal" => ScalarType::Decimal(unit(m)?),
        "date" => ScalarType::Date,
        "datetime" => ScalarType::Datetime,
        "enum" => {
            if let Some(published) = m.get("published") {
                let p = as_object(published)?;
                ScalarType::Enum(NomenclatureRef::Published {
                    id: varve_core::NomenclatureId::new(str_field(p, "id")?),
                    version: match field(p, "version")? {
                        Value::Number(n) => n
                            .as_u64()
                            .and_then(|n| u32::try_from(n).ok())
                            .ok_or_else(|| TreeDecodeError("bad nomenclature version".into()))?,
                        _ => return err("'version' must be a number"),
                    },
                })
            } else {
                let rows = match field(m, "options")? {
                    Value::Array(items) => items
                        .iter()
                        .map(|item| {
                            let o = as_object(item)?;
                            Ok(OptionRow {
                                id: OptionId::new(str_field(o, "id")?),
                                label: str_field(o, "label")?,
                                fields: match o.get("fields") {
                                    None | Some(Value::Null) => Vec::new(),
                                    Some(fields) => serde_json::from_value(fields.clone())
                                        .map_err(|e| {
                                            TreeDecodeError(format!("bad option fields: {e}"))
                                        })?,
                                },
                            })
                        })
                        .collect::<Result<Vec<_>, TreeDecodeError>>()?,
                    _ => return err("'options' must be an array"),
                };
                ScalarType::Enum(NomenclatureRef::Inline(rows))
            }
        }
        "attachment" => {
            let accept = match field(m, "accept")? {
                Value::Array(items) => items
                    .iter()
                    .map(|item| match item {
                        Value::String(s) => Ok(s.clone()),
                        _ => err("'accept' entries must be strings"),
                    })
                    .collect::<Result<Vec<_>, TreeDecodeError>>()?,
                _ => return err("'accept' must be an array"),
            };
            let max_bytes = match field(m, "max_bytes")? {
                Value::Null => None,
                Value::Number(n) => Some(
                    n.as_u64()
                        .ok_or_else(|| TreeDecodeError("bad max_bytes".into()))?,
                ),
                _ => return err("'max_bytes' must be a number or null"),
            };
            ScalarType::Attachment(AttachmentConstraints { accept, max_bytes })
        }
        "geometry" => ScalarType::Geometry,
        other => return err(format!("unknown type kind '{other}'")),
    };
    Ok((ty, arity))
}

fn unit_from_str(token: &str) -> Result<Unit, TreeDecodeError> {
    Ok(match token {
        "mm" => Unit::Millimetre,
        "cm" => Unit::Centimetre,
        "m" => Unit::Metre,
        "km" => Unit::Kilometre,
        "g" => Unit::Gram,
        "kg" => Unit::Kilogram,
        "t" => Unit::Tonne,
        "minute" => Unit::Minute,
        "hour" => Unit::Hour,
        "day" => Unit::Day,
        "week" => Unit::Week,
        "month" => Unit::Month,
        "year" => Unit::Year,
        "m2" => Unit::SquareMetre,
        "ha" => Unit::Hectare,
        "km2" => Unit::SquareKilometre,
        "l" => Unit::Litre,
        "m3" => Unit::CubicMetre,
        "percent" => Unit::Percent,
        other => return err(format!("unknown unit '{other}'")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tree {
        Tree {
            elements: vec![
                TreeElement::Section(TreeSection {
                    id: NodeId::new("s1"),
                    title: "Identité".into(),
                    help: Some("Vos informations".into()),
                    audience: Audience::All,
                    children: vec![
                        TreeElement::Column(TreeColumn {
                            id: ColumnId::new("name"),
                            label: "Nom".into(),
                            ty: ScalarType::Text,
                            arity: Arity::One,
                            format: Some(Format::Email),
                            required: true,
                            audience: Audience::All,
                        }),
                        TreeElement::Note(TreeNote {
                            id: NodeId::new("n1"),
                            title: None,
                            body: "Vérifier la pièce d'identité.".into(),
                            audience: Audience::Reviewer,
                        }),
                    ],
                }),
                TreeElement::Group(TreeGroup {
                    id: GroupId::new("contacts"),
                    label: "Contacts".into(),
                    cardinality: Cardinality::Many,
                    audience: Audience::All,
                    children: vec![TreeElement::Column(TreeColumn {
                        id: ColumnId::new("kind"),
                        label: "Type".into(),
                        ty: ScalarType::Enum(NomenclatureRef::Inline(vec![OptionRow {
                            id: OptionId::new("o1"),
                            label: "Personnel".into(),
                            fields: vec![],
                        }])),
                        arity: Arity::Many,
                        format: None,
                        required: true,
                        audience: Audience::Reviewer,
                    })],
                }),
                TreeElement::Column(TreeColumn {
                    id: ColumnId::new("surface"),
                    label: "Surface".into(),
                    ty: ScalarType::Decimal(Some(Unit::SquareMetre)),
                    arity: Arity::One,
                    format: None,
                    required: true,
                    audience: Audience::All,
                }),
            ],
        }
    }

    #[test]
    fn bytes_round_trip() {
        let tree = sample();
        let decoded = Tree::from_bytes(&tree.to_bytes()).unwrap();
        assert_eq!(decoded, tree);
        // Attachment constraints and empty trees ride too.
        let tree = Tree {
            elements: vec![TreeElement::Column(TreeColumn {
                id: ColumnId::new("files"),
                label: "Pièces".into(),
                ty: ScalarType::Attachment(AttachmentConstraints {
                    accept: vec!["application/pdf".into()],
                    max_bytes: Some(10_000_000),
                }),
                arity: Arity::Many,
                format: None,
                required: false,
                audience: Audience::All,
            })],
        };
        assert_eq!(Tree::from_bytes(&tree.to_bytes()).unwrap(), tree);
        assert_eq!(
            Tree::from_bytes(&Tree::default().to_bytes()).unwrap(),
            Tree::default()
        );
        // A custom pattern rides too.
        let tree = Tree {
            elements: vec![TreeElement::Column(TreeColumn {
                id: ColumnId::new("code"),
                label: "Code".into(),
                ty: ScalarType::Text,
                arity: Arity::One,
                required: true,
                format: Some(Format::Regex("[0-9]{5}".into())),
                audience: Audience::All,
            })],
        };
        assert_eq!(Tree::from_bytes(&tree.to_bytes()).unwrap(), tree);
        // Drafts stored before `required`/`format` existed decode with
        // the creation defaults.
        let legacy = br#"{"elements":[{"kind":"column","id":"c1","label":"Nom","audience":"all","type":{"kind":"text"}}]}"#;
        match &Tree::from_bytes(legacy).unwrap().elements[0] {
            TreeElement::Column(c) => {
                assert!(c.required);
                assert_eq!(c.format, None);
            }
            _ => panic!(),
        }
        assert!(Tree::from_bytes(b"nonsense").is_err());
        assert!(Tree::from_bytes(br#"{"elements":[{"kind":"desk"}]}"#).is_err());
    }

    #[test]
    fn schema_lifts_sections_and_drops_notes() {
        let schema = sample().schema();
        // Section children lift to the root; the note vanishes; the
        // reviewer-only column stays — audiences prune surfaces, not
        // the schema.
        let ids: Vec<&str> = schema
            .root
            .iter()
            .map(|e| match e {
                Element::Column(c) => c.id.as_str(),
                Element::Group(g) => g.id.as_str(),
            })
            .collect();
        assert_eq!(ids, ["name", "contacts", "surface"]);
        match &schema.root[1] {
            Element::Group(g) => {
                assert_eq!(g.children.len(), 1);
                assert_eq!(g.cardinality, Cardinality::Many);
            }
            _ => panic!(),
        }
        assert!(varve_schema::validate(&schema, Default::default()).is_empty());
    }

    /// Every arm of the type/unit/format mapping tables, byte
    /// round-tripped: one column per shape.
    #[test]
    fn every_type_unit_and_format_rides() {
        let units = [
            Unit::Millimetre,
            Unit::Centimetre,
            Unit::Metre,
            Unit::Kilometre,
            Unit::Gram,
            Unit::Kilogram,
            Unit::Tonne,
            Unit::Minute,
            Unit::Hour,
            Unit::Day,
            Unit::Week,
            Unit::Month,
            Unit::Year,
            Unit::SquareMetre,
            Unit::Hectare,
            Unit::SquareKilometre,
            Unit::Litre,
            Unit::CubicMetre,
            Unit::Percent,
        ];
        let mut shapes: Vec<(ScalarType, Option<Format>, Arity)> = vec![
            (ScalarType::Text, Some(Format::Phone), Arity::One),
            (ScalarType::Text, Some(Format::Iban), Arity::One),
            (ScalarType::Boolean, None, Arity::One),
            (ScalarType::Integer(None), None, Arity::One),
            (ScalarType::Decimal(None), None, Arity::One),
            (ScalarType::Date, None, Arity::One),
            (ScalarType::Datetime, None, Arity::One),
            (ScalarType::Geometry, None, Arity::Many),
            (
                ScalarType::Enum(NomenclatureRef::Inline(vec![OptionRow {
                    id: OptionId::new("o1"),
                    label: "Un".into(),
                    // Rows with extra fields activate the resolver
                    // aspect (§2.12) — they must ride too.
                    fields: vec![("code".into(), "01".into())],
                }])),
                None,
                Arity::One,
            ),
            (
                ScalarType::Enum(NomenclatureRef::Published {
                    id: varve_core::NomenclatureId::new("insee-cog"),
                    version: 3,
                }),
                None,
                Arity::Many,
            ),
            (
                ScalarType::Attachment(AttachmentConstraints {
                    accept: vec![],
                    max_bytes: None,
                }),
                None,
                Arity::One,
            ),
        ];
        shapes.extend(
            units
                .iter()
                .map(|u| (ScalarType::Integer(Some(*u)), None, Arity::One)),
        );
        shapes.push((ScalarType::Decimal(Some(Unit::Percent)), None, Arity::One));
        let tree = Tree {
            elements: shapes
                .into_iter()
                .enumerate()
                .map(|(i, (ty, format, arity))| {
                    TreeElement::Column(TreeColumn {
                        id: ColumnId::new(format!("c{i}")),
                        label: format!("C{i}"),
                        ty,
                        arity,
                        required: i % 2 == 0,
                        format,
                        audience: if i % 3 == 0 {
                            Audience::Reviewer
                        } else {
                            Audience::All
                        },
                    })
                })
                .collect(),
        };
        assert_eq!(Tree::from_bytes(&tree.to_bytes()).unwrap(), tree);
    }

    /// Each decode refusal names its reason: corruption surfaces,
    /// never silently repairs (the error type's charter).
    #[test]
    fn malformed_trees_are_refused_with_the_reason() {
        fn refused(doc: &str) -> String {
            Tree::from_bytes(doc.as_bytes()).unwrap_err().0
        }
        fn col(ty: &str) -> String {
            format!(
                r#"{{"elements":[{{"kind":"column","id":"c","label":"C","audience":"all","type":{ty}}}]}}"#
            )
        }
        // The envelope.
        assert_eq!(refused("[]"), "expected an object");
        assert_eq!(refused("{}"), "missing 'elements'");
        assert_eq!(
            refused(r#"{"elements":1}"#),
            "'elements'/'children' must be an array"
        );
        assert_eq!(refused(r#"{"elements":[1]}"#), "expected an object");
        // Elements.
        assert_eq!(
            refused(
                r#"{"elements":[{"kind":"column","id":"c","audience":"all","type":{"kind":"text"}}]}"#
            ),
            "missing 'label'"
        );
        assert_eq!(
            refused(
                r#"{"elements":[{"kind":"column","id":"c","label":1,"audience":"all","type":{"kind":"text"}}]}"#
            ),
            "'label' must be a string"
        );
        assert_eq!(
            refused(
                r#"{"elements":[{"kind":"column","id":"c","label":"C","audience":"staff","type":{"kind":"text"}}]}"#
            ),
            "unknown audience 'staff'"
        );
        assert_eq!(
            refused(
                r#"{"elements":[{"kind":"column","id":"c","label":"C","required":1,"audience":"all","type":{"kind":"text"}}]}"#
            ),
            "'required' must be a boolean"
        );
        assert_eq!(
            refused(
                r#"{"elements":[{"kind":"column","id":"c","label":"C","format":"telepathy","audience":"all","type":{"kind":"text"}}]}"#
            ),
            "unknown format 'telepathy'"
        );
        assert_eq!(
            refused(
                r#"{"elements":[{"kind":"column","id":"c","label":"C","format":7,"audience":"all","type":{"kind":"text"}}]}"#
            ),
            "'format' must be a name, an object or null"
        );
        assert_eq!(
            refused(
                r#"{"elements":[{"kind":"column","id":"c","label":"C","format":{"pattern":"x"},"audience":"all","type":{"kind":"text"}}]}"#
            ),
            "missing 'regex'"
        );
        assert_eq!(
            refused(
                r#"{"elements":[{"kind":"group","id":"g","label":"G","cardinality":"few","audience":"all","children":[]}]}"#
            ),
            "unknown cardinality 'few'"
        );
        assert_eq!(
            refused(
                r#"{"elements":[{"kind":"section","id":"s","title":"T","help":7,"audience":"all","children":[]}]}"#
            ),
            "'help' must be a string or null"
        );
        assert_eq!(
            refused(r#"{"elements":[{"kind":"note","id":"n","title":null,"audience":"all"}]}"#),
            "missing 'body'"
        );
        // Types.
        assert_eq!(
            refused(&col(r#"{"kind":"blob"}"#)),
            "unknown type kind 'blob'"
        );
        assert_eq!(
            refused(&col(r#"{"kind":"text","multiple":7}"#)),
            "'multiple' must be a boolean"
        );
        assert_eq!(
            refused(&col(r#"{"kind":"integer","unit":"furlong"}"#)),
            "unknown unit 'furlong'"
        );
        assert_eq!(
            refused(&col(
                r#"{"kind":"enum","published":{"id":"x","version":"3"},"multiple":false}"#
            )),
            "'version' must be a number"
        );
        assert_eq!(
            refused(&col(
                r#"{"kind":"enum","published":{"id":"x","version":-1},"multiple":false}"#
            )),
            "bad nomenclature version"
        );
        assert_eq!(
            refused(&col(r#"{"kind":"enum","multiple":false}"#)),
            "missing 'options'"
        );
        assert_eq!(
            refused(&col(r#"{"kind":"enum","options":7,"multiple":false}"#)),
            "'options' must be an array"
        );
        assert!(
            refused(&col(
                r#"{"kind":"enum","options":[{"id":"o","label":"O","fields":7}],"multiple":false}"#
            ))
            .starts_with("bad option fields")
        );
        assert_eq!(
            refused(&col(r#"{"kind":"attachment","accept":7,"max_bytes":null}"#)),
            "'accept' must be an array"
        );
        assert_eq!(
            refused(&col(
                r#"{"kind":"attachment","accept":[7],"max_bytes":null}"#
            )),
            "'accept' entries must be strings"
        );
        assert_eq!(
            refused(&col(
                r#"{"kind":"attachment","accept":[],"max_bytes":"big"}"#
            )),
            "'max_bytes' must be a number or null"
        );
        assert_eq!(
            refused(&col(r#"{"kind":"attachment","accept":[],"max_bytes":-1}"#)),
            "bad max_bytes"
        );
    }
}
