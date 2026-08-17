//! Canonical (JSON-shaped) forms of ops, states and origins — the bytes
//! that per-op commitments commit to (§2.13 decision 4).
//!
//! Scalars are pre-rendered per §2.13 decision 3: exact numbers
//! (integers, decimals) and instants as their normalized strings — JSON
//! numbers are JCS doubles and cannot carry a full i64. Geometry is
//! embedded as its canonical JSON value (numbers as doubles, ES6
//! rendering), never as a stringified blob.

use std::collections::BTreeMap;

use varve_core::RowPath;
use varve_core::canonical::CanonicalValue;
use varve_value::{CellState, CellValue, Op, Scalar};

use crate::{Derivation, Origin};

fn obj(pairs: Vec<(&str, CanonicalValue)>) -> CanonicalValue {
    CanonicalValue::Object(
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn string(s: impl ToString) -> CanonicalValue {
    CanonicalValue::String(s.to_string())
}

fn path(p: &RowPath) -> CanonicalValue {
    CanonicalValue::Array(
        p.segments()
            .iter()
            .map(|seg| {
                CanonicalValue::Array(vec![string(&seg.group), string(&seg.item)])
            })
            .collect(),
    )
}

fn scalar(s: &Scalar) -> CanonicalValue {
    match s {
        Scalar::Text(v) => obj(vec![("text", string(v))]),
        Scalar::Boolean(v) => obj(vec![("boolean", CanonicalValue::Bool(*v))]),
        Scalar::Integer(v) => obj(vec![("integer", string(v))]),
        Scalar::Decimal(v) => obj(vec![("decimal", string(v))]),
        Scalar::Date(v) => obj(vec![("date", string(v))]),
        Scalar::Datetime(v) => obj(vec![("datetime", string(v))]),
        Scalar::Enum(v) => obj(vec![("option", string(v))]),
        Scalar::Attachment(a) => obj(vec![(
            "attachment",
            obj(vec![
                ("id", string(&a.id)),
                ("hash", string(a.hash)),
                ("filename", string(&a.filename)),
                ("content_type", string(&a.content_type)),
                ("byte_size", CanonicalValue::Int(a.byte_size as i64)),
            ]),
        )]),
        Scalar::Geometry(f) => obj(vec![("geometry", f.to_canonical().clone())]),
    }
}

/// Stored state (§2.4/§5), one encoding for ops and cell maps alike:
/// `null` = empty; a scalar object = an arity-one value; an array of
/// scalar objects = an arity-many value (never empty — a blank `many`
/// cell is `null`). Absent is the key's absence, never a value.
fn state(s: &CellState) -> CanonicalValue {
    match s {
        CellState::Empty => CanonicalValue::Null,
        CellState::Value(CellValue::One(v)) => scalar(v),
        CellState::Value(CellValue::Many(vs)) => {
            CanonicalValue::Array(vs.iter().map(scalar).collect())
        }
    }
}

pub fn op(op: &Op) -> CanonicalValue {
    match op {
        Op::Set {
            column,
            path: p,
            state: s,
        } => obj(vec![
            ("op", string("set")),
            ("column", string(column)),
            ("path", path(p)),
            ("state", state(s)),
        ]),
        Op::Unset { column, path: p } => obj(vec![
            ("op", string("unset")),
            ("column", string(column)),
            ("path", path(p)),
        ]),
        Op::AddItem {
            group,
            parent,
            item,
            at,
        } => obj(vec![
            ("op", string("add_item")),
            ("group", string(group)),
            ("parent", path(parent)),
            ("item", string(item)),
            ("at", CanonicalValue::Int(*at as i64)),
        ]),
        Op::RemoveItem {
            group,
            parent,
            item,
        } => obj(vec![
            ("op", string("remove_item")),
            ("group", string(group)),
            ("parent", path(parent)),
            ("item", string(item)),
        ]),
        Op::Reorder {
            group,
            parent,
            order,
        } => obj(vec![
            ("op", string("reorder")),
            ("group", string(group)),
            ("parent", path(parent)),
            (
                "order",
                CanonicalValue::Array(order.iter().map(string).collect()),
            ),
        ]),
    }
}

fn derivation(d: &Derivation) -> CanonicalValue {
    obj(vec![
        ("source", string(&d.source)),
        ("source_version", CanonicalValue::Int(d.source_version as i64)),
        ("mapping_version", CanonicalValue::Int(d.mapping_version as i64)),
        ("snapshot_ref", string(d.snapshot_ref)),
    ])
}

/// The entry's redactable metadata: origin and note (§2.13 decision 8 —
/// origin describes the values, so it is content, not envelope).
pub fn meta(origin: &Origin, note: Option<&str>) -> CanonicalValue {
    let origin = match origin {
        Origin::Entered => string("entered"),
        Origin::Derived(d) => obj(vec![("derived", derivation(d))]),
        Origin::Overridden { superseded } => obj(vec![(
            "overridden",
            match superseded {
                None => CanonicalValue::Null,
                Some(d) => derivation(d),
            },
        )]),
    };
    obj(vec![
        ("origin", origin),
        (
            "note",
            match note {
                None => CanonicalValue::Null,
                Some(n) => string(n),
            },
        ),
    ])
}

// ---------------------------------------------------------------------
// Decoding (the wire's parse direction, §5) plus whole-entry encoding.
// Strict and total. Round-trip is a tested law.

use varve_core::canonical::{ContentHash, MAX_SAFE_INTEGER, Salt};
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, ResolverId, RevisionId};
use varve_value::{AttachmentRef, Feature};

use crate::{Actor, ActorKind, Entry, EntryContent, EntrySalts, Envelope};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed record data: {0}")]
pub struct RecordDecodeError(pub String);

type Obj = BTreeMap<String, CanonicalValue>;

fn err<T>(msg: impl Into<String>) -> Result<T, RecordDecodeError> {
    Err(RecordDecodeError(msg.into()))
}

fn as_obj(v: &CanonicalValue) -> Result<&Obj, RecordDecodeError> {
    match v {
        CanonicalValue::Object(m) => Ok(m),
        _ => err("expected an object"),
    }
}

fn as_arr(v: &CanonicalValue) -> Result<&[CanonicalValue], RecordDecodeError> {
    match v {
        CanonicalValue::Array(a) => Ok(a),
        _ => err("expected an array"),
    }
}

fn get_str(m: &Obj, key: &str) -> Result<String, RecordDecodeError> {
    match m.get(key) {
        Some(CanonicalValue::String(s)) => Ok(s.clone()),
        _ => err(format!("missing string '{key}'")),
    }
}

/// Structural counts ride as JSON numbers, so they must be JCS-safe;
/// the wire reader already refuses larger ones — this keeps the decoder
/// total on its own.
fn get_int(m: &Obj, key: &str) -> Result<i64, RecordDecodeError> {
    match m.get(key) {
        Some(CanonicalValue::Int(i)) if i.unsigned_abs() <= MAX_SAFE_INTEGER as u64 => Ok(*i),
        _ => err(format!("missing JCS-safe integer '{key}'")),
    }
}

fn get<'a>(m: &'a Obj, key: &str) -> Result<&'a CanonicalValue, RecordDecodeError> {
    m.get(key)
        .ok_or_else(|| RecordDecodeError(format!("missing '{key}'")))
}

pub fn path_from(v: &CanonicalValue) -> Result<RowPath, RecordDecodeError> {
    let mut path = RowPath::root();
    for seg in as_arr(v)? {
        match as_arr(seg)? {
            [CanonicalValue::String(group), CanonicalValue::String(item)] => {
                path = path.child(PathSeg {
                    group: GroupId::new(group),
                    item: ItemId::new(item),
                });
            }
            _ => return err("path segment must be [group, item]"),
        }
    }
    Ok(path)
}

pub fn path_canonical(p: &RowPath) -> CanonicalValue {
    path(p)
}

pub fn scalar_canonical(s: &Scalar) -> CanonicalValue {
    scalar(s)
}

pub fn scalar_from(v: &CanonicalValue) -> Result<Scalar, RecordDecodeError> {
    let m = as_obj(v)?;
    let (kind, inner) = m
        .iter()
        .next()
        .ok_or_else(|| RecordDecodeError("empty scalar".into()))?;
    let text = |v: &CanonicalValue| match v {
        CanonicalValue::String(s) => Ok(s.clone()),
        _ => err("expected a string"),
    };
    Ok(match kind.as_str() {
        "text" => Scalar::Text(text(inner)?),
        "boolean" => match inner {
            CanonicalValue::Bool(b) => Scalar::Boolean(*b),
            _ => return err("boolean must be a bool"),
        },
        // Exact integer as a string: strict — the text must be the
        // normalized rendering (no `+`, no leading zeros, no `-0`).
        "integer" => {
            let t = text(inner)?;
            match t.parse::<i64>() {
                Ok(i) if i.to_string() == t => Scalar::Integer(i),
                _ => return err("integer must be a normalized decimal string"),
            }
        }
        "decimal" => Scalar::Decimal(
            Decimal::parse(&text(inner)?).map_err(|e| RecordDecodeError(e.to_string()))?,
        ),
        "date" => Scalar::Date(
            Date::parse(&text(inner)?).map_err(|e| RecordDecodeError(e.to_string()))?,
        ),
        "datetime" => Scalar::Datetime(
            Instant::parse(&text(inner)?).map_err(|e| RecordDecodeError(e.to_string()))?,
        ),
        "option" => Scalar::Enum(OptionId::new(text(inner)?)),
        "attachment" => {
            let a = as_obj(inner)?;
            Scalar::Attachment(Box::new(AttachmentRef {
                id: get_str(a, "id")?,
                hash: get_str(a, "hash")?
                    .parse::<ContentHash>()
                    .map_err(|_| RecordDecodeError("bad content hash".into()))?,
                filename: get_str(a, "filename")?,
                content_type: get_str(a, "content_type")?,
                byte_size: u64::try_from(get_int(a, "byte_size")?)
                    .map_err(|_| RecordDecodeError("bad byte_size".into()))?,
            }))
        }
        "geometry" => Scalar::Geometry(Box::new(
            Feature::from_canonical(inner).map_err(|e| RecordDecodeError(e.to_string()))?,
        )),
        other => return err(format!("unknown scalar kind '{other}'")),
    })
}

pub fn state_canonical(s: &CellState) -> CanonicalValue {
    state(s)
}

pub fn state_from(v: &CanonicalValue) -> Result<CellState, RecordDecodeError> {
    match v {
        CanonicalValue::Null => Ok(CellState::Empty),
        CanonicalValue::Object(_) => Ok(CellState::Value(CellValue::One(scalar_from(v)?))),
        CanonicalValue::Array(list) => {
            if list.is_empty() {
                // One state, one encoding (§2.4): a blank `many` cell is
                // `null`, never `[]`.
                return err("a zero-length list is not a value — use null");
            }
            Ok(CellState::Value(CellValue::Many(
                list.iter().map(scalar_from).collect::<Result<_, _>>()?,
            )))
        }
        _ => err("a cell state is null, a scalar object, or an array of scalar objects"),
    }
}

pub fn op_from(v: &CanonicalValue) -> Result<Op, RecordDecodeError> {
    let m = as_obj(v)?;
    let column = || Ok::<_, RecordDecodeError>(ColumnId::new(get_str(m, "column")?));
    let group = || Ok::<_, RecordDecodeError>(GroupId::new(get_str(m, "group")?));
    let item = || Ok::<_, RecordDecodeError>(ItemId::new(get_str(m, "item")?));
    Ok(match get_str(m, "op")?.as_str() {
        "set" => Op::Set {
            column: column()?,
            path: path_from(get(m, "path")?)?,
            state: state_from(get(m, "state")?)?,
        },
        "unset" => Op::Unset { column: column()?, path: path_from(get(m, "path")?)? },
        "add_item" => Op::AddItem {
            group: group()?,
            parent: path_from(get(m, "parent")?)?,
            item: item()?,
            at: usize::try_from(get_int(m, "at")?)
                .map_err(|_| RecordDecodeError("bad index".into()))?,
        },
        "remove_item" => Op::RemoveItem {
            group: group()?,
            parent: path_from(get(m, "parent")?)?,
            item: item()?,
        },
        "reorder" => Op::Reorder {
            group: group()?,
            parent: path_from(get(m, "parent")?)?,
            order: as_arr(get(m, "order")?)?
                .iter()
                .map(|i| match i {
                    CanonicalValue::String(s) => Ok(ItemId::new(s)),
                    _ => err("order entries must be strings"),
                })
                .collect::<Result<_, _>>()?,
        },
        other => return err(format!("unknown op '{other}'")),
    })
}

fn derivation_from(v: &CanonicalValue) -> Result<Derivation, RecordDecodeError> {
    let m = as_obj(v)?;
    Ok(Derivation {
        source: ResolverId::new(get_str(m, "source")?),
        source_version: u32::try_from(get_int(m, "source_version")?)
            .map_err(|_| RecordDecodeError("bad version".into()))?,
        mapping_version: u32::try_from(get_int(m, "mapping_version")?)
            .map_err(|_| RecordDecodeError("bad version".into()))?,
        snapshot_ref: get_str(m, "snapshot_ref")?
            .parse::<ContentHash>()
            .map_err(|_| RecordDecodeError("bad snapshot ref".into()))?,
    })
}

pub fn origin_from(v: &CanonicalValue) -> Result<Origin, RecordDecodeError> {
    if let CanonicalValue::String(s) = v
        && s == "entered"
    {
        return Ok(Origin::Entered);
    }
    let m = as_obj(v)?;
    if let Some(d) = m.get("derived") {
        return Ok(Origin::Derived(derivation_from(d)?));
    }
    if let Some(o) = m.get("overridden") {
        return Ok(Origin::Overridden {
            superseded: match o {
                CanonicalValue::Null => None,
                d => Some(derivation_from(d)?),
            },
        });
    }
    err("origin must be 'entered', 'derived' or 'overridden'")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Result<[u8; 32], RecordDecodeError> {
    if s.len() != 64 {
        return err("salt must be 32 bytes hex");
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let chunk = std::str::from_utf8(chunk).map_err(|_| RecordDecodeError("bad hex".into()))?;
        out[i] = u8::from_str_radix(chunk, 16).map_err(|_| RecordDecodeError("bad hex".into()))?;
    }
    Ok(out)
}

/// A full entry — history export (§5): envelope in the clear, content
/// and salts alongside so the receiver can recompute every commitment
/// and verify the chain.
pub fn entry_canonical(entry: &Entry) -> CanonicalValue {
    let e = &entry.envelope;
    obj(vec![
        ("seq", CanonicalValue::Int(e.seq as i64)),
        ("prev", string(e.prev)),
        ("actor", string(&e.actor.id)),
        (
            "actor_kind",
            string(match e.actor.kind {
                ActorKind::Human => "human",
                ActorKind::Resolver => "resolver",
                ActorKind::System => "system",
            }),
        ),
        ("timestamp", string(e.timestamp)),
        ("revision", string(&e.revision)),
        ("base_version", CanonicalValue::Int(e.base_version as i64)),
        ("content_hash", string(e.content_hash)),
        ("origin", origin_canonical(&entry.content.origin)),
        (
            "note",
            match &entry.content.note {
                None => CanonicalValue::Null,
                Some(n) => string(n),
            },
        ),
        (
            "ops",
            CanonicalValue::Array(entry.content.ops.iter().map(op).collect()),
        ),
        ("meta_salt", string(hex(&entry.salts.meta.0))),
        (
            "op_salts",
            CanonicalValue::Array(entry.salts.ops.iter().map(|s| string(hex(&s.0))).collect()),
        ),
    ])
}

fn origin_canonical(origin: &Origin) -> CanonicalValue {
    match as_obj(&meta(origin, None)) {
        Ok(m) => m.get("origin").cloned().unwrap_or(CanonicalValue::Null),
        Err(_) => CanonicalValue::Null,
    }
}

pub fn entry_from(v: &CanonicalValue) -> Result<Entry, RecordDecodeError> {
    let m = as_obj(v)?;
    let ops = as_arr(get(m, "ops")?)?
        .iter()
        .map(op_from)
        .collect::<Result<Vec<_>, _>>()?;
    let content = EntryContent {
        ops,
        origin: origin_from(get(m, "origin")?)?,
        note: match get(m, "note")? {
            CanonicalValue::Null => None,
            CanonicalValue::String(s) => Some(s.clone()),
            _ => return err("note must be a string or null"),
        },
    };
    let salts = EntrySalts {
        meta: Salt(unhex(&get_str(m, "meta_salt")?)?),
        ops: as_arr(get(m, "op_salts")?)?
            .iter()
            .map(|s| match s {
                CanonicalValue::String(s) => Ok(Salt(unhex(s)?)),
                _ => err("salt must be a string"),
            })
            .collect::<Result<_, _>>()?,
    };
    if salts.ops.len() != content.ops.len() {
        return err(format!(
            "{} ops but {} op salts (need one per op)",
            content.ops.len(),
            salts.ops.len()
        ));
    }
    let envelope = Envelope {
        seq: u64::try_from(get_int(m, "seq")?).map_err(|_| RecordDecodeError("bad seq".into()))?,
        prev: get_str(m, "prev")?
            .parse()
            .map_err(|_| RecordDecodeError("bad prev".into()))?,
        actor: Actor {
            id: get_str(m, "actor")?,
            kind: match get_str(m, "actor_kind")?.as_str() {
                "human" => ActorKind::Human,
                "resolver" => ActorKind::Resolver,
                "system" => ActorKind::System,
                other => return err(format!("unknown actor kind '{other}'")),
            },
        },
        timestamp: Instant::parse(&get_str(m, "timestamp")?)
            .map_err(|e| RecordDecodeError(e.to_string()))?,
        revision: RevisionId::new(get_str(m, "revision")?),
        base_version: u64::try_from(get_int(m, "base_version")?)
            .map_err(|_| RecordDecodeError("bad base_version".into()))?,
        content_hash: get_str(m, "content_hash")?
            .parse()
            .map_err(|_| RecordDecodeError("bad content hash".into()))?,
    };
    Ok(Entry { envelope, content, salts })
}
