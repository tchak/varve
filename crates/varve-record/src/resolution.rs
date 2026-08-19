//! Resolution instances (§2.8) and checkpoints (§2.9) — both **folds of
//! lifecycle ops carried by ordinary chained entries** (settled
//! 2026-08-19), never side structures beside the log.
//!
//! The kernel records *that* a lookup was requested and *how it ended*;
//! everything between — attempts, transient errors, backoff, deadlines —
//! is Tier 5 scheduler state and never reaches the record (§2.8,
//! PLATFORM.md P.12). The kernel contributes pure enumeration
//! (`FoldResult::pending_resolutions`) and pure validation
//! (`validate_after_checkpoint`); it knows nothing of queues or clocks.

use std::collections::{BTreeMap, BTreeSet};

use varve_core::canonical::ContentHash;
use varve_core::{ColumnId, GroupId, ResolverId, RevisionId, RowPath};
use varve_value::Op;

use crate::entry::EntryOp;
use crate::log::RecordLog;
use crate::{ActorKind, Origin};

/// The identity of a resolution instance: the **anchor-group instance**
/// — the declaration's anchor group (§2.7, Q17) plus the instance's row
/// path, empty at root (§2.5 uniformity). Two SIRET blocks at root are
/// two instances of `insee-sirene`, told apart by their anchor groups;
/// the resolver id is data, not identity.
pub type ResolutionKey = (GroupId, RowPath);

/// A lifecycle transition, as carried by `EntryOp::Resolution` (§2.8):
///
/// ```text
/// pending → resolved | not_found | ambiguous | failed | abandoned
///           (any terminal state) → pending        (re-request: deliberate, recorded)
/// ```
///
/// `failed` is a **definitive** resolver answer ("this will never
/// work"), never a timeout; transient failures are scheduler state and
/// never appear here. Every terminal transition carries an [`Outcome`]
/// summary of the attempts that led to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    /// Opens (or re-opens) the instance. Versions bind here (§2.8 rule
    /// 1, RATIFIED): the same request resolves identically whenever the
    /// API answers. Legal from absence and from every terminal state;
    /// **not** while pending — end the pending one first (`abandon`,
    /// reason `superseded`, when the input changed).
    Request {
        resolver: ResolverId,
        resolver_version: u32,
        mapping_version: u32,
    },
    /// The payload landed (§2.7): `pending → resolved`. A resolution
    /// never resolves without its snapshot; whether the mapped cells
    /// change is the fold's business (§2.8 rule 2).
    Land {
        snapshot: ContentHash,
        outcome: Outcome,
    },
    NotFound {
        outcome: Outcome,
    },
    Ambiguous {
        outcome: Outcome,
    },
    Failed {
        outcome: Outcome,
    },
    /// Policy gave up (§2.8): an explicit recorded event, never a
    /// silent give-up.
    Abandon {
        reason: AbandonReason,
        outcome: Outcome,
    },
}

impl Transition {
    /// The status this transition lands the instance in.
    pub fn status(&self) -> ResolutionStatus {
        match self {
            Transition::Request { .. } => ResolutionStatus::Pending,
            Transition::Land { .. } => ResolutionStatus::Resolved,
            Transition::NotFound { .. } => ResolutionStatus::NotFound,
            Transition::Ambiguous { .. } => ResolutionStatus::Ambiguous,
            Transition::Failed { .. } => ResolutionStatus::Failed,
            Transition::Abandon { reason, .. } => ResolutionStatus::Abandoned(*reason),
        }
    }
}

/// How a terminal transition was reached — the one summary of the
/// scheduler's attempt history that is record meaning (§2.8):
/// "abandoned after 212 attempts, last error 503" in one entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub attempts: u32,
    pub last_error: Option<String>,
}

/// Why an instance was abandoned (§2.8, P.12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AbandonReason {
    /// The instance's abandonment policy ran out.
    Deadline,
    /// An operator decided.
    Operator,
    /// No implementation can serve this resolver here (typically after
    /// a history import).
    ResolverUnavailable,
    /// The lookup's input changed while it was pending; a fresh
    /// `request` follows.
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionStatus {
    Pending,
    Resolved,
    NotFound,
    Ambiguous,
    Failed,
    Abandoned(AbandonReason),
}

impl ResolutionStatus {
    pub fn is_pending(self) -> bool {
        self == ResolutionStatus::Pending
    }
}

/// One resolution instance: the fold of its lifecycle ops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// The declaration's anchor group (§2.7): which block this
    /// resolution feeds.
    pub anchor: GroupId,
    pub scope: RowPath,
    pub resolver: ResolverId,
    /// Bound at request time, not completion (§2.8 rule 1, RATIFIED).
    pub resolver_version: u32,
    pub mapping_version: u32,
    pub status: ResolutionStatus,
    /// Seq of the entry carrying the (latest) `request`: its envelope
    /// timestamp is *when* — the record carries no deadline (§2.8).
    pub requested_at: u64,
    /// Seq of the entry carrying the terminal transition, if any.
    pub closed_at: Option<u64>,
    /// The landed payload (§2.7). Set by `land`; this is where the
    /// snapshot lives when the target cells were overridden while the
    /// resolution was pending (§2.8 rule 2) — and a GC root
    /// (`RecordLog::referenced_blobs`).
    pub snapshot: Option<ContentHash>,
    /// The terminal transition's summary, if the instance is closed.
    pub outcome: Option<Outcome>,
}

impl Resolution {
    pub fn key(&self) -> ResolutionKey {
        (self.anchor.clone(), self.scope.clone())
    }
}

/// A lifecycle op the fold refuses: the log must never hold an illegal
/// transition, so `append` rejects it like an op that does not apply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LifecycleError {
    /// `request` while the instance is pending: end it first.
    #[error("resolution {anchor}@{scope:?} is already pending; end it before re-requesting")]
    AlreadyPending { anchor: GroupId, scope: RowPath },
    /// A terminal transition on an instance that is not pending —
    /// absent, or already closed.
    #[error("resolution {anchor}@{scope:?} is not pending (status {status:?})")]
    NotPending {
        anchor: GroupId,
        scope: RowPath,
        status: Option<ResolutionStatus>,
    },
    /// A checkpoint may only expect resolutions that are pending at
    /// its position, under the versions they were requested with.
    #[error("checkpoint expects a resolution that is not pending: {0:?}")]
    ExpectedNotPending(ExpectedResolution),
    /// One checkpoint per entry: a checkpoint pins a log position.
    #[error("an entry carries more than one checkpoint")]
    MultipleCheckpoints,
}

/// Fold one lifecycle transition into `resolutions` (the §2.8 table).
pub(crate) fn fold_transition(
    resolutions: &mut BTreeMap<ResolutionKey, Resolution>,
    seq: u64,
    anchor: &GroupId,
    scope: &RowPath,
    transition: &Transition,
) -> Result<(), LifecycleError> {
    let key = (anchor.clone(), scope.clone());
    let current = resolutions.get(&key).map(|r| r.status);
    match transition {
        Transition::Request {
            resolver,
            resolver_version,
            mapping_version,
        } => {
            if current.is_some_and(|s| s.is_pending()) {
                return Err(LifecycleError::AlreadyPending {
                    anchor: anchor.clone(),
                    scope: scope.clone(),
                });
            }
            resolutions.insert(
                key,
                Resolution {
                    anchor: anchor.clone(),
                    scope: scope.clone(),
                    resolver: resolver.clone(),
                    resolver_version: *resolver_version,
                    mapping_version: *mapping_version,
                    status: ResolutionStatus::Pending,
                    requested_at: seq,
                    closed_at: None,
                    snapshot: None,
                    outcome: None,
                },
            );
            Ok(())
        }
        terminal => {
            let Some(r) = resolutions.get_mut(&key).filter(|r| r.status.is_pending()) else {
                return Err(LifecycleError::NotPending {
                    anchor: anchor.clone(),
                    scope: scope.clone(),
                    status: current,
                });
            };
            r.status = terminal.status();
            r.closed_at = Some(seq);
            r.outcome = Some(match terminal {
                Transition::Land { outcome, .. }
                | Transition::NotFound { outcome }
                | Transition::Ambiguous { outcome }
                | Transition::Failed { outcome }
                | Transition::Abandon { outcome, .. } => outcome.clone(),
                Transition::Request { .. } => unreachable!("handled above"),
            });
            if let Transition::Land { snapshot, .. } = terminal {
                r.snapshot = Some(*snapshot);
            }
            Ok(())
        }
    }
}

/// A pending resolution a checkpoint expects to land after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedResolution {
    pub anchor: GroupId,
    pub scope: RowPath,
    pub resolver: ResolverId,
    /// §2.8 rule 1: the versions bound at request time. A late write
    /// under other versions is not the resolution the checkpoint
    /// expected — a re-map is a deliberate act, reported.
    pub resolver_version: u32,
    pub mapping_version: u32,
}

impl ExpectedResolution {
    pub fn matches(&self, r: &Resolution) -> bool {
        self.anchor == r.anchor
            && self.scope == r.scope
            && self.resolver == r.resolver
            && self.resolver_version == r.resolver_version
            && self.mapping_version == r.mapping_version
    }
}

/// §2.9: a checkpoint is an **entry in the log** — an `EntryOp::
/// Checkpoint`, whose chained position pins the content it freezes
/// (everything before it) — carrying a name, a reading revision, the
/// pending resolutions expected to land after it, and the **frozen
/// set**.
///
/// The freeze is **surface-scoped** (§2.8, settled): the frozen set is
/// the columns (and the `many` groups holding them) that were writable
/// on the surface the checkpoint was taken through — the applicant
/// form. Everything else on the record — annotation columns, third-
/// party columns — stays open to its own surfaces, which is what makes
/// the record a case file (§2.9) rather than a frozen submission. A
/// later checkpoint entry supersedes this one (DN's "back to
/// construction").
///
/// The kernel derives nothing here (it cannot see surfaces); a
/// platform fills `frozen_*` from the surface (`varve-surface`
/// exposes `writable_columns`/`writable_groups`) and `expected` from
/// the fold's pending set — the fold refuses a checkpoint expecting a
/// resolution that is not pending at its position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub name: String,
    pub reading_revision: RevisionId,
    pub expected: Vec<ExpectedResolution>,
    /// Columns frozen by this checkpoint.
    pub frozen_columns: BTreeSet<ColumnId>,
    /// `many` groups frozen by this checkpoint: adding, removing or
    /// reordering their items is a write into the frozen set.
    pub frozen_groups: BTreeSet<GroupId>,
}

/// A checkpoint as found in the log: the op plus the position that pins
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointAt {
    pub seq: u64,
    /// Hash of the checkpoint entry — what a platform stores or cites.
    pub entry_hash: ContentHash,
    pub checkpoint: Checkpoint,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CheckpointViolation {
    /// No checkpoint entry at that seq in this log.
    #[error("no checkpoint at seq {seq} in this log")]
    UnknownCheckpoint { seq: u64 },
    /// A post-checkpoint entry wrote into the frozen set and was not an
    /// expected derived write (§2.8). Names the frozen columns and
    /// groups the entry touched.
    #[error(
        "entry {seq}: write into the frozen set after checkpoint was not an expected derived write"
    )]
    IllegalWrite {
        seq: u64,
        columns: BTreeSet<ColumnId>,
        groups: BTreeSet<GroupId>,
    },
}

/// §2.8: a checkpoint freezes the cells of its surface and enumerates
/// the pending resolutions it expects. Between the checkpoint entry at
/// `at` and the next checkpoint entry (or the end of the log), a write
/// into the frozen set is legal only if it is an expected derived
/// write; writes outside the frozen set are not the checkpoint's
/// business. The superseding checkpoint's own entry is still judged
/// under the old regime; the new one starts after it.
///
/// Pure and reporting, never gating: the platform decides what to do
/// with a violation (§2.9 — the kernel has no permission model; append
/// never consults checkpoints).
pub fn validate_after_checkpoint(log: &RecordLog, at: u64) -> Vec<CheckpointViolation> {
    let entries = log.entries();
    let Some(checkpoint) = entries
        .get(at as usize)
        .and_then(|e| checkpoint_op(&e.content.ops))
    else {
        return vec![CheckpointViolation::UnknownCheckpoint { seq: at }];
    };
    let position = at as usize;
    let end = entries
        .iter()
        .skip(position + 1)
        .position(|e| checkpoint_op(&e.content.ops).is_some())
        .map_or(entries.len(), |p| position + 1 + p + 1);

    let mut violations = Vec::new();
    for entry in &entries[position + 1..end] {
        let (columns, groups) = frozen_touched(&entry.content.ops, checkpoint);
        if columns.is_empty() && groups.is_empty() {
            continue; // Outside the frozen set: not this checkpoint's business.
        }
        let expected = entry.envelope.actor.kind == ActorKind::Resolver
            && match &entry.content.origin {
                Origin::Derived(d) => checkpoint.expected.iter().any(|exp| {
                    exp.resolver == d.source
                        && exp.resolver_version == d.source_version
                        && exp.mapping_version == d.mapping_version
                        && ops_within(&entry.content.ops, &exp.scope)
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

/// The checkpoint an entry carries, if any (the fold admits at most
/// one per entry).
pub(crate) fn checkpoint_op(ops: &[EntryOp]) -> Option<&Checkpoint> {
    ops.iter().find_map(|op| match op {
        EntryOp::Checkpoint(c) => Some(c),
        _ => None,
    })
}

/// The frozen columns and groups these ops write into.
fn frozen_touched(
    ops: &[EntryOp],
    checkpoint: &Checkpoint,
) -> (BTreeSet<ColumnId>, BTreeSet<GroupId>) {
    let mut columns = BTreeSet::new();
    let mut groups = BTreeSet::new();
    for op in ops {
        match op {
            EntryOp::Cell(Op::Set { column, .. } | Op::Unset { column, .. }) => {
                if checkpoint.frozen_columns.contains(column) {
                    columns.insert(column.clone());
                }
            }
            EntryOp::Cell(
                Op::AddItem { group, .. }
                | Op::RemoveItem { group, .. }
                | Op::Reorder { group, .. },
            ) => {
                if checkpoint.frozen_groups.contains(group) {
                    groups.insert(group.clone());
                }
            }
            EntryOp::Resolution { .. } | EntryOp::Checkpoint(_) => {}
        }
    }
    (columns, groups)
}

/// Every cell op targets cells at or below the expected scope, and
/// every lifecycle op names an instance at or below it.
fn ops_within(ops: &[EntryOp], scope: &RowPath) -> bool {
    ops.iter().all(|op| match op {
        EntryOp::Cell(Op::Set { path, .. } | Op::Unset { path, .. }) => path.starts_with(scope),
        EntryOp::Cell(
            Op::AddItem { parent, .. } | Op::RemoveItem { parent, .. } | Op::Reorder { parent, .. },
        ) => parent.starts_with(scope),
        EntryOp::Resolution { scope: s, .. } => s.starts_with(scope),
        EntryOp::Checkpoint(_) => true,
    })
}
