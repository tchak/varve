//! The append-only record log: current state is `fold(log)` (§2.9).

use std::collections::BTreeMap;

use varve_core::canonical::ContentHash;
use varve_value::{ApplyError, CellAddr, Op, RecordValues, apply, diff};

use crate::entry::{Draft, Entry, Envelope, genesis_hash};
use crate::Origin;

#[derive(Debug, Clone, Default)]
pub struct RecordLog {
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppendError {
    /// One salt per op plus one for metadata — counts must line up.
    #[error("{ops} ops but {salts} op salts (need one per op)")]
    SaltCount { ops: usize, salts: usize },
    /// `base_version` cannot exceed the log's current version.
    #[error("base_version {base} is ahead of the log (version {version})")]
    BaseVersionAhead { base: u64, version: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainError {
    #[error("entry {at}: seq does not match its position")]
    SeqMismatch { at: usize },
    #[error("entry {at}: prev does not match the preceding entry's hash")]
    PrevMismatch { at: usize },
    #[error("entry {at}: content commitment does not match content + salts")]
    ContentMismatch { at: usize },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("entry {seq}: {error}")]
pub struct FoldError {
    pub seq: u64,
    pub error: ApplyError,
}

/// Folded state plus derived cell provenance: a cell's origin is the
/// origin of the entry that last set it (§2.7).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FoldResult {
    pub values: RecordValues,
    pub provenance: BTreeMap<CellAddr, Origin>,
}

/// Two actors wrote the same cell from the same base: detected and
/// reported, never merged (§2.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub addr: CellAddr,
    pub earlier: u64,
    pub later: u64,
}

/// A folded state pinned to the entry it folds up to — the erasure
/// horizon and performance foothold of §2.9/§2.10.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Number of entries folded (seq of the next entry).
    pub at: u64,
    /// Hash of the last entry folded: what makes the snapshot auditable.
    pub entry_hash: ContentHash,
    pub state: FoldResult,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SnapshotError {
    #[error("snapshot point is outside the log")]
    OutOfRange,
    #[error("snapshot's entry hash does not match the log")]
    HashMismatch,
    #[error("snapshot's state does not match a refold")]
    StateMismatch,
}

impl RecordLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rehydrate a log from stored or transmitted entries — unvalidated:
    /// callers (Tier 5 load, import) must `verify_chain` before trusting
    /// it.
    pub fn from_entries(entries: Vec<Entry>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The log's current version: the seq the next entry will get.
    pub fn version(&self) -> u64 {
        self.entries.len() as u64
    }

    pub fn append(&mut self, draft: Draft) -> Result<&Entry, AppendError> {
        if draft.salts.ops.len() != draft.ops.len() {
            return Err(AppendError::SaltCount {
                ops: draft.ops.len(),
                salts: draft.salts.ops.len(),
            });
        }
        if draft.base_version > self.version() {
            return Err(AppendError::BaseVersionAhead {
                base: draft.base_version,
                version: self.version(),
            });
        }
        let content = crate::EntryContent {
            ops: draft.ops,
            origin: draft.origin,
            note: draft.note,
        };
        let content_hash = Entry::content_hash(&content, &draft.salts);
        let prev = match self.entries.last() {
            None => genesis_hash(),
            Some(entry) => entry.hash(),
        };
        let entry = Entry {
            envelope: Envelope {
                seq: self.version(),
                prev,
                actor: draft.actor,
                timestamp: draft.timestamp,
                revision: draft.revision,
                base_version: draft.base_version,
                content_hash,
            },
            content,
            salts: draft.salts,
        };
        self.entries.push(entry);
        Ok(self.entries.last().expect("just pushed"))
    }

    /// Verify seq contiguity, `prev` linkage, and that every content
    /// commitment matches its content + salts.
    pub fn verify_chain(&self) -> Result<(), ChainError> {
        let mut prev = genesis_hash();
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.envelope.seq != i as u64 {
                return Err(ChainError::SeqMismatch { at: i });
            }
            if entry.envelope.prev != prev {
                return Err(ChainError::PrevMismatch { at: i });
            }
            let recomputed = Entry::content_hash(&entry.content, &entry.salts);
            if recomputed != entry.envelope.content_hash {
                return Err(ChainError::ContentMismatch { at: i });
            }
            prev = entry.hash();
        }
        Ok(())
    }

    /// Fold the first `upto` entries into state + provenance.
    pub fn fold_at(&self, upto: u64) -> Result<FoldResult, FoldError> {
        let mut result = FoldResult::default();
        for entry in self.entries.iter().take(upto as usize) {
            for op in &entry.content.ops {
                apply(&mut result.values, op).map_err(|error| FoldError {
                    seq: entry.envelope.seq,
                    error,
                })?;
                match op {
                    Op::Set { column, path, .. } => {
                        result.provenance.insert(
                            CellAddr {
                                column: column.clone(),
                                path: path.clone(),
                            },
                            entry.content.origin.clone(),
                        );
                    }
                    Op::Unset { column, path } => {
                        result.provenance.remove(&CellAddr {
                            column: column.clone(),
                            path: path.clone(),
                        });
                    }
                    Op::RemoveItem {
                        group,
                        parent,
                        item,
                    } => {
                        let prefix = parent.child(varve_core::PathSeg {
                            group: group.clone(),
                            item: item.clone(),
                        });
                        result
                            .provenance
                            .retain(|addr, _| !addr.path.starts_with(&prefix));
                    }
                    Op::AddItem { .. } | Op::Reorder { .. } => {}
                }
            }
        }
        Ok(result)
    }

    pub fn fold(&self) -> Result<FoldResult, FoldError> {
        self.fold_at(self.version())
    }

    /// "What changed since I last opened this file" (§2.9): the §5 ops
    /// between two log points.
    pub fn diff_between(&self, from: u64, to: u64) -> Result<Vec<Op>, FoldError> {
        let a = self.fold_at(from)?;
        let b = self.fold_at(to)?;
        Ok(diff(&a.values, &b.values))
    }

    /// Same-cell writes from the same base, pairwise: entry B conflicts
    /// with an earlier entry A when A wrote a cell B also writes and
    /// `B.base_version <= A.seq` (B did not see A's write).
    pub fn detect_conflicts(&self) -> Vec<Conflict> {
        fn written(entry: &Entry) -> Vec<CellAddr> {
            entry
                .content
                .ops
                .iter()
                .filter_map(|op| match op {
                    Op::Set { column, path, .. } | Op::Unset { column, path } => {
                        Some(CellAddr {
                            column: column.clone(),
                            path: path.clone(),
                        })
                    }
                    _ => None,
                })
                .collect()
        }
        let mut conflicts = Vec::new();
        for (i, later) in self.entries.iter().enumerate() {
            let later_writes = written(later);
            for earlier in &self.entries[..i] {
                if later.envelope.base_version > earlier.envelope.seq {
                    continue; // later saw earlier's write: ordinary LWW.
                }
                for addr in &later_writes {
                    if written(earlier).contains(addr) {
                        conflicts.push(Conflict {
                            addr: addr.clone(),
                            earlier: earlier.envelope.seq,
                            later: later.envelope.seq,
                        });
                    }
                }
            }
        }
        conflicts
    }

    /// §2.15 GC roots: every blob this record's *entire log* references —
    /// attachment cells in every entry (including superseded values) and
    /// resolver-payload snapshots in origins. The kernel enumerates
    /// roots; the Tier 5 store sweeps. Erasure must cover history, so
    /// this walks the log, not the fold.
    pub fn referenced_blobs(&self) -> std::collections::BTreeSet<
        varve_core::canonical::ContentHash,
    > {
        use varve_value::{CellValue as V, Scalar};
        let mut blobs = std::collections::BTreeSet::new();
        for entry in &self.entries {
            match &entry.content.origin {
                crate::Origin::Derived(d)
                | crate::Origin::Overridden { superseded: Some(d) } => {
                    blobs.insert(d.snapshot_ref);
                }
                _ => {}
            }
            for op in &entry.content.ops {
                if let Op::Set { state, .. } = op
                    && let varve_value::CellState::Value(value) = state
                {
                    let scalars: Vec<&Scalar> = match value {
                        V::One(s) => vec![s],
                        V::Many(list) => list.iter().collect(),
                    };
                    for scalar in scalars {
                        if let Scalar::Attachment(a) = scalar {
                            blobs.insert(a.hash);
                        }
                    }
                }
            }
        }
        blobs
    }

    /// Snapshot at a log point, pinned to the hash of the last entry it
    /// folds — verifiable by refolding (§2.9).
    pub fn snapshot_at(&self, upto: u64) -> Result<Snapshot, SnapshotError> {
        if upto == 0 || upto > self.version() {
            return Err(SnapshotError::OutOfRange);
        }
        let state = self
            .fold_at(upto)
            .map_err(|_| SnapshotError::StateMismatch)?;
        Ok(Snapshot {
            at: upto,
            entry_hash: self.entries[upto as usize - 1].hash(),
            state,
        })
    }

    pub fn verify_snapshot(&self, snapshot: &Snapshot) -> Result<(), SnapshotError> {
        if snapshot.at == 0 || snapshot.at > self.version() {
            return Err(SnapshotError::OutOfRange);
        }
        if self.entries[snapshot.at as usize - 1].hash() != snapshot.entry_hash {
            return Err(SnapshotError::HashMismatch);
        }
        let refolded = self
            .fold_at(snapshot.at)
            .map_err(|_| SnapshotError::StateMismatch)?;
        if refolded != snapshot.state {
            return Err(SnapshotError::StateMismatch);
        }
        Ok(())
    }
}
