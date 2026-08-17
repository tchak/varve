//! Resolution instances (§2.8) and checkpoints (§2.9).
//!
//! Resolutions sit *beside* cells, not inside them. The kernel
//! contributes pure functions; a Tier 5 scheduler drives retries
//! without the kernel knowing about queues or clocks.

use std::collections::BTreeSet;

use varve_core::canonical::ContentHash;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, GroupId, ResolverId, RevisionId, RowPath};
use varve_value::Op;

use crate::log::RecordLog;
use crate::{ActorKind, Origin};

/// One resolution instance, keyed by `(scope, resolver)` — the scope is
/// the group instance's row path, empty for root (§2.5 uniformity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub resolver: ResolverId,
    /// Bound at request time, not completion (§2.8 rule 1, RATIFIED).
    pub resolver_version: u32,
    pub mapping_version: u32,
    pub scope: RowPath,
    pub status: ResolutionStatus,
    pub attempts: u32,
    pub last_error: Option<String>,
    pub deadline: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionStatus {
    Pending,
    Resolved,
    NotFound,
    Ambiguous,
    Failed,
    /// Must be an explicit recorded event — pending-forever is a leak,
    /// silent give-up is unauditable (§2.8).
    Abandoned,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal resolution transition {from:?} → {to:?}")]
pub struct TransitionError {
    pub from: ResolutionStatus,
    pub to: ResolutionStatus,
}

impl Resolution {
    /// Lifecycle: `pending → resolved | not_found | ambiguous | failed |
    /// abandoned`; `failed → pending` (retry, counted). Terminal states
    /// do not transition.
    pub fn transition(&mut self, to: ResolutionStatus) -> Result<(), TransitionError> {
        use ResolutionStatus::*;
        let legal = matches!(
            (self.status, to),
            (Pending, Resolved | NotFound | Ambiguous | Failed | Abandoned)
                | (Failed, Pending | Abandoned)
        );
        if !legal {
            return Err(TransitionError {
                from: self.status,
                to,
            });
        }
        if (self.status, to) == (Failed, Pending) {
            self.attempts += 1;
        }
        self.status = to;
        Ok(())
    }
}

/// The one pure function a Tier 5 scheduler needs (§2.8).
pub fn pending_resolutions(resolutions: &[Resolution]) -> Vec<&Resolution> {
    resolutions
        .iter()
        .filter(|r| r.status == ResolutionStatus::Pending)
        .collect()
}

/// §2.15: the scan lifecycle beside attachment cells, mirroring
/// resolutions — the scanner is Tier 5; the kernel provides state and
/// pure enumeration so surfaces can gate on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// The attachment element id (§2.4 value-internal identity).
    pub element: String,
    pub hash: varve_core::canonical::ContentHash,
    pub status: ScanStatus,
    pub attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStatus {
    Pending,
    Clean,
    Infected,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal scan transition {from:?} → {to:?}")]
pub struct ScanTransitionError {
    pub from: ScanStatus,
    pub to: ScanStatus,
}

impl Scan {
    /// `pending → clean | infected | failed`; `failed → pending`
    /// (retry, counted). Clean and infected are terminal.
    pub fn transition(&mut self, to: ScanStatus) -> Result<(), ScanTransitionError> {
        use ScanStatus::*;
        let legal = matches!(
            (self.status, to),
            (Pending, Clean | Infected | Failed) | (Failed, Pending)
        );
        if !legal {
            return Err(ScanTransitionError { from: self.status, to });
        }
        if (self.status, to) == (Failed, Pending) {
            self.attempts += 1;
        }
        self.status = to;
        Ok(())
    }
}

pub fn pending_scans(scans: &[Scan]) -> Vec<&Scan> {
    scans.iter().filter(|s| s.status == ScanStatus::Pending).collect()
}

/// A pending resolution a checkpoint expects to land after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedResolution {
    pub resolver: ResolverId,
    pub scope: RowPath,
}

/// §2.9: a named entry hash (the hash, not the seq, pins content), plus
/// a reading revision, plus the pending resolutions expected to land
/// after it, plus the **frozen set** — what the checkpoint freezes.
///
/// The freeze is **surface-scoped** (§2.8, settled): the frozen set is
/// the columns (and the `many` groups holding them) that were writable
/// on the surface the checkpoint was taken through — the applicant
/// form. Everything else on the record — annotation columns, third-
/// party columns — stays open to its own surfaces, which is what makes
/// the record a case file (§2.9) rather than a frozen submission. A
/// later checkpoint supersedes this one (DN's "back to construction").
///
/// The kernel derives nothing here (it cannot see surfaces); a
/// platform fills `frozen_*` from the surface (`varve-surface`
/// exposes `writable_columns`/`writable_groups`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub name: String,
    pub entry: ContentHash,
    pub reading_revision: RevisionId,
    pub expected: Vec<ExpectedResolution>,
    /// Columns frozen by this checkpoint.
    pub frozen_columns: BTreeSet<ColumnId>,
    /// `many` groups frozen by this checkpoint: adding, removing or
    /// reordering their items is a write into the frozen set.
    pub frozen_groups: BTreeSet<GroupId>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CheckpointViolation {
    /// The named entry hash is not in this log.
    #[error("checkpoint names an entry hash not present in this log")]
    UnknownEntry,
    /// The superseding checkpoint's entry hash is not in this log, or
    /// precedes this checkpoint's.
    #[error("superseding checkpoint names an entry hash not after this checkpoint")]
    UnknownSupersedingEntry,
    /// A post-checkpoint entry wrote into the frozen set and was not an
    /// expected derived write (§2.8). Names the frozen columns and
    /// groups the entry touched.
    #[error("entry {seq}: write into the frozen set after checkpoint was not an expected derived write")]
    IllegalWrite {
        seq: u64,
        columns: BTreeSet<ColumnId>,
        groups: BTreeSet<GroupId>,
    },
}

/// §2.8: a checkpoint freezes the cells of its surface and enumerates
/// the pending resolutions it expects. Between this checkpoint and the
/// one superseding it (or the end of the log), a write into the frozen
/// set is legal only if it is an expected derived write; writes outside
/// the frozen set are not the checkpoint's business.
///
/// Pure and reporting, never gating: the platform decides what to do
/// with a violation (§2.9 — the kernel has no permission model; append
/// never consults checkpoints).
pub fn validate_after_checkpoint(
    log: &RecordLog,
    checkpoint: &Checkpoint,
    superseded_by: Option<&Checkpoint>,
) -> Vec<CheckpointViolation> {
    let entries = log.entries();
    let Some(position) = entries.iter().position(|e| e.hash() == checkpoint.entry) else {
        return vec![CheckpointViolation::UnknownEntry];
    };
    let end = match superseded_by {
        None => entries.len(),
        Some(next) => match entries.iter().position(|e| e.hash() == next.entry) {
            // The superseding entry itself is written under the old
            // checkpoint's regime; the new regime starts after it.
            Some(p) if p > position => p + 1,
            _ => return vec![CheckpointViolation::UnknownSupersedingEntry],
        },
    };

    let mut violations = Vec::new();
    for entry in &entries[position + 1..end] {
        let (columns, groups) = frozen_touched(&entry.content.ops, checkpoint);
        if columns.is_empty() && groups.is_empty() {
            continue; // Outside the frozen set: not this checkpoint's business.
        }
        let expected = entry.envelope.actor.kind == ActorKind::Resolver
            && match &entry.content.origin {
                Origin::Derived(d) => checkpoint.expected.iter().any(|exp| {
                    exp.resolver == d.source && ops_within(&entry.content.ops, &exp.scope)
                }),
                _ => false,
            };
        if !expected {
            violations.push(CheckpointViolation::IllegalWrite {
                seq: entry.envelope.seq,
                columns,
                groups,
            });
        }
    }
    violations
}

/// The frozen columns and groups these ops write into.
fn frozen_touched(ops: &[Op], checkpoint: &Checkpoint) -> (BTreeSet<ColumnId>, BTreeSet<GroupId>) {
    let mut columns = BTreeSet::new();
    let mut groups = BTreeSet::new();
    for op in ops {
        match op {
            Op::Set { column, .. } | Op::Unset { column, .. } => {
                if checkpoint.frozen_columns.contains(column) {
                    columns.insert(column.clone());
                }
            }
            Op::AddItem { group, .. } | Op::RemoveItem { group, .. } | Op::Reorder { group, .. } => {
                if checkpoint.frozen_groups.contains(group) {
                    groups.insert(group.clone());
                }
            }
        }
    }
    (columns, groups)
}

/// Every op targets cells at or below the expected scope.
fn ops_within(ops: &[Op], scope: &RowPath) -> bool {
    ops.iter().all(|op| match op {
        Op::Set { path, .. } | Op::Unset { path, .. } => path.starts_with(scope),
        Op::AddItem { parent, .. }
        | Op::RemoveItem { parent, .. }
        | Op::Reorder { parent, .. } => parent.starts_with(scope),
    })
}
