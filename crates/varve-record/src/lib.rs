//! Tier 3 (§7): the record log (§2.9) — entries, fold, snapshots,
//! concurrency detection — and, as folds of lifecycle ops in that same
//! log, resolution instances (§2.8) and checkpoints (§2.9).
//!
//! Still deterministic: no clock, no IO. Timestamps and salts are
//! inputs, generated at Tier 5 and passed in.

#![forbid(unsafe_code)]

pub mod canon;
mod entry;
mod log;
mod resolution;
mod scan;

pub use entry::{
    Draft, Entry, EntryContent, EntryOp, EntrySalts, Envelope, SaltCountMismatch, genesis_hash,
};
pub use log::{
    AppendError, ChainError, Conflict, FoldError, FoldResult, RecordLog, Snapshot, SnapshotError,
    Suppressed,
};
pub use resolution::{
    AbandonReason, Checkpoint, CheckpointAt, CheckpointViolation, ExpectedResolution,
    LifecycleError, Outcome, Resolution, ResolutionKey, ResolutionStatus, Transition,
    validate_after_checkpoint,
};
pub use scan::{Scan, ScanStatus, ScanTransitionError, pending_scans};

use varve_core::ResolverId;
use varve_core::canonical::ContentHash;

/// Who authored an entry (§2.9): an opaque id plus a kind.
///
/// **Contract (§2.13):** the id is a pseudonymous reference. The
/// id→person mapping lives platform-side and is separately erasable —
/// a platform that writes direct identifiers here has broken the
/// contract, and only whole-record erasure recovers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    pub id: String,
    pub kind: ActorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorKind {
    Human,
    Resolver,
    System,
}

/// Where an entry's values came from (§2.7). Cell-level provenance is
/// *derived* by fold from the entry origins: a human write over a
/// derived cell yields `overridden { superseded }` even if the entry
/// just said `entered`, and a resolver's late derived write onto a
/// human-authored cell is refused and lands as `superseded` instead
/// (§2.8 rule 2). See `RecordLog::fold`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Entered,
    Derived(Derivation),
    /// Retains what it replaced (§2.7): divergence is a cell-local
    /// read. `superseded` is absent only while an override made during
    /// a pending resolution has not yet been answered — once the late
    /// derived write lands, the fold fills it in; the landed snapshot
    /// also lives on the resolution instance (`Resolution::snapshot`).
    Overridden {
        superseded: Option<Derivation>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derivation {
    pub source: ResolverId,
    pub source_version: u32,
    pub mapping_version: u32,
    pub snapshot_ref: ContentHash,
}
