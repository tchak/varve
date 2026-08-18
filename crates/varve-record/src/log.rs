//! The append-only record log: current state is `fold(log)` (§2.9).

use std::collections::BTreeMap;

use varve_core::RecordId;
use varve_core::canonical::ContentHash;
use varve_value::{ApplyError, CellAddr, Op, RecordValues, apply, diff};

use crate::entry::{Draft, Entry, Envelope, SaltCountMismatch, genesis_hash};
use crate::Origin;

#[derive(Debug, Clone)]
pub struct RecordLog {
    /// The record this log belongs to: the chain's genesis commits to
    /// it (§2.9), so entries verify only under this id.
    record: RecordId,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AppendError {
    /// One salt per op plus one for metadata — counts must line up.
    #[error(transparent)]
    SaltCount(#[from] SaltCountMismatch),
    /// `base_version` cannot exceed the log's current version.
    #[error("base_version {base} is ahead of the log (version {version})")]
    BaseVersionAhead { base: u64, version: u64 },
    /// The draft's ops do not apply to the current state (an item that
    /// no longer exists, a duplicate item, a bad index …). Refused at
    /// append: an entry the fold cannot apply would poison the log —
    /// every later fold would fail forever. Detected, not merged (§2.9):
    /// the platform reports the conflict and lets the actor retry.
    #[error("ops do not apply to the current state: {0}")]
    DoesNotApply(ApplyError),
    /// The log as loaded does not fold (a poisoned or tampered log);
    /// nothing can be appended until it is repaired.
    #[error("the log does not fold: {0}")]
    Unfoldable(FoldError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChainError {
    #[error("entry {at}: seq does not match its position")]
    SeqMismatch { at: usize },
    #[error("entry {at}: prev does not match the preceding entry's hash")]
    PrevMismatch { at: usize },
    #[error("entry {at}: content commitment does not match content + salts")]
    ContentMismatch { at: usize },
    /// An op without a salt (or vice versa) can never have been
    /// committed: the entry is malformed, whatever its hash says.
    #[error("entry {at}: {mismatch}")]
    SaltCount { at: usize, mismatch: SaltCountMismatch },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FoldError {
    /// Entry `seq`'s ops do not apply — a poisoned or tampered log.
    #[error("entry {seq}: {error}")]
    Apply { seq: u64, error: ApplyError },
    /// A log point past the end: `upto` entries were asked for, the log
    /// has `version`. Never clamped silently — "the state at entry 40"
    /// of a 30-entry log is a caller error, not the head.
    #[error("log point {upto} is past the log's version {version}")]
    OutOfRange { upto: u64, version: u64 },
}

/// Folded state plus derived cell provenance (§2.7). Provenance is
/// *derived*, not copied: a human write over a derived cell becomes
/// `overridden { superseded }` whether or not the entry said so, and a
/// resolver's late derived write never clobbers a human-authored cell
/// (§2.8 rule 2) — it is recorded in `suppressed` and its derivation
/// lands on the cell as `superseded`, so divergence is a cell-local
/// read and restore is one `set` away.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FoldResult {
    pub values: RecordValues,
    pub provenance: BTreeMap<CellAddr, Origin>,
    /// Late machine writes the fold refused to apply (§2.8 rule 2):
    /// visible, never silent.
    pub suppressed: Vec<Suppressed>,
}

/// A resolver's derived write that landed on a human-authored cell and
/// was not applied (§2.8 rule 2: override wins over late resolution).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suppressed {
    pub seq: u64,
    pub addr: CellAddr,
}

/// Two actors wrote the same cell from the same base: detected and
/// reported, never merged (§2.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub addr: CellAddr,
    pub earlier: u64,
    pub later: u64,
}

/// A folded state pinned to the entry it folds up to: the performance
/// foothold of §2.9, and what a §2.10 erasure horizon will be once a
/// fold can start from a snapshot instead of empty (not built — see
/// §10 Q11's residual).
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
    /// The log itself does not fold up to the snapshot point.
    #[error("the log does not fold: {0}")]
    Unfoldable(FoldError),
}

/// Cell provenance after a write with `origin` lands on a cell whose
/// current provenance is `current` (§2.7): a human write over a
/// derived value retains what it replaced; an explicit `overridden`
/// without `superseded` picks it up from the cell; everything else is
/// the entry's origin verbatim.
fn derive_provenance(origin: &Origin, current: Option<&Origin>) -> Origin {
    let replaced = match current {
        Some(Origin::Derived(d)) => Some(d.clone()),
        Some(Origin::Overridden { superseded }) => superseded.clone(),
        _ => None,
    };
    match origin {
        Origin::Entered => match replaced {
            Some(d) => Origin::Overridden { superseded: Some(d) },
            None if matches!(current, Some(Origin::Overridden { .. })) => {
                Origin::Overridden { superseded: None }
            }
            None => Origin::Entered,
        },
        Origin::Overridden { superseded: None } => Origin::Overridden { superseded: replaced },
        other => other.clone(),
    }
}

/// Fold one entry into `result`: apply its ops with the §2.8 rule 2
/// guard, and derive provenance (§2.7). The single implementation used
/// by `fold_at` and, on the candidate entry, by `append`.
fn fold_entry(result: &mut FoldResult, entry: &Entry) -> Result<(), FoldError> {
    let seq = entry.envelope.seq;
    let origin = &entry.content.origin;
    let late_machine_write = entry.envelope.actor.kind == crate::ActorKind::Resolver
        && matches!(origin, Origin::Derived(_));
    for op in &entry.content.ops {
        // §2.8 rule 2, enforced where provenance is derived: a
        // resolver's derived write onto a human-authored cell is not
        // applied; the cell keeps its value and gains the late
        // derivation as `superseded` (divergence visible, restore
        // possible), and the write is reported.
        if let Op::Set { column, path, .. } | Op::Unset { column, path } = op
            && late_machine_write
        {
            let addr = CellAddr { column: column.clone(), path: path.clone() };
            if let Some(current) = result.provenance.get(&addr)
                && matches!(current, Origin::Entered | Origin::Overridden { .. })
            {
                let landed = match origin {
                    Origin::Derived(d) => Some(d.clone()),
                    _ => None,
                };
                let superseded = match current {
                    Origin::Overridden { superseded: Some(d) } => Some(d.clone()),
                    _ => landed,
                };
                result.provenance.insert(addr.clone(), Origin::Overridden { superseded });
                result.suppressed.push(Suppressed { seq, addr });
                continue;
            }
        }
        apply(&mut result.values, op).map_err(|error| FoldError::Apply { seq, error })?;
        match op {
            Op::Set { column, path, .. } => {
                let addr = CellAddr { column: column.clone(), path: path.clone() };
                let derived = derive_provenance(origin, result.provenance.get(&addr));
                result.provenance.insert(addr, derived);
            }
            Op::Unset { column, path } => {
                result.provenance.remove(&CellAddr { column: column.clone(), path: path.clone() });
            }
            Op::RemoveItem { group, parent, item } => {
                let prefix =
                    parent.child(varve_core::PathSeg { group: group.clone(), item: item.clone() });
                result.provenance.retain(|addr, _| !addr.path.starts_with(&prefix));
            }
            Op::AddItem { .. } | Op::Reorder { .. } => {}
        }
    }
    Ok(())
}

impl RecordLog {
    /// An empty log for `record`.
    pub fn new(record: RecordId) -> Self {
        Self { record, entries: Vec::new() }
    }

    /// Rehydrate `record`'s log from stored or transmitted entries —
    /// unvalidated: callers (Tier 5 load, import) must `verify_chain`
    /// before trusting it. Entries transplanted from another record
    /// fail there: the genesis commits to the id.
    pub fn from_entries(record: RecordId, entries: Vec<Entry>) -> Self {
        Self { record, entries }
    }

    pub fn record(&self) -> &RecordId {
        &self.record
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The log's current version: the seq the next entry will get.
    pub fn version(&self) -> u64 {
        self.entries.len() as u64
    }

    pub fn append(&mut self, draft: Draft) -> Result<&Entry, AppendError> {
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
        let content_hash = Entry::content_hash(&content, &draft.salts)?;
        let prev = match self.entries.last() {
            None => genesis_hash(&self.record),
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
        // The entry must apply to the current state — through the very
        // fold that will read it back — or the log would be poisoned.
        let mut state = self.fold().map_err(AppendError::Unfoldable)?;
        fold_entry(&mut state, &entry).map_err(|e| match e {
            FoldError::Apply { error, .. } => AppendError::DoesNotApply(error),
            FoldError::OutOfRange { .. } => unreachable!("fold_entry never ranges"),
        })?;
        self.entries.push(entry);
        Ok(self.entries.last().expect("just pushed"))
    }

    /// Verify seq contiguity, `prev` linkage, that every op carries a
    /// salt, and that every content commitment matches its content +
    /// salts. The tail is unanchored by construction: truncation and
    /// tail edits are caught only by a checkpoint or snapshot that pins
    /// the head hash (§2.9).
    pub fn verify_chain(&self) -> Result<(), ChainError> {
        let mut prev = genesis_hash(&self.record);
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.envelope.seq != i as u64 {
                return Err(ChainError::SeqMismatch { at: i });
            }
            if entry.envelope.prev != prev {
                return Err(ChainError::PrevMismatch { at: i });
            }
            let recomputed = Entry::content_hash(&entry.content, &entry.salts)
                .map_err(|mismatch| ChainError::SaltCount { at: i, mismatch })?;
            if recomputed != entry.envelope.content_hash {
                return Err(ChainError::ContentMismatch { at: i });
            }
            prev = entry.hash();
        }
        Ok(())
    }

    /// Fold the first `upto` entries into state + provenance.
    pub fn fold_at(&self, upto: u64) -> Result<FoldResult, FoldError> {
        if upto > self.version() {
            return Err(FoldError::OutOfRange { upto, version: self.version() });
        }
        let mut result = FoldResult::default();
        for entry in self.entries.iter().take(upto as usize) {
            fold_entry(&mut result, entry)?;
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

    /// Same-cell writes by two actors from the same base (§2.9: detect,
    /// do not merge): entry B conflicts with an earlier entry A by another
    /// actor when A wrote a cell B also writes and `B.base_version <=
    /// A.seq` (B did not see A's write). One forward pass with a per-cell
    /// write history — linear in the ops of the log.
    pub fn detect_conflicts(&self) -> Vec<Conflict> {
        // Per cell: the entries that wrote it, in seq order.
        let mut writes: BTreeMap<CellAddr, Vec<usize>> = BTreeMap::new();
        let mut conflicts = Vec::new();
        for (i, later) in self.entries.iter().enumerate() {
            let base = later.envelope.base_version;
            let mut touched: Vec<CellAddr> = Vec::new();
            for op in &later.content.ops {
                let (Op::Set { column, path, .. } | Op::Unset { column, path }) = op else { continue };
                let addr = CellAddr { column: column.clone(), path: path.clone() };
                if let Some(history) = writes.get(&addr) {
                    // Writes `later` did not see: seq >= base. History is
                    // in seq order, so walk back from the end.
                    for &j in history.iter().rev() {
                        let earlier = &self.entries[j];
                        if earlier.envelope.seq < base {
                            break; // seen: ordinary LWW from here down.
                        }
                        if earlier.envelope.actor.id != later.envelope.actor.id {
                            conflicts.push(Conflict {
                                addr: addr.clone(),
                                earlier: earlier.envelope.seq,
                                later: later.envelope.seq,
                            });
                        }
                    }
                }
                touched.push(addr);
            }
            for addr in touched {
                writes.entry(addr).or_default().push(i);
            }
        }
        conflicts.sort_by_key(|c| (c.later, c.earlier));
        conflicts.dedup();
        conflicts
    }

    /// §2.15 GC roots: every blob this record's *entire log* references —
    /// attachment cells in every entry (including superseded values) and
    /// resolver-payload snapshots in origins. The kernel enumerates
    /// roots; the Tier 5 store sweeps. Erasure must cover history, so
    /// this walks the log, not the fold.
    pub fn referenced_blobs(
        &self,
        resolutions: &[crate::Resolution],
    ) -> std::collections::BTreeSet<varve_core::canonical::ContentHash> {
        use varve_value::{CellValue as V, Scalar};
        let mut blobs = std::collections::BTreeSet::new();
        // Landed payloads live on the resolution instance too (§2.7):
        // a snapshot that landed while its targets were overridden may
        // be referenced from nowhere else.
        blobs.extend(resolutions.iter().filter_map(|r| r.snapshot));
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
        let state = self.fold_at(upto).map_err(SnapshotError::Unfoldable)?;
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
        let refolded = self.fold_at(snapshot.at).map_err(SnapshotError::Unfoldable)?;
        if refolded != snapshot.state {
            return Err(SnapshotError::StateMismatch);
        }
        Ok(())
    }
}
