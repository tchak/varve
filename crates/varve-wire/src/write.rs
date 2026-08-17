//! The writer: each line is the JCS canonical bytes of its
//! `CanonicalValue` (§2.13, §5 — hash the canonical bytes, never the
//! emitted line; here they are the same bytes).

use varve_core::canonical::{CanonicalError, CanonicalValue, canonical_bytes};
use varve_record::canon as record_canon;
use varve_schema::{option_row_canonical, schema_canonical};

use crate::line::{Intent, Line, Manifest, Mode, RecordLine, obj, string};

pub(crate) fn manifest_canonical(m: &Manifest) -> CanonicalValue {
    obj(vec![
        ("k", string("header")),
        ("format_version", CanonicalValue::Int(i64::from(m.format_version))),
        ("source_instance", string(&m.source_instance)),
        (
            "mode",
            string(match m.mode {
                Mode::History => "history",
                Mode::Snapshot => "snapshot",
            }),
        ),
        (
            "intent",
            string(match m.intent {
                Intent::CreateOnly => "create_only",
                Intent::UpdateOnly => "update_only",
                Intent::Upsert => "upsert",
            }),
        ),
        (
            "revisions",
            CanonicalValue::Array(m.revisions.iter().map(string).collect()),
        ),
        ("record_count", CanonicalValue::Int(m.record_count as i64)),
        (
            "attachments",
            string(if m.attachments_bundled { "bundled" } else { "referenced" }),
        ),
    ])
}

fn record_line_canonical(r: &RecordLine) -> CanonicalValue {
    let cells = CanonicalValue::Array(
        r.values
            .cells
            .iter()
            .map(|(addr, state)| {
                obj(vec![
                    ("column", string(&addr.column)),
                    ("path", record_canon::path_canonical(&addr.path)),
                    ("state", record_canon::state_canonical(state)),
                ])
            })
            .collect(),
    );
    let items = CanonicalValue::Array(
        r.values
            .items
            .iter()
            .map(|(addr, list)| {
                obj(vec![
                    ("group", string(&addr.group)),
                    ("parent", record_canon::path_canonical(&addr.parent)),
                    (
                        "items",
                        CanonicalValue::Array(list.iter().map(string).collect()),
                    ),
                ])
            })
            .collect(),
    );
    obj(vec![
        ("k", string("record")),
        ("id", string(&r.record)),
        ("lens", string(&r.lens)),
        ("cells", cells),
        ("items", items),
    ])
}

pub fn line_canonical(line: &Line) -> CanonicalValue {
    match line {
        Line::Header(m) => manifest_canonical(m),
        Line::Revision { id, schema } => obj(vec![
            ("k", string("revision")),
            ("id", string(id)),
            ("schema", schema_canonical(schema)),
        ]),
        Line::Nomenclature { id, version, rows } => obj(vec![
            ("k", string("nomenclature")),
            ("id", string(id)),
            ("version", CanonicalValue::Int(i64::from(*version))),
            (
                "rows",
                CanonicalValue::Array(rows.iter().map(option_row_canonical).collect()),
            ),
        ]),
        Line::Record(r) => record_line_canonical(r),
        Line::Entry { record, entry } => {
            let mut fields = match record_canon::entry_canonical(entry) {
                CanonicalValue::Object(m) => m,
                _ => unreachable!("entry canonical is an object"),
            };
            fields.insert("k".into(), string("entry"));
            fields.insert("record".into(), string(record));
            CanonicalValue::Object(fields)
        }
        Line::Attachment { hash, byte_size, content_type } => obj(vec![
            ("k", string("attachment")),
            ("hash", string(hash)),
            ("byte_size", CanonicalValue::Int(*byte_size as i64)),
            ("content_type", string(content_type)),
        ]),
    }
}

/// A line whose canonical form is not JCS-representable. Only reachable
/// with unvalidated data — a structural count or size claim beyond
/// 2^53 − 1 (schema validation and conformance bound those) — never
/// with floats, which come only from parsed GeoJSON and are finite.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {error}")]
pub struct WriteError {
    /// 1-based line index in the stream.
    pub line: usize,
    pub error: CanonicalError,
}

/// Serialize lines to JSONL bytes: one canonical JSON object per line,
/// `\n`-terminated. Deterministic by construction.
pub fn write_lines(lines: &[Line]) -> Result<Vec<u8>, WriteError> {
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let bytes = canonical_bytes(&line_canonical(line))
            .map_err(|error| WriteError { line: index + 1, error })?;
        out.extend_from_slice(&bytes);
        out.push(b'\n');
    }
    Ok(out)
}

/// A history export: manifest, schema-side lines, then every record's
/// full log (§5). Lossless and chain-preserving.
pub fn write_history(
    manifest: Manifest,
    schema_lines: Vec<Line>,
    records: &[(varve_core::RecordId, &varve_record::RecordLog)],
) -> Result<Vec<u8>, WriteError> {
    debug_assert_eq!(manifest.mode, Mode::History);
    let mut lines = vec![Line::Header(manifest)];
    lines.extend(schema_lines);
    for (record, log) in records {
        for entry in log.entries() {
            lines.push(Line::Entry { record: record.clone(), entry: entry.clone() });
        }
    }
    write_lines(&lines)
}

/// A snapshot export: manifest, schema-side lines, one folded record
/// per line through the given lens (§5). A patch against the empty
/// state.
pub fn write_snapshot(
    manifest: Manifest,
    schema_lines: Vec<Line>,
    records: Vec<RecordLine>,
) -> Result<Vec<u8>, WriteError> {
    debug_assert_eq!(manifest.mode, Mode::Snapshot);
    let mut lines = vec![Line::Header(manifest)];
    lines.extend(schema_lines);
    lines.extend(records.into_iter().map(Line::Record));
    write_lines(&lines)
}
