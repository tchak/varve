//! Canonical form and content hash of a schema: the revision identity
//! (§2.13 decision 7, plain regime — identical schemas converge on
//! identical ids on every instance). Identity-bearing: types, arity,
//! cardinality, element order, labels, inline nomenclature rows
//! (a relabel is a new revision, §2.11), units, resolver declarations.
//! Not identity-bearing: surfaces (separate objects).

use varve_core::canonical::{CanonicalValue, ContentHash, hash_plain};
use varve_core::{BlockId, RevisionId};

use crate::{
    Arity, Block, BlockRef, Cardinality, Element, NomenclatureRef, OptionRow,
    ResolverDeclaration, ScalarType, Schema,
};

fn obj(pairs: Vec<(&str, CanonicalValue)>) -> CanonicalValue {
    CanonicalValue::Object(
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn string(s: impl ToString) -> CanonicalValue {
    CanonicalValue::String(s.to_string())
}

fn array<T>(items: &[T], f: impl Fn(&T) -> CanonicalValue) -> CanonicalValue {
    CanonicalValue::Array(items.iter().map(f).collect())
}

pub fn schema_canonical(schema: &Schema) -> CanonicalValue {
    obj(vec![
        ("elements", array(&schema.root, element)),
        ("resolvers", array(&schema.resolvers, resolver)),
    ])
}

/// The revision id: hash of the canonical bytes (§2.13). Deterministic
/// and instance-independent by construction.
pub fn revision_id(schema: &Schema) -> RevisionId {
    let hash = hash_plain(&schema_canonical(schema)).expect("schemas contain no floats");
    RevisionId::new(hash.to_string())
}

fn element(el: &Element) -> CanonicalValue {
    match el {
        Element::Column(c) => obj(vec![(
            "column",
            obj(vec![
                ("id", string(&c.id)),
                ("label", string(&c.label)),
                ("type", scalar_type(&c.ty)),
                (
                    "arity",
                    string(match c.arity {
                        Arity::One => "one",
                        Arity::Many => "many",
                    }),
                ),
            ]),
        )]),
        Element::Group(g) => {
            let mut pairs = vec![
                ("id", string(&g.id)),
                ("label", string(&g.label)),
                (
                    "cardinality",
                    string(match g.cardinality {
                        Cardinality::One => "one",
                        Cardinality::Many => "many",
                    }),
                ),
                ("children", array(&g.children, element)),
            ];
            // Optional, omitted when absent (§2.13 decision 7) — schemas
            // without blocks hash exactly as before.
            if let Some(b) = &g.included_from {
                pairs.push(("included_from", block_ref(b)));
            }
            obj(vec![("group", obj(pairs))])
        }
    }
}

fn block_ref(b: &BlockRef) -> CanonicalValue {
    obj(vec![
        ("id", string(&b.id)),
        ("version", CanonicalValue::Int(i64::from(b.version))),
    ])
}

/// Canonical form of a published block (§2.1): its shell and paired
/// declarations — schema-side, plain regime (§2.13). The shell is
/// canonicalized *without* provenance: a block is not included from
/// anything.
pub fn block_canonical(block: &Block) -> CanonicalValue {
    obj(vec![
        ("id", string(&block.id)),
        ("version", CanonicalValue::Int(i64::from(block.version))),
        ("group", element(&Element::Group(block.group.clone()))),
        ("resolvers", array(&block.resolvers, resolver)),
    ])
}

/// A block's content address: the plain hash of its canonical form.
pub fn block_hash(block: &Block) -> ContentHash {
    hash_plain(&block_canonical(block)).expect("blocks contain no floats")
}

pub fn block_from_canonical(v: &CanonicalValue) -> Result<Block, SchemaDecodeError> {
    let m = as_obj(v)?;
    let group = match element_from(get(m, "group")?)? {
        Element::Group(g) => g,
        Element::Column(_) => return err("block 'group' must be a group"),
    };
    Ok(Block {
        id: BlockId::new(get_str(m, "id")?),
        version: get_u32(m, "version")?,
        group,
        resolvers: get_arr(m, "resolvers")?
            .iter()
            .map(resolver_from)
            .collect::<Result<_, _>>()?,
    })
}

#[cfg(test)]
pub(crate) fn scalar_type_canonical_for_test(ty: &ScalarType) -> CanonicalValue {
    scalar_type(ty)
}

fn scalar_type(ty: &ScalarType) -> CanonicalValue {
    let simple = |kind: &str| obj(vec![("kind", string(kind))]);
    match ty {
        ScalarType::Text => simple("text"),
        ScalarType::Boolean => simple("boolean"),
        ScalarType::Date => simple("date"),
        ScalarType::Datetime => simple("datetime"),
        // §2.15: constraints are identity-bearing — narrowing the
        // accept set is a revision.
        ScalarType::Attachment(constraints) => {
            // The accept set is a set (case-insensitive, unordered):
            // canonicalize its normalized form so order and case are
            // not identity-bearing.
            let constraints = constraints.normalized();
            let mut pairs = vec![("kind", string("attachment"))];
            if !constraints.accept.is_empty() {
                pairs.push((
                    "accept",
                    CanonicalValue::Array(
                        constraints.accept.iter().map(string).collect(),
                    ),
                ));
            }
            if let Some(max) = constraints.max_bytes {
                pairs.push(("max_bytes", CanonicalValue::Int(max as i64)));
            }
            obj(pairs)
        }
        ScalarType::Geometry => simple("geometry"),
        ScalarType::Integer(unit) | ScalarType::Decimal(unit) => {
            let kind = if matches!(ty, ScalarType::Integer(_)) {
                "integer"
            } else {
                "decimal"
            };
            let mut pairs = vec![("kind", string(kind))];
            if let Some(unit) = unit {
                pairs.push(("unit", string(unit.name())));
            }
            obj(pairs)
        }
        ScalarType::Enum(nref) => obj(vec![
            ("kind", string("enum")),
            (
                "nomenclature",
                match nref {
                    NomenclatureRef::Inline(rows) => {
                        obj(vec![("inline", array(rows, option_row))])
                    }
                    NomenclatureRef::Published { id, version } => obj(vec![(
                        "published",
                        obj(vec![
                            ("id", string(id)),
                            ("version", CanonicalValue::Int(i64::from(*version))),
                        ]),
                    )]),
                },
            ),
        ]),
    }
}

fn option_row(row: &OptionRow) -> CanonicalValue {
    obj(vec![
        ("id", string(&row.id)),
        ("label", string(&row.label)),
        (
            "fields",
            CanonicalValue::Array(
                row.fields
                    .iter()
                    .map(|(k, v)| CanonicalValue::Array(vec![string(k), string(v)]))
                    .collect(),
            ),
        ),
    ])
}

fn resolver(decl: &ResolverDeclaration) -> CanonicalValue {
    obj(vec![
        ("id", string(&decl.id)),
        ("version", CanonicalValue::Int(i64::from(decl.version))),
        (
            "input",
            CanonicalValue::Array(
                decl.input
                    .iter()
                    .map(|(column, ty)| {
                        CanonicalValue::Array(vec![string(column), scalar_type(ty)])
                    })
                    .collect(),
            ),
        ),
        (
            "result",
            CanonicalValue::Array(
                decl.result_type
                    .iter()
                    .map(|f| obj(vec![("name", string(&f.name)), ("type", scalar_type(&f.ty))]))
                    .collect(),
            ),
        ),
        (
            "mapping",
            CanonicalValue::Array(
                decl.mapping
                    .iter()
                    .map(|m| {
                        obj(vec![
                            ("field", string(&m.result_field)),
                            ("target", string(&m.target)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

// ---------------------------------------------------------------------
// Decoding — the parse direction the wire (§5) uses. Strict and total:
// every malformation is a `SchemaDecodeError`, never a panic. Round-trip
// with `schema_canonical` is the law tested in the crate.

use crate::{Column, Group, Mapping, ResultField, AttachmentConstraints, Unit};
use varve_core::{ColumnId, GroupId, NomenclatureId, OptionId, ResolverId};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed schema: {0}")]
pub struct SchemaDecodeError(pub String);

type Obj = std::collections::BTreeMap<String, CanonicalValue>;

fn err<T>(msg: impl Into<String>) -> Result<T, SchemaDecodeError> {
    Err(SchemaDecodeError(msg.into()))
}

fn as_obj(v: &CanonicalValue) -> Result<&Obj, SchemaDecodeError> {
    match v {
        CanonicalValue::Object(m) => Ok(m),
        _ => err("expected an object"),
    }
}

fn as_arr(v: &CanonicalValue) -> Result<&[CanonicalValue], SchemaDecodeError> {
    match v {
        CanonicalValue::Array(a) => Ok(a),
        _ => err("expected an array"),
    }
}

fn get_str(m: &Obj, key: &str) -> Result<String, SchemaDecodeError> {
    match m.get(key) {
        Some(CanonicalValue::String(s)) => Ok(s.clone()),
        _ => err(format!("missing string '{key}'")),
    }
}

fn get_int(m: &Obj, key: &str) -> Result<i64, SchemaDecodeError> {
    match m.get(key) {
        Some(CanonicalValue::Int(i)) => Ok(*i),
        _ => err(format!("missing integer '{key}'")),
    }
}

fn get<'a>(m: &'a Obj, key: &str) -> Result<&'a CanonicalValue, SchemaDecodeError> {
    m.get(key).ok_or_else(|| SchemaDecodeError(format!("missing '{key}'")))
}

fn get_u32(m: &Obj, key: &str) -> Result<u32, SchemaDecodeError> {
    u32::try_from(get_int(m, key)?).map_err(|_| SchemaDecodeError(format!("bad u32 '{key}'")))
}

fn get_arr<'a>(m: &'a Obj, key: &str) -> Result<&'a [CanonicalValue], SchemaDecodeError> {
    m.get(key)
        .map(as_arr)
        .unwrap_or_else(|| err(format!("missing array '{key}'")))
}

pub fn schema_from_canonical(v: &CanonicalValue) -> Result<Schema, SchemaDecodeError> {
    let m = as_obj(v)?;
    let root = get_arr(m, "elements")?
        .iter()
        .map(element_from)
        .collect::<Result<_, _>>()?;
    let resolvers = get_arr(m, "resolvers")?
        .iter()
        .map(resolver_from)
        .collect::<Result<_, _>>()?;
    Ok(Schema { root, resolvers })
}

fn arity_from(s: &str) -> Result<Arity, SchemaDecodeError> {
    match s {
        "one" => Ok(Arity::One),
        "many" => Ok(Arity::Many),
        other => err(format!("bad arity '{other}'")),
    }
}

fn element_from(v: &CanonicalValue) -> Result<Element, SchemaDecodeError> {
    let m = as_obj(v)?;
    if let Some(c) = m.get("column") {
        let c = as_obj(c)?;
        return Ok(Element::Column(Column {
            id: ColumnId::new(get_str(c, "id")?),
            label: get_str(c, "label")?,
            ty: scalar_type_from(
                c.get("type").ok_or_else(|| SchemaDecodeError("missing 'type'".into()))?,
            )?,
            arity: arity_from(&get_str(c, "arity")?)?,
        }));
    }
    if let Some(g) = m.get("group") {
        let g = as_obj(g)?;
        return Ok(Element::Group(Group {
            id: GroupId::new(get_str(g, "id")?),
            label: get_str(g, "label")?,
            cardinality: match get_str(g, "cardinality")?.as_str() {
                "one" => Cardinality::One,
                "many" => Cardinality::Many,
                other => return err(format!("bad cardinality '{other}'")),
            },
            children: get_arr(g, "children")?
                .iter()
                .map(element_from)
                .collect::<Result<_, _>>()?,
            included_from: match g.get("included_from") {
                None => None,
                Some(b) => {
                    let b = as_obj(b)?;
                    Some(BlockRef {
                        id: BlockId::new(get_str(b, "id")?),
                        version: get_u32(b, "version")?,
                    })
                }
            },
        }));
    }
    err("element must be 'column' or 'group'")
}

pub fn scalar_type_from_canonical(v: &CanonicalValue) -> Result<ScalarType, SchemaDecodeError> {
    scalar_type_from(v)
}

fn scalar_type_from(v: &CanonicalValue) -> Result<ScalarType, SchemaDecodeError> {
    let m = as_obj(v)?;
    let unit = |m: &Obj| -> Result<Option<Unit>, SchemaDecodeError> {
        match m.get("unit") {
            None => Ok(None),
            Some(CanonicalValue::String(name)) => Unit::parse(name)
                .map(Some)
                .ok_or_else(|| SchemaDecodeError(format!("unknown unit '{name}'"))),
            Some(_) => err("'unit' must be a string"),
        }
    };
    Ok(match get_str(m, "kind")?.as_str() {
        "text" => ScalarType::Text,
        "boolean" => ScalarType::Boolean,
        "date" => ScalarType::Date,
        "datetime" => ScalarType::Datetime,
        "geometry" => ScalarType::Geometry,
        "integer" => ScalarType::Integer(unit(m)?),
        "decimal" => ScalarType::Decimal(unit(m)?),
        "attachment" => {
            let accept = match m.get("accept") {
                None => Vec::new(),
                Some(a) => as_arr(a)?
                    .iter()
                    .map(|p| match p {
                        CanonicalValue::String(s) => Ok(s.clone()),
                        _ => err("accept patterns must be strings"),
                    })
                    .collect::<Result<_, _>>()?,
            };
            let max_bytes = match m.get("max_bytes") {
                None => None,
                Some(CanonicalValue::Int(i)) if *i >= 0 => Some(*i as u64),
                Some(_) => return err("'max_bytes' must be a non-negative integer"),
            };
            ScalarType::Attachment(AttachmentConstraints { accept, max_bytes })
        }
        "enum" => {
            let n = as_obj(
                m.get("nomenclature")
                    .ok_or_else(|| SchemaDecodeError("missing 'nomenclature'".into()))?,
            )?;
            if let Some(rows) = n.get("inline") {
                let rows = as_arr(rows)?
                    .iter()
                    .map(option_row_from)
                    .collect::<Result<_, _>>()?;
                ScalarType::Enum(NomenclatureRef::Inline(rows))
            } else if let Some(p) = n.get("published") {
                let p = as_obj(p)?;
                ScalarType::Enum(NomenclatureRef::Published {
                    id: NomenclatureId::new(get_str(p, "id")?),
                    version: u32::try_from(get_int(p, "version")?)
                        .map_err(|_| SchemaDecodeError("bad version".into()))?,
                })
            } else {
                return err("nomenclature must be 'inline' or 'published'");
            }
        }
        other => return err(format!("unknown scalar kind '{other}'")),
    })
}

pub fn option_row_from_canonical(v: &CanonicalValue) -> Result<OptionRow, SchemaDecodeError> {
    option_row_from(v)
}

fn option_row_from(v: &CanonicalValue) -> Result<OptionRow, SchemaDecodeError> {
    let m = as_obj(v)?;
    let fields = get_arr(m, "fields")?
        .iter()
        .map(|pair| {
            let pair = as_arr(pair)?;
            match pair {
                [CanonicalValue::String(k), CanonicalValue::String(v)] => {
                    Ok((k.clone(), v.clone()))
                }
                _ => err("field must be a [key, value] string pair"),
            }
        })
        .collect::<Result<_, _>>()?;
    Ok(OptionRow {
        id: OptionId::new(get_str(m, "id")?),
        label: get_str(m, "label")?,
        fields,
    })
}

pub fn option_row_canonical(row: &OptionRow) -> CanonicalValue {
    option_row(row)
}

fn resolver_from(v: &CanonicalValue) -> Result<ResolverDeclaration, SchemaDecodeError> {
    let m = as_obj(v)?;
    let input = get_arr(m, "input")?
        .iter()
        .map(|pair| {
            let pair = as_arr(pair)?;
            match pair {
                [CanonicalValue::String(column), ty] => {
                    Ok((ColumnId::new(column), scalar_type_from(ty)?))
                }
                _ => err("input must be [column, type]"),
            }
        })
        .collect::<Result<_, _>>()?;
    let result_type = get_arr(m, "result")?
        .iter()
        .map(|f| {
            let f = as_obj(f)?;
            Ok(ResultField {
                name: get_str(f, "name")?,
                ty: scalar_type_from(
                    f.get("type").ok_or_else(|| SchemaDecodeError("missing 'type'".into()))?,
                )?,
            })
        })
        .collect::<Result<_, _>>()?;
    let mapping = get_arr(m, "mapping")?
        .iter()
        .map(|mp| {
            let mp = as_obj(mp)?;
            Ok(Mapping {
                result_field: get_str(mp, "field")?,
                target: ColumnId::new(get_str(mp, "target")?),
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(ResolverDeclaration {
        id: ResolverId::new(get_str(m, "id")?),
        version: u32::try_from(get_int(m, "version")?)
            .map_err(|_| SchemaDecodeError("bad version".into()))?,
        input,
        result_type,
        mapping,
    })
}
