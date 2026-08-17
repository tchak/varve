//! Line kinds (§5) and their canonical shapes.

use std::collections::BTreeMap;

use varve_core::canonical::{CanonicalValue, ContentHash};
use varve_core::{ColumnId, GroupId, ItemId, NomenclatureId, PathSeg, RecordId, RevisionId, RowPath};
use varve_record::Entry;
use varve_schema::{OptionRow, Schema};
use varve_value::{CellState, ItemsAddr, RecordValues};

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

/// A snapshot-mode `record` line (§5): the record's **root** cells,
/// keyed by column. Item cells travel on `item` lines that follow it —
/// lines stay bounded however many items a record has.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordLine {
    pub record: RecordId,
    /// The reading revision the fold used — a record is not "on" a
    /// revision (§2.9); this names the lens.
    pub lens: RevisionId,
    /// Root cells: key absent → absent, `null` → empty, value → value.
    pub cells: BTreeMap<ColumnId, CellState>,
}

/// A snapshot-mode `item` line (§5): one item of a `many` group and its
/// cells. **Contiguity rule**: a record's item lines immediately follow
/// its `record` line, parents before children (`parent` names an item
/// already seen, or the root), in list order (`ord` is the position);
/// any other line kind, or the end of the stream, closes the record.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemLine {
    pub record: RecordId,
    pub group: GroupId,
    /// The containing item's path — root at depth 1 (§2.3 keeps this
    /// depth-N ready).
    pub parent: RowPath,
    pub id: ItemId,
    pub ord: usize,
    /// The item's cells: `(column, parent/group:id)`, keyed by column.
    pub cells: BTreeMap<ColumnId, CellState>,
}

/// A whole record in snapshot mode — what a caller hands the writer and
/// what the reader reassembles from `record` + `item` lines
/// (`snapshot_records`).
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotRecord {
    pub record: RecordId,
    pub lens: RevisionId,
    pub values: RecordValues,
}

impl SnapshotRecord {
    /// Explode into physical lines: the `record` line, then `item` lines
    /// parents-first in list order — the contiguity rule, by
    /// construction.
    pub fn lines(&self) -> Vec<Line> {
        let mut cells_at: BTreeMap<&RowPath, BTreeMap<ColumnId, CellState>> = BTreeMap::new();
        for (addr, state) in &self.values.cells {
            cells_at.entry(&addr.path).or_default().insert(addr.column.clone(), state.clone());
        }
        let root = RowPath::root();
        let mut out = vec![Line::Record(RecordLine {
            record: self.record.clone(),
            lens: self.lens.clone(),
            cells: cells_at.remove(&root).unwrap_or_default(),
        })];
        // Parents before children: sort item lists by parent depth, then
        // (parent, group) for determinism.
        let mut lists: Vec<(&ItemsAddr, &Vec<ItemId>)> = self.values.items.iter().collect();
        lists.sort_by_key(|(addr, _)| (addr.parent.depth(), addr.parent.clone(), addr.group.clone()));
        for (addr, ids) in lists {
            for (ord, id) in ids.iter().enumerate() {
                let path = addr.parent.child(PathSeg { group: addr.group.clone(), item: id.clone() });
                out.push(Line::Item(ItemLine {
                    record: self.record.clone(),
                    group: addr.group.clone(),
                    parent: addr.parent.clone(),
                    id: id.clone(),
                    ord,
                    cells: cells_at.remove(&path).unwrap_or_default(),
                }));
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Header(Manifest),
    Revision { id: RevisionId, schema: Schema },
    Nomenclature { id: NomenclatureId, version: u32, rows: Vec<OptionRow> },
    Record(RecordLine),
    Item(ItemLine),
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
