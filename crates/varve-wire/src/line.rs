//! Line kinds (§5) and their canonical shapes.

use std::collections::BTreeMap;

use varve_core::canonical::{CanonicalValue, ContentHash};
use varve_core::{NomenclatureId, RecordId, RevisionId};
use varve_record::Entry;
use varve_schema::{OptionRow, Schema};
use varve_value::RecordValues;

/// Which export/import mode a stream is (§5): a stream kind, never a
/// flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `entry` lines: lossless, chain-preserving — migration.
    History,
    /// `record` cell lines through a reading lens: whole-record replace.
    Snapshot,
}

/// Record-level intent (§5 import modes): id mismatches fail loudly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    CreateOnly,
    UpdateOnly,
    Upsert,
}

/// Line 1: everything needed to fail fast (§5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub format_version: u32,
    pub source_instance: String,
    pub mode: Mode,
    pub intent: Intent,
    pub revisions: Vec<RevisionId>,
    pub record_count: u64,
    /// `referenced` (blobs by hash, not included) or `bundled` (a
    /// sidecar archive keyed by hash accompanies the stream — §2.15).
    pub attachments_bundled: bool,
}

/// A snapshot-mode record: folded cells (root and items alike are
/// carried in `values`; the reader keeps lines bounded by emitting one
/// `record` line per record — item lines follow (§5) when a record's
/// cell count exceeds the line budget; v1 emits one line per record and
/// relies on the writer's line-size guard).
#[derive(Debug, Clone, PartialEq)]
pub struct RecordLine {
    pub record: RecordId,
    /// The reading revision the fold used — a record is not "on" a
    /// revision (§2.9); this names the lens.
    pub lens: RevisionId,
    pub values: RecordValues,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Header(Manifest),
    Revision { id: RevisionId, schema: Schema },
    Nomenclature { id: NomenclatureId, version: u32, rows: Vec<OptionRow> },
    Record(RecordLine),
    Entry { record: RecordId, entry: Entry },
    /// Describes a blob (§2.15): hash, size, type. Filenames stay in
    /// cells.
    Attachment { hash: ContentHash, byte_size: u64, content_type: String },
}

pub(crate) fn obj(pairs: Vec<(&str, CanonicalValue)>) -> CanonicalValue {
    CanonicalValue::Object(
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<BTreeMap<_, _>>(),
    )
}

pub(crate) fn string(s: impl ToString) -> CanonicalValue {
    CanonicalValue::String(s.to_string())
}
