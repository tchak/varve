//! Resolution instances (§2.8) and checkpoints (§2.9).
//!
//! Resolutions sit *beside* cells, not inside them. The kernel
//! contributes pure functions; a Tier 5 scheduler drives retries
//! without the kernel knowing about queues or clocks.

use varve_core::canonical::ContentHash;
use varve_core::primitives::Instant;
use varve_core::{ResolverId, RevisionId, RowPath};
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

/// A pending resolution a checkpoint expects to land after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedResolution {
    pub resolver: ResolverId,
    pub scope: RowPath,
}

/// §2.9: a named entry hash (the hash, not the seq, pins content), plus
/// a reading revision, plus the pending resolutions expected to land
/// after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub name: String,
    pub entry: ContentHash,
    pub reading_revision: RevisionId,
    pub expected: Vec<ExpectedResolution>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CheckpointViolation {
    /// The named entry hash is not in this log.
    UnknownEntry,
    /// A post-checkpoint entry that is not a derived write from the
    /// expected list (§2.8: "anything else is rejected").
    IllegalWrite { seq: u64 },
}

/// §2.8: a checkpoint freezes entered cells and enumerates the pending
/// resolutions it expects. Late derived writes are legal only if they
/// were on that list.
pub fn validate_after_checkpoint(
    log: &RecordLog,
    checkpoint: &Checkpoint,
) -> Vec<CheckpointViolation> {
    let Some(position) = log
        .entries()
        .iter()
        .position(|e| e.hash() == checkpoint.entry)
    else {
        return vec![CheckpointViolation::UnknownEntry];
    };

    let mut violations = Vec::new();
    for entry in &log.entries()[position + 1..] {
        let legal = entry.envelope.actor.kind == ActorKind::Resolver
            && match &entry.content.origin {
                Origin::Derived(d) => checkpoint.expected.iter().any(|exp| {
                    exp.resolver == d.source && ops_within(&entry.content.ops, &exp.scope)
                }),
                _ => false,
            };
        if !legal {
            violations.push(CheckpointViolation::IllegalWrite {
                seq: entry.envelope.seq,
            });
        }
    }
    violations
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
