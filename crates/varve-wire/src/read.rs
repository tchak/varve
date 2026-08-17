//! The reader: parse JSONL into typed lines. Fails fast on line 1 (§5)
//! and rejects mode-inconsistent streams (a snapshot stream carrying
//! `entry` lines, or vice versa — two sources of truth for the same
//! cells). Strict and total: every malformation is a `ReadError`. This
//! is where untrusted bytes enter the kernel — the fuzz target.

use std::collections::BTreeMap;

use varve_core::canonical::{CanonicalValue, ContentHash, MAX_SAFE_INTEGER};
use varve_core::{ColumnId, GroupId, ItemId, NomenclatureId, PathSeg, RecordId, RevisionId, RowPath};
use varve_record::canon as record_canon;
use varve_schema::{block_from_canonical, option_row_from_canonical, schema_from_canonical};
use varve_value::{CellAddr, CellState, ItemsAddr, RecordValues};

use crate::line::{Intent, ItemLine, Line, Manifest, Mode, RecordLine, SnapshotRecord};
use crate::FORMAT_VERSION;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ReadError {
    #[error("line {line}: not valid JSON")]
    Json { line: usize },
    #[error("line {line}: {reason}")]
    Malformed { line: usize, reason: String },
    #[error("line 1 must be a header")]
    MissingHeader,
    #[error("unsupported format version {0} (this reader speaks {FORMAT_VERSION})")]
    UnsupportedVersion(u32),
    /// §5: history and snapshot lines never mix.
    #[error("line {line}: '{kind}' line in a {mode:?} stream")]
    ModeMismatch { line: usize, kind: String, mode: Mode },
    #[error("stream ends without the {expected} records the manifest declared (got {got})")]
    RecordCountMismatch { expected: u64, got: u64 },
}

/// A parsed stream, validated for structure (not yet applied).
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub manifest: Manifest,
    pub lines: Vec<Line>,
}

pub fn read_stream(bytes: &[u8]) -> Result<Stream, ReadError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ReadError::Malformed { line: 1, reason: "not UTF-8".into() })?;
    let mut manifest: Option<Manifest> = None;
    let mut lines = Vec::new();
    let mut records_seen: std::collections::BTreeSet<RecordId> = Default::default();
    // Snapshot contiguity (§5): the record whose item lines may follow,
    // the item paths seen so far, and per-(group, parent) item counts
    // so `ord` is checked in sequence.
    let mut open: Option<OpenRecord> = None;

    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let malformed = |reason: String| ReadError::Malformed { line: line_no, reason };
        let value = match serde_json::from_str::<JsonLine>(raw) {
            Ok(JsonLine(v)) => v,
            Err(e) if e.classify() == serde_json::error::Category::Data => {
                // Well-formed JSON the visitor refused: a duplicate key.
                return Err(malformed(e.to_string()));
            }
            Err(_) => return Err(ReadError::Json { line: line_no }),
        };

        let map = match &value {
            CanonicalValue::Object(m) => m,
            _ => return Err(malformed("line must be a JSON object".into())),
        };
        let kind = match map.get("k") {
            Some(CanonicalValue::String(k)) => k.as_str(),
            _ => return Err(malformed("missing 'k'".into())),
        };

        if manifest.is_none() {
            if kind != "header" {
                return Err(ReadError::MissingHeader);
            }
            let m = manifest_from(map).map_err(malformed)?;
            if m.format_version != FORMAT_VERSION {
                return Err(ReadError::UnsupportedVersion(m.format_version));
            }
            manifest = Some(m.clone());
            lines.push(Line::Header(m));
            continue;
        }
        let mode = manifest.as_ref().expect("set above").mode;
        if kind != "item" {
            open = None; // Any other line kind closes the open record (§5).
        }

        let line = match kind {
            "header" => return Err(malformed("duplicate header".into())),
            "revision" => Line::Revision {
                id: RevisionId::new(get_str(map, "id").map_err(malformed)?),
                schema: schema_from_canonical(get(map, "schema").map_err(malformed)?)
                    .map_err(|e| malformed(e.to_string()))?,
            },
            "nomenclature" => Line::Nomenclature {
                id: NomenclatureId::new(get_str(map, "id").map_err(malformed)?),
                version: get_u32(map, "version").map_err(malformed)?,
                rows: as_arr(get(map, "rows").map_err(malformed)?)
                    .map_err(malformed)?
                    .iter()
                    .map(option_row_from_canonical)
                    .collect::<Result<_, _>>()
                    .map_err(|e| malformed(e.to_string()))?,
            },
            "block" => Line::Block(block_from_canonical(&value).map_err(|e| malformed(e.to_string()))?),
            "attachment" => Line::Attachment {
                hash: get_str(map, "hash")
                    .map_err(malformed)?
                    .parse::<ContentHash>()
                    .map_err(|_| malformed("bad hash".into()))?,
                byte_size: get_u64(map, "byte_size").map_err(malformed)?,
                content_type: get_str(map, "content_type").map_err(malformed)?,
            },
            "record" => {
                if mode != Mode::Snapshot {
                    return Err(ReadError::ModeMismatch {
                        line: line_no,
                        kind: kind.into(),
                        mode,
                    });
                }
                let r = record_line_from(map).map_err(malformed)?;
                // A stream is authoritative for each record it contains
                // exactly once (§5) — a second `record` line for one id
                // is malformed, never a second version.
                if !records_seen.insert(r.record.clone()) {
                    return Err(malformed(format!("duplicate record '{}'", r.record)));
                }
                open = Some(OpenRecord {
                    record: r.record.clone(),
                    paths: Default::default(),
                    counts: Default::default(),
                });
                Line::Record(r)
            }
            "item" => {
                if mode != Mode::Snapshot {
                    return Err(ReadError::ModeMismatch {
                        line: line_no,
                        kind: kind.into(),
                        mode,
                    });
                }
                let i = item_line_from(map).map_err(malformed)?;
                let Some(current) = open.as_mut().filter(|o| o.record == i.record) else {
                    return Err(malformed(format!(
                        "item line for record '{}' outside that record's lines",
                        i.record
                    )));
                };
                if i.parent.depth() != 0 && !current.paths.contains(&i.parent) {
                    return Err(malformed("item's parent is not an item seen for this record".into()));
                }
                let count = current.counts.entry((i.group.clone(), i.parent.clone())).or_insert(0);
                if i.ord != *count {
                    return Err(malformed(format!("item ord {} out of sequence (expected {count})", i.ord)));
                }
                *count += 1;
                let path = i.parent.child(PathSeg { group: i.group.clone(), item: i.id.clone() });
                if !current.paths.insert(path) {
                    return Err(malformed(format!("duplicate item '{}' in group '{}'", i.id, i.group)));
                }
                Line::Item(i)
            }
            "entry" => {
                if mode != Mode::History {
                    return Err(ReadError::ModeMismatch {
                        line: line_no,
                        kind: kind.into(),
                        mode,
                    });
                }
                let record = RecordId::new(get_str(map, "record").map_err(malformed)?);
                let entry = record_canon::entry_from(&value)
                    .map_err(|e| malformed(e.to_string()))?;
                records_seen.insert(record.clone());
                Line::Entry { record, entry }
            }
            other => return Err(malformed(format!("unknown line kind '{other}'"))),
        };
        lines.push(line);
    }

    let manifest = manifest.ok_or(ReadError::MissingHeader)?;
    let got = records_seen.len() as u64;
    if got != manifest.record_count {
        return Err(ReadError::RecordCountMismatch { expected: manifest.record_count, got });
    }
    Ok(Stream { manifest, lines })
}

type Obj = BTreeMap<String, CanonicalValue>;

/// A JSON line parsed straight into a canonical value, under JCS number
/// semantics: every JSON number denotes a double. An integer literal
/// within ±(2^53 − 1) is an `Int` (structural counts); anything else —
/// fractional, exponent, or an integer literal too large for exact
/// representation (ES6 renders doubles below 1e21 without an exponent,
/// so `5752289928800135000` is a legitimate coordinate) — is the
/// `Float` it denotes. Decoders that expect a count reject a `Float`,
/// so no unsafe integer ever reaches a hash; exact integers travel as
/// strings. **Duplicate object keys are refused**: a JCS serializer
/// never emits them, so a line carrying two values for one key is
/// malformed — never last-wins.
struct JsonLine(CanonicalValue);

impl<'de> serde::Deserialize<'de> for JsonLine {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(JsonVisitor).map(JsonLine)
    }
}

struct JsonVisitor;

impl<'de> serde::de::Visitor<'de> for JsonVisitor {
    type Value = CanonicalValue;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_unit<E>(self) -> Result<CanonicalValue, E> {
        Ok(CanonicalValue::Null)
    }
    fn visit_bool<E>(self, b: bool) -> Result<CanonicalValue, E> {
        Ok(CanonicalValue::Bool(b))
    }
    fn visit_i64<E>(self, i: i64) -> Result<CanonicalValue, E> {
        Ok(if i.unsigned_abs() <= MAX_SAFE_INTEGER as u64 {
            CanonicalValue::Int(i)
        } else {
            CanonicalValue::Float(i as f64)
        })
    }
    fn visit_u64<E>(self, u: u64) -> Result<CanonicalValue, E> {
        Ok(if u <= MAX_SAFE_INTEGER as u64 {
            CanonicalValue::Int(u as i64)
        } else {
            CanonicalValue::Float(u as f64)
        })
    }
    fn visit_f64<E>(self, f: f64) -> Result<CanonicalValue, E> {
        Ok(CanonicalValue::Float(f))
    }
    fn visit_str<E>(self, s: &str) -> Result<CanonicalValue, E> {
        Ok(CanonicalValue::String(s.to_string()))
    }
    fn visit_string<E>(self, s: String) -> Result<CanonicalValue, E> {
        Ok(CanonicalValue::String(s))
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<CanonicalValue, A::Error> {
        let mut items = Vec::new();
        while let Some(JsonLine(v)) = seq.next_element()? {
            items.push(v);
        }
        Ok(CanonicalValue::Array(items))
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<CanonicalValue, A::Error> {
        let mut out = BTreeMap::new();
        while let Some((key, JsonLine(v))) = map.next_entry::<String, JsonLine>()? {
            if out.insert(key.clone(), v).is_some() {
                return Err(serde::de::Error::custom(format!("duplicate key '{key}'")));
            }
        }
        Ok(CanonicalValue::Object(out))
    }
}

fn get<'a>(m: &'a Obj, key: &str) -> Result<&'a CanonicalValue, String> {
    m.get(key).ok_or_else(|| format!("missing '{key}'"))
}

fn get_str(m: &Obj, key: &str) -> Result<String, String> {
    match m.get(key) {
        Some(CanonicalValue::String(s)) => Ok(s.clone()),
        _ => Err(format!("missing string '{key}'")),
    }
}

fn get_u64(m: &Obj, key: &str) -> Result<u64, String> {
    match m.get(key) {
        Some(CanonicalValue::Int(i)) if *i >= 0 => Ok(*i as u64),
        _ => Err(format!("missing non-negative integer '{key}'")),
    }
}

fn get_u32(m: &Obj, key: &str) -> Result<u32, String> {
    u32::try_from(get_u64(m, key)?).map_err(|_| format!("'{key}' out of range"))
}

fn as_arr(v: &CanonicalValue) -> Result<&[CanonicalValue], String> {
    match v {
        CanonicalValue::Array(a) => Ok(a),
        _ => Err("expected an array".into()),
    }
}

fn as_obj(v: &CanonicalValue) -> Result<&Obj, String> {
    match v {
        CanonicalValue::Object(m) => Ok(m),
        _ => Err("expected an object".into()),
    }
}

fn manifest_from(m: &Obj) -> Result<Manifest, String> {
    Ok(Manifest {
        format_version: get_u32(m, "format_version")?,
        source_instance: get_str(m, "source_instance")?,
        mode: match get_str(m, "mode")?.as_str() {
            "history" => Mode::History,
            "snapshot" => Mode::Snapshot,
            other => return Err(format!("unknown mode '{other}'")),
        },
        intent: match get_str(m, "intent")?.as_str() {
            "create_only" => Intent::CreateOnly,
            "update_only" => Intent::UpdateOnly,
            "upsert" => Intent::Upsert,
            other => return Err(format!("unknown intent '{other}'")),
        },
        revisions: as_arr(get(m, "revisions")?)?
            .iter()
            .map(|r| match r {
                CanonicalValue::String(s) => Ok(RevisionId::new(s)),
                _ => Err("revision ids must be strings".to_string()),
            })
            .collect::<Result<_, _>>()?,
        record_count: get_u64(m, "record_count")?,
        attachments_bundled: match get_str(m, "attachments")?.as_str() {
            "bundled" => true,
            "referenced" => false,
            other => return Err(format!("unknown attachments mode '{other}'")),
        },
    })
}

struct OpenRecord {
    record: RecordId,
    paths: std::collections::BTreeSet<RowPath>,
    counts: BTreeMap<(GroupId, RowPath), usize>,
}

fn cells_from(m: &Obj) -> Result<BTreeMap<ColumnId, CellState>, String> {
    as_obj(get(m, "cells")?)?
        .iter()
        .map(|(column, state)| {
            Ok((
                ColumnId::new(column),
                record_canon::state_from(state).map_err(|e| e.to_string())?,
            ))
        })
        .collect()
}

fn record_line_from(m: &Obj) -> Result<RecordLine, String> {
    Ok(RecordLine {
        record: RecordId::new(get_str(m, "id")?),
        lens: RevisionId::new(get_str(m, "lens")?),
        cells: cells_from(m)?,
    })
}

fn item_line_from(m: &Obj) -> Result<ItemLine, String> {
    Ok(ItemLine {
        record: RecordId::new(get_str(m, "record")?),
        group: GroupId::new(get_str(m, "group")?),
        parent: record_canon::path_from(get(m, "parent")?).map_err(|e| e.to_string())?,
        id: ItemId::new(get_str(m, "id")?),
        ord: get_u64(m, "ord")? as usize,
        cells: cells_from(m)?,
    })
}

/// Reassemble the snapshot records of a read stream from their `record`
/// and `item` lines (contiguity was verified on read).
pub fn snapshot_records(stream: &Stream) -> Vec<SnapshotRecord> {
    let mut out: Vec<SnapshotRecord> = Vec::new();
    for line in &stream.lines {
        match line {
            Line::Record(r) => {
                let mut values = RecordValues::new();
                for (column, state) in &r.cells {
                    values.cells.insert(
                        CellAddr { column: column.clone(), path: RowPath::root() },
                        state.clone(),
                    );
                }
                out.push(SnapshotRecord { record: r.record.clone(), lens: r.lens.clone(), values });
            }
            Line::Item(i) => {
                let Some(current) = out.last_mut().filter(|c| c.record == i.record) else {
                    continue; // unreachable after read_stream's contiguity check
                };
                let path = i.parent.child(PathSeg { group: i.group.clone(), item: i.id.clone() });
                current
                    .values
                    .items
                    .entry(ItemsAddr { group: i.group.clone(), parent: i.parent.clone() })
                    .or_default()
                    .push(i.id.clone());
                for (column, state) in &i.cells {
                    current
                        .values
                        .cells
                        .insert(CellAddr { column: column.clone(), path: path.clone() }, state.clone());
                }
            }
            _ => {}
        }
    }
    out
}
