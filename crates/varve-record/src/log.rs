//! The append-only record log: current state is `fold(log)` (§2.9).

use std::collections::{BTreeMap, BTreeSet};

use varve_core::canonical::ContentHash;
use varve_core::{GroupId, RecordId, RowPath};
use varve_value::{ApplyError, CellAddr, Op, RecordValues, apply, diff};

use crate::Origin;
use crate::entry::{Draft, Entry, EntryOp, Envelope, SaltCountMismatch, genesis_hash};
use crate::resolution::{
    CheckpointAt, LifecycleError, Resolution, ResolutionKey, Transition, checkpoint_op,
    fold_transition,
};
use crate::scan::{Scan, ScanTransition, fold_scan_transition};

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
    /// A lifecycle op the §2.8 table does not allow from the current
    /// state (a request while pending, a landing on a closed instance,
    /// a checkpoint expecting what is not pending). Refused for the
    /// same reason: the log must fold.
    #[error("illegal lifecycle transition: {0}")]
    IllegalTransition(LifecycleError),
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
    SaltCount {
        at: usize,
        mismatch: SaltCountMismatch,
    },
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FoldError {
    /// Entry `seq`'s ops do not apply — a poisoned or tampered log.
    #[error("entry {seq}: {error}")]
    Apply { seq: u64, error: ApplyError },
    /// Entry `seq` carries a lifecycle op illegal from the state before
    /// it — a poisoned or tampered log.
    #[error("entry {seq}: {error}")]
    Lifecycle { seq: u64, error: LifecycleError },
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
    /// Resolution instances (§2.8): the fold of the log's lifecycle
    /// ops, keyed by anchor-group instance.
    pub resolutions: BTreeMap<ResolutionKey, Resolution>,
    /// Attachment scans (§2.15), keyed by element id.
    pub scans: BTreeMap<String, Scan>,
}

impl FoldResult {
    /// The one pure enumeration a Tier 5 scheduler needs (§2.8): every
    /// instance still pending, whatever its age — the kernel has no
    /// clock and no deadline; policy is the platform's (P.12).
    pub fn pending_resolutions(&self) -> impl Iterator<Item = &Resolution> {
        self.resolutions.values().filter(|r| r.status.is_pending())
    }

    /// What the logic language reads (§2.8 rule 3): pending resolutions
    /// as `(scope, anchor group)` pairs — per group instance, so
    /// "required unless pending" in one item does not leak into
    /// another, and two blocks fed by one resolver do not leak into
    /// each other (§10 Q17). Feed this to
    /// `varve_logic::EvalContext::pending`.
    pub fn pending_set(&self) -> BTreeSet<(RowPath, GroupId)> {
        self.pending_resolutions()
            .map(|r| (r.scope.clone(), r.anchor.clone()))
            .collect()
    }

    /// The pure enumeration a Tier 5 scanner needs (§2.15): every
    /// element whose scan is pending. Includes elements since removed
    /// from their cell until the platform ends them (`abandon`,
    /// `superseded`) — the fold keeps history, not a view.
    pub fn pending_scans(&self) -> impl Iterator<Item = &Scan> {
        self.scans.values().filter(|s| s.status.is_pending())
    }
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
            Some(d) => Origin::Overridden {
                superseded: Some(d),
            },
            None if matches!(current, Some(Origin::Overridden { .. })) => {
                Origin::Overridden { superseded: None }
            }
            None => Origin::Entered,
        },
        Origin::Overridden { superseded: None } => Origin::Overridden {
            superseded: replaced,
        },
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
    let mut checkpoints = 0;
    for op in &entry.content.ops {
        let op = match op {
            EntryOp::Cell(op) => op,
            EntryOp::Resolution {
                anchor,
                scope,
                transition,
            } => {
                fold_transition(&mut result.resolutions, seq, anchor, scope, transition)
                    .map_err(|error| FoldError::Lifecycle { seq, error })?;
                continue;
            }
            EntryOp::Scan {
                element,
                transition,
            } => {
                fold_scan_transition(&mut result.scans, seq, element, transition).map_err(
                    |error| FoldError::Lifecycle {
                        seq,
                        error: error.into(),
                    },
                )?;
                continue;
            }
            EntryOp::Checkpoint(checkpoint) => {
                checkpoints += 1;
                if checkpoints > 1 {
                    return Err(FoldError::Lifecycle {
                        seq,
                        error: LifecycleError::MultipleCheckpoints,
                    });
                }
                // A checkpoint expects only what is pending at its
                // position, under the versions it was requested with.
                for exp in &checkpoint.expected {
                    let pending = result
                        .resolutions
                        .get(&(exp.anchor.clone(), exp.scope.clone()))
                        .is_some_and(|r| r.status.is_pending() && exp.matches(r));
                    if !pending {
                        return Err(FoldError::Lifecycle {
                            seq,
                            error: LifecycleError::ExpectedNotPending(exp.clone()),
                        });
                    }
                }
                continue;
            }
        };
        // §2.8 rule 2, enforced where provenance is derived: a
        // resolver's derived write onto a human-authored cell is not
        // applied; the cell keeps its value and gains the late
        // derivation as `superseded` (divergence visible, restore
        // possible), and the write is reported.
        if let Op::Set { column, path, .. } | Op::Unset { column, path } = op
            && late_machine_write
        {
            let addr = CellAddr {
                column: column.clone(),
                path: path.clone(),
            };
            if let Some(current) = result.provenance.get(&addr)
                && matches!(current, Origin::Entered | Origin::Overridden { .. })
            {
                let landed = match origin {
                    Origin::Derived(d) => Some(d.clone()),
                    _ => None,
                };
                let superseded = match current {
                    Origin::Overridden {
                        superseded: Some(d),
                    } => Some(d.clone()),
                    _ => landed,
                };
                result
                    .provenance
                    .insert(addr.clone(), Origin::Overridden { superseded });
                result.suppressed.push(Suppressed { seq, addr });
                continue;
            }
        }
        apply(&mut result.values, op).map_err(|error| FoldError::Apply { seq, error })?;
        match op {
            Op::Set { column, path, .. } => {
                let addr = CellAddr {
                    column: column.clone(),
                    path: path.clone(),
                };
                let derived = derive_provenance(origin, result.provenance.get(&addr));
                result.provenance.insert(addr, derived);
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
    Ok(())
}

impl RecordLog {
    /// An empty log for `record`.
    pub fn new(record: RecordId) -> Self {
        Self {
            record,
            entries: Vec::new(),
        }
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

    /// The checkpoints in this log (§2.9), in order — each an entry
    /// whose position pins what it freezes. The last one is the
    /// regime in force.
    pub fn checkpoints(&self) -> Vec<CheckpointAt> {
        self.entries
            .iter()
            .filter_map(|e| {
                checkpoint_op(&e.content.ops).map(|c| CheckpointAt {
                    seq: e.envelope.seq,
                    entry_hash: e.hash(),
                    checkpoint: c.clone(),
                })
            })
            .collect()
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
            FoldError::Lifecycle { error, .. } => AppendError::IllegalTransition(error),
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
            return Err(FoldError::OutOfRange {
                upto,
                version: self.version(),
            });
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

    /// Same-cell writes the later actor had not seen (§2.9: detect,
    /// do not merge): entry B conflicts with an earlier entry A by another
    /// actor when A wrote a cell B also writes and `B.base_version <=
    /// A.seq` (B did not see A's write; same base is the boundary case,
    /// not the criterion). One forward pass with a per-cell
    /// write history — linear in the ops of the log.
    pub fn detect_conflicts(&self) -> Vec<Conflict> {
        // Per cell: the entries that wrote it, in seq order.
        let mut writes: BTreeMap<CellAddr, Vec<usize>> = BTreeMap::new();
        let mut conflicts = Vec::new();
        for (i, later) in self.entries.iter().enumerate() {
            let base = later.envelope.base_version;
            let mut touched: Vec<CellAddr> = Vec::new();
            for op in &later.content.ops {
                let Some(Op::Set { column, path, .. } | Op::Unset { column, path }) = op.cell()
                else {
                    continue;
                };
                let addr = CellAddr {
                    column: column.clone(),
                    path: path.clone(),
                };
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
    /// attachment cells in every entry (including superseded values),
    /// resolver-payload snapshots in origins, the payloads landed by
    /// `land` ops (a snapshot that landed while its targets were
    /// overridden may be referenced from nowhere else, §2.8 rule 2),
    /// and the bytes named by scan requests.
    /// The kernel enumerates roots; the Tier 5 store sweeps. Erasure
    /// must cover history, so this walks the log, not the fold.
    pub fn referenced_blobs(&self) -> BTreeSet<ContentHash> {
        use varve_value::{CellValue as V, Scalar};
        let mut blobs = BTreeSet::new();
        for entry in &self.entries {
            match &entry.content.origin {
                crate::Origin::Derived(d)
                | crate::Origin::Overridden {
                    superseded: Some(d),
                } => {
                    blobs.insert(d.snapshot_ref);
                }
                _ => {}
            }
            for op in &entry.content.ops {
                match op {
                    EntryOp::Resolution {
                        transition: Transition::Land { snapshot, .. },
                        ..
                    } => {
                        blobs.insert(*snapshot);
                    }
                    EntryOp::Scan {
                        transition: ScanTransition::Request { hash },
                        ..
                    } => {
                        blobs.insert(*hash);
                    }
                    _ => {}
                }
                if let Some(Op::Set { state, .. }) = op.cell()
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
        let refolded = self
            .fold_at(snapshot.at)
            .map_err(SnapshotError::Unfoldable)?;
        if refolded != snapshot.state {
            return Err(SnapshotError::StateMismatch);
        }
        Ok(())
    }
}
