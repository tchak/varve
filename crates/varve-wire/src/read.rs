//! The reader: parse JSONL into typed lines. Fails fast on line 1 (§5)
//! and rejects mode-inconsistent streams (a snapshot stream carrying
//! `entry` lines, or vice versa — two sources of truth for the same
//! cells). Strict and total: every malformation is a `ReadError`. This
//! is where untrusted bytes enter the kernel — the fuzz target.

use std::collections::BTreeMap;

use varve_core::canonical::{CanonicalValue, ContentHash, MAX_SAFE_INTEGER};
use varve_core::{ColumnId, GroupId, ItemId, NomenclatureId, RecordId, RevisionId};
use varve_record::canon as record_canon;
use varve_schema::{option_row_from_canonical, schema_from_canonical};
use varve_value::{CellAddr, ItemsAddr, RecordValues};

use crate::line::{Intent, Line, Manifest, Mode, RecordLine};
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

    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let json: serde_json::Value =
            serde_json::from_str(raw).map_err(|_| ReadError::Json { line: line_no })?;
        let value = to_canonical(&json);
        let malformed = |reason: String| ReadError::Malformed { line: line_no, reason };

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
                records_seen.insert(r.record.clone());
                Line::Record(r)
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

/// JSON tree → canonical value, under JCS number semantics: every JSON
/// number denotes a double. An integer literal within ±(2^53 − 1) is an
/// `Int` (structural counts); anything else — fractional, exponent, or
/// an integer literal too large for exact representation (ES6 renders
/// doubles below 1e21 without an exponent, so `5752289928800135000` is
/// a legitimate coordinate) — is the `Float` it denotes. Decoders that
/// expect a count reject a `Float`, so no unsafe integer ever reaches
/// a hash; exact integers travel as strings.
fn to_canonical(v: &serde_json::Value) -> CanonicalValue {
    match v {
        serde_json::Value::Null => CanonicalValue::Null,
        serde_json::Value::Bool(b) => CanonicalValue::Bool(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) if i.unsigned_abs() <= MAX_SAFE_INTEGER as u64 => CanonicalValue::Int(i),
            _ => CanonicalValue::Float(n.as_f64().expect("JSON numbers are finite")),
        },
        serde_json::Value::String(s) => CanonicalValue::String(s.clone()),
        serde_json::Value::Array(a) => CanonicalValue::Array(a.iter().map(to_canonical).collect()),
        serde_json::Value::Object(o) => CanonicalValue::Object(
            o.iter().map(|(k, v)| (k.clone(), to_canonical(v))).collect(),
        ),
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

fn record_line_from(m: &Obj) -> Result<RecordLine, String> {
    let mut values = RecordValues::new();
    for cell in as_arr(get(m, "cells")?)? {
        let cell = as_obj(cell)?;
        values.cells.insert(
            CellAddr {
                column: ColumnId::new(get_str(cell, "column")?),
                path: record_canon::path_from(get(cell, "path")?).map_err(|e| e.to_string())?,
            },
            record_canon::state_from(get(cell, "state")?).map_err(|e| e.to_string())?,
        );
    }
    for list in as_arr(get(m, "items")?)? {
        let list = as_obj(list)?;
        let ids: Vec<ItemId> = as_arr(get(list, "items")?)?
            .iter()
            .map(|i| match i {
                CanonicalValue::String(s) => Ok(ItemId::new(s)),
                _ => Err("item ids must be strings".to_string()),
            })
            .collect::<Result<_, _>>()?;
        if ids.is_empty() {
            // One state, one encoding (§2.4): a group with no items has
            // no item list.
            return Err("an empty item list is not stored — omit it".to_string());
        }
        values.items.insert(
            ItemsAddr {
                group: GroupId::new(get_str(list, "group")?),
                parent: record_canon::path_from(get(list, "parent")?)
                    .map_err(|e| e.to_string())?,
            },
            ids,
        );
    }
    Ok(RecordLine {
        record: RecordId::new(get_str(m, "id")?),
        lens: RevisionId::new(get_str(m, "lens")?),
        values,
    })
}
