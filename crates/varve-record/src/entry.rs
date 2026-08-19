//! The log entry (§2.9), shaped by §2.13: a plaintext **envelope** that
//! survives redaction and lives as long as the record, and redactable
//! **content** committed op-by-op.

use std::collections::BTreeMap;

use varve_core::canonical::{CanonicalValue, ContentHash, Salt, commit, commit_vector, hash_plain};
use varve_core::primitives::Instant;
use varve_core::{GroupId, RecordId, RevisionId, RowPath};
use varve_value::Op;

use crate::canon;
use crate::resolution::{Checkpoint, Transition};
use crate::scan::ScanTransition;
use crate::{Actor, Origin};

/// One op of an entry (§2.9): a cell op — the §5 wire ops — or a
/// **lifecycle op** (settled 2026-08-19): a resolution transition
/// (§2.8), an attachment scan transition (§2.15), or a checkpoint
/// (§2.9). Lifecycle ops ride the same chained, salted, committed list
/// as cell ops — one representation for log, export, migration and
/// diff.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryOp {
    Cell(Op),
    /// A transition of the resolution instance keyed by
    /// `(anchor, scope)` — the anchor-group instance (§2.7, Q17).
    Resolution {
        anchor: GroupId,
        scope: RowPath,
        transition: Transition,
    },
    /// A transition of the scan of one attachment element (§2.15),
    /// keyed by its element id (§2.4 value-internal identity).
    Scan {
        element: String,
        transition: ScanTransition,
    },
    Checkpoint(Checkpoint),
}

impl From<Op> for EntryOp {
    fn from(op: Op) -> Self {
        EntryOp::Cell(op)
    }
}

impl EntryOp {
    /// The cell op, if this is one.
    pub fn cell(&self) -> Option<&Op> {
        match self {
            EntryOp::Cell(op) => Some(op),
            _ => None,
        }
    }
}

/// Chain anchor: `prev` of the entry at seq 0, committing to the
/// record's id — so a log verifies only under the record it belongs to
/// and cannot be transplanted under another (§2.9). Still per-record:
/// nothing global (§2.10).
pub fn genesis_hash(record: &RecordId) -> ContentHash {
    hash_plain(&CanonicalValue::String(format!("varve:genesis:{record}")))
        .expect("strings never fail")
}

/// Survives redaction; lives as long as the record (§2.13 decision 8).
/// `base_version` is structural (concurrency detection, §2.9) and
/// non-personal, so it rides in the envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub seq: u64,
    pub prev: ContentHash,
    pub actor: Actor,
    pub timestamp: Instant,
    /// Authored-against revision: every entry has one; the record does
    /// not (§2.9 — the record's revision is a reading lens).
    pub revision: RevisionId,
    pub base_version: u64,
    pub content_hash: ContentHash,
}

/// Redactable, erasable (§2.13 decision 8).
#[derive(Debug, Clone, PartialEq)]
pub struct EntryContent {
    pub ops: Vec<EntryOp>,
    pub origin: Origin,
    pub note: Option<String>,
}

/// One salt per op plus one for the metadata (origin + note). Inputs,
/// like timestamps; destroyed with the content they commit (§2.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntrySalts {
    pub meta: Salt,
    pub ops: Vec<Salt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub envelope: Envelope,
    pub content: EntryContent,
    pub salts: EntrySalts,
}

/// Everything an appender provides; seq, prev and hashes are computed.
#[derive(Debug, Clone)]
pub struct Draft {
    pub actor: Actor,
    pub timestamp: Instant,
    pub revision: RevisionId,
    pub base_version: u64,
    pub origin: Origin,
    pub note: Option<String>,
    pub ops: Vec<EntryOp>,
    pub salts: EntrySalts,
}

/// Ops and op salts must pair one-to-one. Anything else is refused
/// rather than committed: an op without a salt would fall outside the
/// commitment and become invisible to chain verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{ops} ops but {salts} op salts (need one per op)")]
pub struct SaltCountMismatch {
    pub ops: usize,
    pub salts: usize,
}

impl Entry {
    /// The content commitment: vector commitment over the salted
    /// metadata commitment and one salted commitment per op (§2.13
    /// decision 4). Every op is committed — a salt/op count mismatch
    /// is an error, never a truncated commitment.
    pub fn content_hash(
        content: &EntryContent,
        salts: &EntrySalts,
    ) -> Result<ContentHash, SaltCountMismatch> {
        if content.ops.len() != salts.ops.len() {
            return Err(SaltCountMismatch {
                ops: content.ops.len(),
                salts: salts.ops.len(),
            });
        }
        let meta = canon::meta(&content.origin, content.note.as_deref());
        let mut parts = vec![commit(&salts.meta, &meta).expect("no floats in metadata")];
        for (op, salt) in content.ops.iter().zip(&salts.ops) {
            parts.push(commit(salt, &canon::op(op)).expect("ops canonicalize"));
        }
        Ok(commit_vector(&parts))
    }

    /// The entry hash: plain hash over the envelope, which includes the
    /// content commitment. This is the chain link (`prev` of the next
    /// entry) and what a checkpoint entry's position pins.
    pub fn hash(&self) -> ContentHash {
        let e = &self.envelope;
        let fields: BTreeMap<String, CanonicalValue> = [
            ("seq", CanonicalValue::Int(e.seq as i64)),
            ("prev", CanonicalValue::String(e.prev.to_string())),
            ("actor", CanonicalValue::String(e.actor.id.clone())),
            (
                "actor_kind",
                CanonicalValue::String(
                    match e.actor.kind {
                        crate::ActorKind::Human => "human",
                        crate::ActorKind::Resolver => "resolver",
                        crate::ActorKind::System => "system",
                    }
                    .to_string(),
                ),
            ),
            ("timestamp", CanonicalValue::String(e.timestamp.to_string())),
            ("revision", CanonicalValue::String(e.revision.to_string())),
            ("base_version", CanonicalValue::Int(e.base_version as i64)),
            (
                "content",
                CanonicalValue::String(e.content_hash.to_string()),
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        hash_plain(&CanonicalValue::Object(fields)).expect("no floats in envelope")
    }
}
