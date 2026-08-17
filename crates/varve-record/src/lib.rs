//! Tier 3 (§7): the record log (§2.9) — entries, fold, snapshots,
//! checkpoints, concurrency detection, resolution instances (§2.8).
//!
//! Still deterministic: no clock, no IO. Timestamps and salts are
//! inputs, generated at Tier 5 and passed in.

#![forbid(unsafe_code)]

pub mod canon;
mod entry;
mod log;
mod resolution;

pub use entry::{Draft, Entry, EntryContent, EntrySalts, Envelope, genesis_hash};
pub use log::{
    AppendError, ChainError, Conflict, FoldError, FoldResult, RecordLog,
    Snapshot, SnapshotError,
};
pub use resolution::{
    Checkpoint, CheckpointViolation, ExpectedResolution, Resolution,
    ResolutionStatus, Scan, ScanStatus, ScanTransitionError, TransitionError,
    pending_resolutions, pending_scans, validate_after_checkpoint,
};

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
/// derived by fold: a cell's origin is the origin of the entry that
/// last set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Entered,
    Derived(Derivation),
    /// Retains what it replaced (§2.7): divergence is a cell-local
    /// read. `superseded` is absent only when overriding while a
    /// resolution was still pending — the landed snapshot then lives on
    /// the resolution instance.
    Overridden { superseded: Option<Derivation> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derivation {
    pub source: ResolverId,
    pub source_version: u32,
    pub mapping_version: u32,
    pub snapshot_ref: ContentHash,
}
