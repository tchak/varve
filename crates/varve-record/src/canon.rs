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

/// Strict decoding: exactly the keys the canonical form emits.
fn only_keys(m: &Obj, allowed: &[&str]) -> Result<(), RecordDecodeError> {
    match m.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(extra) => err(format!("unexpected key '{extra}'")),
        None => Ok(()),
    }
}

/// A tagged union: exactly one key.
fn single_key<'a>(m: &'a Obj, what: &str) -> Result<(&'a String, &'a CanonicalValue), RecordDecodeError> {
    let mut it = m.iter();
    match (it.next(), it.next()) {
        (Some(entry), None) => Ok(entry),
        (None, _) => err(format!("empty {what}")),
        (Some(_), Some(_)) => err(format!("{what} must have exactly one key")),
    }
}

/// A text that must be the normalized rendering of what it parses to —
/// one value, one text.
fn normalized<T: std::fmt::Display>(
    text: &str,
    parse: impl Fn(&str) -> Result<T, RecordDecodeError>,
    what: &str,
) -> Result<T, RecordDecodeError> {
    let value = parse(text)?;
    if value.to_string() != text {
        return err(format!("{what} must be in normalized form"));
    }
    Ok(value)
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
    let (kind, inner) = single_key(m, "scalar")?;
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
        "decimal" => Scalar::Decimal(normalized(
            &text(inner)?,
            |t| Decimal::parse(t).map_err(|e| RecordDecodeError(e.to_string())),
            "decimal",
        )?),
        "date" => Scalar::Date(
            Date::parse(&text(inner)?).map_err(|e| RecordDecodeError(e.to_string()))?,
        ),
        "datetime" => Scalar::Datetime(normalized(
            &text(inner)?,
            |t| Instant::parse(t).map_err(|e| RecordDecodeError(e.to_string())),
            "datetime",
        )?),
        "option" => Scalar::Enum(OptionId::new(text(inner)?)),
        "attachment" => {
            let a = as_obj(inner)?;
            only_keys(a, &["id", "hash", "filename", "content_type", "byte_size"])?;
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
    let op = get_str(m, "op")?;
    match op.as_str() {
        "set" => only_keys(m, &["op", "column", "path", "state"])?,
        "unset" => only_keys(m, &["op", "column", "path"])?,
        "add_item" => only_keys(m, &["op", "group", "parent", "item", "at"])?,
        "remove_item" => only_keys(m, &["op", "group", "parent", "item"])?,
        "reorder" => only_keys(m, &["op", "group", "parent", "order"])?,
        _ => {}
    }
    Ok(match op.as_str() {
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
    only_keys(m, &["source", "source_version", "mapping_version", "snapshot_ref"])?;
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
    match single_key(m, "origin")? {
        (k, d) if k == "derived" => Ok(Origin::Derived(derivation_from(d)?)),
        (k, o) if k == "overridden" => Ok(Origin::Overridden {
            superseded: match o {
                CanonicalValue::Null => None,
                d => Some(derivation_from(d)?),
            },
        }),
        _ => err("origin must be 'entered', 'derived' or 'overridden'"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Result<[u8; 32], RecordDecodeError> {
    if s.len() != 64 {
        return err("salt must be 32 bytes hex");
    }
    // Lowercase hex digits only — `from_str_radix` would also take a
    // leading `+` and uppercase, giving one salt several spellings.
    if !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return err("salt must be lowercase hex");
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
    // The wire adds `k` and `record` around the entry's own fields.
    only_keys(
        m,
        &[
            "k", "record", "seq", "prev", "actor", "actor_kind", "timestamp", "revision",
            "base_version", "content_hash", "origin", "note", "ops", "meta_salt", "op_salts",
        ],
    )?;
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

#[cfg(test)]
mod tests {
    //! Round-trip is the law (§5): `from(to(x)) == x` over generated ops,
    //! states, scalars, origins and whole entries — and the decoder
    //! refuses every alternative spelling of one value.

    use super::*;
    use proptest::prelude::*;
    use varve_core::canonical::hash_plain;
    use varve_value::CellState;

    // ------------------------------------------------------------ strategies

    fn content_hash() -> impl Strategy<Value = ContentHash> {
        any::<u32>().prop_map(|n| {
            hash_plain(&CanonicalValue::String(n.to_string())).expect("strings never fail")
        })
    }

    fn feature() -> impl Strategy<Value = Feature> {
        (any::<i32>(), any::<i32>(), any::<u16>(), any::<bool>()).prop_map(|(x, y, id, props)| {
            let text = format!(
                r#"{{"type":"Feature","id":{id},"geometry":{{"type":"Point","coordinates":[{}.5,{}]}},"properties":{}}}"#,
                x,
                y,
                if props { r#"{"n":-0.0,"m":1e300,"k":1.5e-7,"s":"é"}"# } else { "null" }
            );
            Feature::parse(&text).unwrap()
        })
    }

    fn scalar() -> impl Strategy<Value = Scalar> {
        prop_oneof![
            "\\PC{0,8}".prop_map(Scalar::Text),
            any::<bool>().prop_map(Scalar::Boolean),
            any::<i64>().prop_map(Scalar::Integer),
            // Fractional and integral, negative and zero, normalized by parse.
            "-?[0-9]{1,12}(\\.[0-9]{1,6})?".prop_map(|s| Scalar::Decimal(Decimal::parse(&s).unwrap())),
            (0i32..=9998, 1u8..=12, 1u8..=28).prop_map(|(y, m, d)| {
                Scalar::Date(Date::parse(&format!("{y:04}-{m:02}-{d:02}")).unwrap())
            }),
            (1970i32..=2100, 1u8..=12, 1u8..=28, 0u8..24, 0u8..60, 0u8..60, proptest::option::of(1u32..1_000_000_000))
                .prop_map(|(y, mo, d, h, mi, s, frac)| {
                    let frac = frac.map(|f| format!(".{f:09}").trim_end_matches('0').to_string()).unwrap_or_default();
                    Scalar::Datetime(
                        Instant::parse(&format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}{frac}Z")).unwrap(),
                    )
                }),
            "[a-z0-9]{1,5}".prop_map(|s| Scalar::Enum(OptionId::new(s))),
            ("[a-z0-9]{1,4}", content_hash(), "\\PC{0,8}", "[a-z]{1,5}/[a-z.+-]{1,8}", 0u64..(1 << 53))
                .prop_map(|(id, hash, filename, content_type, byte_size)| {
                    Scalar::Attachment(Box::new(AttachmentRef { id, hash, filename, content_type, byte_size }))
                }),
            feature().prop_map(|f| Scalar::Geometry(Box::new(f))),
        ]
    }

    fn cell_state() -> impl Strategy<Value = CellState> {
        prop_oneof![
            Just(CellState::Empty),
            scalar().prop_map(|s| CellState::Value(CellValue::One(s))),
            proptest::collection::vec(scalar(), 1..4).prop_map(|v| CellState::Value(CellValue::Many(v))),
        ]
    }

    fn row_path() -> impl Strategy<Value = RowPath> {
        proptest::collection::vec(("[a-z]{1,3}", "[a-z0-9]{1,3}"), 0..3).prop_map(|segs| {
            segs.into_iter().fold(RowPath::root(), |p, (g, i)| {
                p.child(PathSeg { group: GroupId::new(g), item: ItemId::new(i) })
            })
        })
    }

    fn any_op() -> impl Strategy<Value = Op> {
        let column = || "[a-z_]{1,6}".prop_map(ColumnId::new);
        let group = || "[a-z]{1,4}".prop_map(GroupId::new);
        let item = || "[a-z0-9]{1,4}".prop_map(ItemId::new);
        prop_oneof![
            (column(), row_path(), cell_state()).prop_map(|(column, path, state)| Op::Set { column, path, state }),
            (column(), row_path()).prop_map(|(column, path)| Op::Unset { column, path }),
            (group(), row_path(), item(), 0usize..1000)
                .prop_map(|(group, parent, item, at)| Op::AddItem { group, parent, item, at }),
            (group(), row_path(), item()).prop_map(|(group, parent, item)| Op::RemoveItem { group, parent, item }),
            (group(), row_path(), proptest::collection::vec(item(), 0..4))
                .prop_map(|(group, parent, order)| Op::Reorder { group, parent, order }),
        ]
    }

    fn any_derivation() -> impl Strategy<Value = Derivation> {
        ("[a-z-]{1,8}", any::<u32>(), any::<u32>(), content_hash()).prop_map(
            |(source, source_version, mapping_version, snapshot_ref)| Derivation {
                source: ResolverId::new(source),
                source_version,
                mapping_version,
                snapshot_ref,
            },
        )
    }

    fn any_origin() -> impl Strategy<Value = Origin> {
        prop_oneof![
            Just(Origin::Entered),
            any_derivation().prop_map(Origin::Derived),
            Just(Origin::Overridden { superseded: None }),
            any_derivation().prop_map(|d| Origin::Overridden { superseded: Some(d) }),
        ]
    }

    fn salt() -> impl Strategy<Value = Salt> {
        any::<[u8; 32]>().prop_map(Salt)
    }

    fn actor() -> impl Strategy<Value = Actor> {
        (
            "\\PC{1,8}",
            prop_oneof![Just(ActorKind::Human), Just(ActorKind::Resolver), Just(ActorKind::System)],
        )
            .prop_map(|(id, kind)| Actor { id, kind })
    }

    fn any_entry() -> impl Strategy<Value = Entry> {
        (
            proptest::collection::vec(any_op(), 0..4),
            any_origin(),
            proptest::option::of("\\PC{0,12}"),
            salt(),
            any::<u64>(),
            content_hash(),
            actor(),
            (1970i32..=2100, 1u8..=12, 1u8..=28).prop_map(|(y, m, d)| {
                Instant::parse(&format!("{y:04}-{m:02}-{d:02}T10:00:00Z")).unwrap()
            }),
            "[a-z0-9:]{1,12}",
            any::<u64>(),
            content_hash(),
        )
            .prop_flat_map(
                |(ops, origin, note, meta, seq, prev, actor, timestamp, revision, base_version, content_hash)| {
                    let n = ops.len();
                    proptest::collection::vec(salt(), n..=n).prop_map(move |op_salts| Entry {
                        envelope: Envelope {
                            seq: seq >> 11, // JCS-safe
                            prev,
                            actor: actor.clone(),
                            timestamp,
                            revision: RevisionId::new(&revision),
                            base_version: base_version >> 11,
                            content_hash,
                        },
                        content: EntryContent { ops: ops.clone(), origin: origin.clone(), note: note.clone() },
                        salts: EntrySalts { meta, ops: op_salts },
                    })
                },
            )
    }

    // ---------------------------------------------------------------- laws

    proptest! {
        #[test]
        fn scalar_round_trips(s in scalar()) {
            prop_assert_eq!(scalar_from(&scalar_canonical(&s)).unwrap(), s);
        }

        #[test]
        fn state_round_trips(s in cell_state()) {
            prop_assert_eq!(state_from(&state_canonical(&s)).unwrap(), s);
        }

        #[test]
        fn path_round_trips(p in row_path()) {
            prop_assert_eq!(path_from(&path_canonical(&p)).unwrap(), p);
        }

        #[test]
        fn op_round_trips(o in any_op()) {
            prop_assert_eq!(op_from(&op(&o)).unwrap(), o);
        }

        #[test]
        fn origin_round_trips(o in any_origin()) {
            prop_assert_eq!(origin_from(&origin_canonical(&o)).unwrap(), o);
        }

        /// The full history line minus the wire's `k`/`record` wrapper —
        /// and re-encoding the decoded entry gives the same canonical
        /// value (one entry, one text).
        #[test]
        fn entry_round_trips(e in any_entry()) {
            let encoded = entry_canonical(&e);
            let decoded = entry_from(&encoded).unwrap();
            prop_assert_eq!(&decoded, &e);
            prop_assert_eq!(entry_canonical(&decoded), encoded);
        }
    }

    // ----------------------------------------------------------- negatives

    fn s(t: &str) -> CanonicalValue {
        CanonicalValue::String(t.into())
    }

    fn one(kind: &str, v: CanonicalValue) -> CanonicalValue {
        obj(vec![(kind, v)])
    }

    #[test]
    fn scalars_have_one_text_each() {
        // Integers: the normalized i64 rendering only.
        for bad in ["007", "+1", "-0", " 1", "1 ", "1.0", "", "0x10"] {
            assert!(scalar_from(&one("integer", s(bad))).is_err(), "integer {bad:?}");
        }
        for good in ["0", "-1", "9223372036854775807", "-9223372036854775808"] {
            assert!(scalar_from(&one("integer", s(good))).is_ok(), "integer {good:?}");
        }
        // Decimals: what `Decimal::parse` accepts *and* renders back the same.
        for bad in ["1.50", "1.", ".5", "007", "-0", "+1", "1,5", ""] {
            assert!(scalar_from(&one("decimal", s(bad))).is_err(), "decimal {bad:?}");
        }
        for good in ["1.5", "0", "-0.05", "12345678901234567890"] {
            assert!(scalar_from(&one("decimal", s(good))).is_ok(), "decimal {good:?}");
        }
        // Datetimes: the normalized UTC form only.
        assert!(scalar_from(&one("datetime", s("2026-08-16T12:00:00+02:00"))).is_err());
        assert!(scalar_from(&one("datetime", s("2026-08-16T12:00:00+00:00"))).is_err());
        assert!(scalar_from(&one("datetime", s("2026-08-16T12:00:00.500Z"))).is_err());
        assert!(scalar_from(&one("datetime", s("2026-08-16T12:00:00Z"))).is_ok());
        assert!(scalar_from(&one("datetime", s("2026-08-16T12:00:00.5Z"))).is_ok());
        // Dates: strict YYYY-MM-DD.
        assert!(scalar_from(&one("date", s("2026-8-16"))).is_err());
        assert!(scalar_from(&one("date", s("2026-02-30"))).is_err());
        assert!(scalar_from(&one("date", s("2026-08-16"))).is_ok());
        // Wrong JSON type inside a kind.
        assert!(scalar_from(&one("text", CanonicalValue::Int(1))).is_err());
        assert!(scalar_from(&one("boolean", s("true"))).is_err());
        assert!(scalar_from(&one("integer", CanonicalValue::Int(1))).is_err());
        // Tagged union: exactly one known key.
        assert!(scalar_from(&one("float", CanonicalValue::Float(1.0))).is_err());
        assert!(scalar_from(&obj(vec![])).is_err());
        assert!(scalar_from(&obj(vec![("text", s("a")), ("boolean", CanonicalValue::Bool(true))])).is_err());
        assert!(scalar_from(&s("text")).is_err());
        // Geometry must be a Feature.
        assert!(scalar_from(&one("geometry", obj(vec![("type", s("Point")), ("coordinates", CanonicalValue::Array(vec![]))]))).is_err());
    }

    #[test]
    fn attachments_are_decoded_strictly() {
        let hash = hash_plain(&s("x")).unwrap().to_string();
        let att = |extra: Vec<(&str, CanonicalValue)>, size: CanonicalValue, h: &str| {
            let mut pairs = vec![
                ("id", s("f1")),
                ("hash", s(h)),
                ("filename", s("f.pdf")),
                ("content_type", s("application/pdf")),
                ("byte_size", size),
            ];
            pairs.extend(extra);
            one("attachment", obj(pairs))
        };
        assert!(scalar_from(&att(vec![], CanonicalValue::Int(10), &hash)).is_ok());
        assert!(scalar_from(&att(vec![("extra", s("x"))], CanonicalValue::Int(10), &hash)).is_err());
        assert!(scalar_from(&att(vec![], CanonicalValue::Int(-1), &hash)).is_err());
        assert!(scalar_from(&att(vec![], CanonicalValue::Float(10.0), &hash)).is_err());
        assert!(scalar_from(&att(vec![], s("10"), &hash)).is_err());
        assert!(scalar_from(&att(vec![], CanonicalValue::Int(10), "sha256:nothex")).is_err());
        assert!(scalar_from(&att(vec![], CanonicalValue::Int(10), &hash.to_uppercase())).is_err());
        // A missing field.
        let missing = one("attachment", obj(vec![("id", s("f1")), ("hash", s(&hash))]));
        assert!(scalar_from(&missing).is_err());
    }

    #[test]
    fn states_ops_and_origins_are_decoded_strictly() {
        // §2.4 one state, one encoding: `[]` is not a value.
        assert!(state_from(&CanonicalValue::Array(vec![])).is_err());
        assert!(state_from(&CanonicalValue::Bool(true)).is_err());
        assert!(state_from(&s("text")).is_err());
        assert_eq!(state_from(&CanonicalValue::Null).unwrap(), CellState::Empty);

        let path = CanonicalValue::Array(vec![]);
        // Unknown op; extra key; negative index; missing field.
        assert!(op_from(&obj(vec![("op", s("delete")), ("column", s("c")), ("path", path.clone())])).is_err());
        assert!(op_from(&obj(vec![("op", s("unset")), ("column", s("c")), ("path", path.clone()), ("state", CanonicalValue::Null)])).is_err());
        assert!(op_from(&obj(vec![("op", s("add_item")), ("group", s("g")), ("parent", path.clone()), ("item", s("i")), ("at", CanonicalValue::Int(-1))])).is_err());
        assert!(op_from(&obj(vec![("op", s("add_item")), ("group", s("g")), ("parent", path.clone()), ("item", s("i")), ("at", CanonicalValue::Float(0.0))])).is_err());
        assert!(op_from(&obj(vec![("op", s("set")), ("column", s("c")), ("path", path.clone())])).is_err());
        assert!(op_from(&obj(vec![("op", s("reorder")), ("group", s("g")), ("parent", path.clone()), ("order", CanonicalValue::Array(vec![CanonicalValue::Int(1)]))])).is_err());
        // Paths: segments are [group, item] string pairs.
        assert!(path_from(&CanonicalValue::Array(vec![CanonicalValue::Array(vec![s("g")])])).is_err());
        assert!(path_from(&CanonicalValue::Array(vec![s("g/i")])).is_err());
        assert!(path_from(&s("")).is_err());

        // Origins.
        assert!(origin_from(&s("derived")).is_err());
        assert!(origin_from(&s("Entered")).is_err());
        assert!(origin_from(&obj(vec![("entered", CanonicalValue::Null)])).is_err());
        let derivation = |extra: bool| {
            let mut pairs = vec![
                ("source", s("insee")),
                ("source_version", CanonicalValue::Int(1)),
                ("mapping_version", CanonicalValue::Int(1)),
                ("snapshot_ref", s(&hash_plain(&s("x")).unwrap().to_string())),
            ];
            if extra {
                pairs.push(("extra", s("x")));
            }
            obj(pairs)
        };
        assert!(origin_from(&one("derived", derivation(false))).is_ok());
        assert!(origin_from(&one("derived", derivation(true))).is_err());
        assert!(origin_from(&one("overridden", derivation(true))).is_err());
        assert!(origin_from(&one("overridden", CanonicalValue::Null)).is_ok());
        assert!(origin_from(&obj(vec![("derived", derivation(false)), ("overridden", CanonicalValue::Null)])).is_err());
    }

    #[test]
    fn entries_are_decoded_strictly() {
        let hash = hash_plain(&s("x")).unwrap().to_string();
        let salt_hex = "ab".repeat(32);
        let base = || {
            vec![
                ("seq", CanonicalValue::Int(0)),
                ("prev", s(&hash)),
                ("actor", s("a1")),
                ("actor_kind", s("human")),
                ("timestamp", s("2026-08-16T10:00:00Z")),
                ("revision", s("rev-1")),
                ("base_version", CanonicalValue::Int(0)),
                ("content_hash", s(&hash)),
                ("origin", s("entered")),
                ("note", CanonicalValue::Null),
                ("ops", CanonicalValue::Array(vec![])),
                ("meta_salt", s(&salt_hex)),
                ("op_salts", CanonicalValue::Array(vec![])),
            ]
        };
        let with = |edits: Vec<(&str, CanonicalValue)>| {
            let mut pairs = base();
            for (k, v) in edits {
                match pairs.iter_mut().find(|(pk, _)| *pk == k) {
                    Some(slot) => slot.1 = v,
                    None => pairs.push((k, v)),
                }
            }
            obj(pairs)
        };
        assert!(entry_from(&with(vec![])).is_ok());
        // The wire's wrapper keys are tolerated; anything else is not.
        assert!(entry_from(&with(vec![("k", s("entry")), ("record", s("r1"))])).is_ok());
        assert!(entry_from(&with(vec![("extra", s("x"))])).is_err());
        // Salts: 64 lowercase hex digits.
        assert!(entry_from(&with(vec![("meta_salt", s(&salt_hex.to_uppercase()))])).is_err());
        assert!(entry_from(&with(vec![("meta_salt", s(&salt_hex[..62]))])).is_err());
        assert!(entry_from(&with(vec![("meta_salt", s(&"zz".repeat(32)))])).is_err());
        assert!(entry_from(&with(vec![("meta_salt", CanonicalValue::Int(1))])).is_err());
        // One salt per op, both ways.
        let set_op = op(&Op::Set { column: ColumnId::new("c"), path: RowPath::root(), state: CellState::Empty });
        assert!(entry_from(&with(vec![("ops", CanonicalValue::Array(vec![set_op.clone()]))])).is_err());
        assert!(entry_from(&with(vec![("op_salts", CanonicalValue::Array(vec![s(&salt_hex)]))])).is_err());
        assert!(entry_from(&with(vec![
            ("ops", CanonicalValue::Array(vec![set_op])),
            ("op_salts", CanonicalValue::Array(vec![s(&salt_hex)])),
        ]))
        .is_ok());
        // Envelope fields.
        assert!(entry_from(&with(vec![("actor_kind", s("robot"))])).is_err());
        assert!(entry_from(&with(vec![("note", CanonicalValue::Int(1))])).is_err());
        assert!(entry_from(&with(vec![("seq", CanonicalValue::Int(-1))])).is_err());
        assert!(entry_from(&with(vec![("seq", CanonicalValue::Float(0.0))])).is_err());
        assert!(entry_from(&with(vec![("timestamp", s("2026-08-16 10:00:00Z"))])).is_err());
        assert!(entry_from(&with(vec![("prev", s("sha256:short"))])).is_err());
        assert!(entry_from(&with(vec![("content_hash", s(&hash.to_uppercase()))])).is_err());
        assert!(entry_from(&with(vec![("origin", s("derived"))])).is_err());
        // Missing a field.
        let mut pairs = base();
        pairs.retain(|(k, _)| *k != "revision");
        assert!(entry_from(&obj(pairs)).is_err());
    }
}
