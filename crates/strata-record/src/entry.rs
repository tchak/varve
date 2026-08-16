//! The log entry (§2.9), shaped by §2.13: a plaintext **envelope** that
//! survives redaction and lives as long as the record, and redactable
//! **content** committed op-by-op.

use std::collections::BTreeMap;

use strata_core::canonical::{
    CanonicalValue, ContentHash, Salt, commit, commit_vector, hash_plain,
};
use strata_core::primitives::Instant;
use strata_core::RevisionId;
use strata_value::Op;

use crate::canon;
use crate::{Actor, Origin};

/// Fixed chain anchor: `prev` of the entry at seq 0.
pub fn genesis_hash() -> ContentHash {
    hash_plain(&CanonicalValue::String("strata:genesis".to_string()))
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
    pub ops: Vec<Op>,
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
    pub ops: Vec<Op>,
    pub salts: EntrySalts,
}

impl Entry {
    /// The content commitment: vector commitment over the salted
    /// metadata commitment and one salted commitment per op (§2.13
    /// decision 4).
    pub fn content_hash(content: &EntryContent, salts: &EntrySalts) -> ContentHash {
        let meta = canon::meta(&content.origin, content.note.as_deref());
        let mut parts =
            vec![commit(&salts.meta, &meta).expect("no floats in metadata")];
        for (op, salt) in content.ops.iter().zip(&salts.ops) {
            parts.push(commit(salt, &canon::op(op)).expect("ops canonicalize"));
        }
        commit_vector(&parts)
    }

    /// The entry hash: plain hash over the envelope, which includes the
    /// content commitment. This is the chain link (`prev` of the next
    /// entry) and what checkpoints name.
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
            ("content", CanonicalValue::String(e.content_hash.to_string())),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        hash_plain(&CanonicalValue::Object(fields)).expect("no floats in envelope")
    }
}
