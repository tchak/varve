//! Import (§5 import modes, §6 migration): history import **adopts**
//! chains (verify, then continue appending); snapshot import lands as
//! an **ordinary log entry** — never a side door — so LWW, conflict
//! detection, provenance and checkpoints apply to bulk imports exactly
//! as to human edits. The manifest's intent makes id mismatches fail
//! loudly.

use std::collections::BTreeMap;

use varve_core::RecordId;
use varve_core::primitives::Instant;
use varve_core::RevisionId;
use varve_record::{Actor, Draft, Entry, EntrySalts, Origin, RecordLog};
use varve_value::diff;

use crate::line::{Intent, Line, Mode};
use crate::read::{Stream, snapshot_records};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ImportError {
    #[error("stream mode is {0:?}, not the mode this import expects")]
    WrongMode(Mode),
    #[error("record '{0}': chain verification failed: {1}")]
    Chain(RecordId, varve_record::ChainError),
    /// §5: create-only with an existing id, update-only with an unknown
    /// id — rejected, never silently duplicated or dropped.
    #[error("record '{0}' already exists (intent: create-only)")]
    AlreadyExists(RecordId),
    #[error("record '{0}' does not exist (intent: update-only)")]
    NotFound(RecordId),
    /// The record's log (existing, or as adopted) does not fold: entry
    /// `seq` failed to apply.
    #[error("record '{0}': entry {1} does not apply — the log does not fold")]
    Fold(RecordId, u64),
    /// The import entry could not be appended (the ops the diff produced
    /// do not apply, or the salts do not line up).
    #[error("record '{0}': import entry could not be appended: {1}")]
    Append(RecordId, varve_record::AppendError),
    /// Under `Upsert`/`UpdateOnly` the imported history must extend the
    /// existing chain; a shorter or diverging history is a conflict of
    /// histories, not a tamper (§6: one-way, one-time migration).
    #[error("record '{0}': imported history diverges from the existing chain")]
    Diverges(RecordId),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportOutcome {
    pub created: Vec<RecordId>,
    pub updated: Vec<RecordId>,
}

fn check_intent(
    intent: Intent,
    existing: bool,
    record: &RecordId,
) -> Result<(), ImportError> {
    match (intent, existing) {
        (Intent::CreateOnly, true) => Err(ImportError::AlreadyExists(record.clone())),
        (Intent::UpdateOnly, false) => Err(ImportError::NotFound(record.clone())),
        _ => Ok(()),
    }
}

/// History import: group entries per record, rebuild each log, **verify
/// its chain**, then adopt it into `store` — the importing instance
/// continues appending to the same chain (§6: tamper-evidence spans
/// both instances). Under `Upsert`/`UpdateOnly` an existing record is
/// replaced by the imported log only if the imported log *extends* the
/// existing one (same prefix): a diverging import is a conflict of
/// histories and is rejected.
///
/// **All or nothing** (§5: "import rejects on line 1 or commits to the
/// whole stream"): every record is verified and staged first; `store`
/// is touched only once nothing can fail.
pub fn adopt_history(
    stream: &Stream,
    store: &mut BTreeMap<RecordId, RecordLog>,
) -> Result<ImportOutcome, ImportError> {
    if stream.manifest.mode != Mode::History {
        return Err(ImportError::WrongMode(stream.manifest.mode));
    }
    let mut incoming: BTreeMap<RecordId, Vec<Entry>> = BTreeMap::new();
    for line in &stream.lines {
        if let Line::Entry { record, entry } = line {
            incoming.entry(record.clone()).or_default().push(entry.clone());
        }
    }
    let mut outcome = ImportOutcome::default();
    let mut staged: Vec<(RecordId, RecordLog)> = Vec::new();
    for (record, mut entries) in incoming {
        entries.sort_by_key(|e| e.envelope.seq);
        let log = RecordLog::from_entries(record.clone(), entries);
        log.verify_chain().map_err(|e| ImportError::Chain(record.clone(), e))?;
        // Adopted logs must fold: a chain that verifies but does not
        // apply would be poison (`RecordLog::append` refuses it later,
        // but importing it would still be importing damage).
        log.fold().map_err(|e| ImportError::Fold(record.clone(), e.seq))?;
        let existing = store.get(&record);
        check_intent(stream.manifest.intent, existing.is_some(), &record)?;
        if let Some(current) = existing {
            // Must extend the current chain: every current entry hash
            // must appear at the same position in the import.
            let extends = current.entries().len() <= log.entries().len()
                && current.entries().iter().zip(log.entries()).all(|(a, b)| a.hash() == b.hash());
            if !extends {
                return Err(ImportError::Diverges(record.clone()));
            }
            outcome.updated.push(record.clone());
        } else {
            outcome.created.push(record.clone());
        }
        staged.push((record, log));
    }
    // Commit: nothing below can fail.
    for (record, log) in staged {
        store.insert(record, log);
    }
    Ok(outcome)
}

/// Everything a snapshot import needs from the caller (Tier 5 supplies
/// the actor, timestamp and salts — the kernel has no clock and no
/// randomness).
pub struct SnapshotImportRequest<'a> {
    pub actor: Actor,
    pub timestamp: Instant,
    /// The revision the imported cells are read through, authored
    /// against for the resulting entry.
    pub revision: RevisionId,
    pub note: Option<String>,
    /// Fresh salts, one per op plus one for metadata — the caller sizes
    /// this per record via `salts_for(op_count)`.
    pub salts_for: &'a dyn Fn(usize) -> EntrySalts,
}

/// Snapshot import: whole-record replace (§5). For each record line,
/// `diff(current folded state, imported state)` becomes one ordinary
/// entry appended to the record's log — new records start from the
/// empty state, so a snapshot export imports as a patch against empty.
pub fn import_snapshot(
    stream: &Stream,
    store: &mut BTreeMap<RecordId, RecordLog>,
    request: &SnapshotImportRequest<'_>,
) -> Result<ImportOutcome, ImportError> {
    if stream.manifest.mode != Mode::Snapshot {
        return Err(ImportError::WrongMode(stream.manifest.mode));
    }
    let mut outcome = ImportOutcome::default();
    // All or nothing (§5): every record's new log is built on a staged
    // copy; `store` is touched only once nothing can fail.
    let mut staged: Vec<(RecordId, RecordLog)> = Vec::new();
    for r in snapshot_records(stream) {
        let existing = store.get(&r.record);
        check_intent(stream.manifest.intent, existing.is_some(), &r.record)?;
        let mut log = existing.cloned().unwrap_or_else(|| RecordLog::new(r.record.clone()));
        let current = log.fold().map_err(|e| ImportError::Fold(r.record.clone(), e.seq))?.values;
        let ops = diff(&current, &r.values);
        if ops.is_empty() && existing.is_some() {
            outcome.updated.push(r.record.clone());
            continue;
        }
        let salts = (request.salts_for)(ops.len());
        let base_version = log.version();
        log.append(Draft {
            actor: request.actor.clone(),
            timestamp: request.timestamp,
            revision: request.revision.clone(),
            base_version,
            origin: Origin::Entered,
            note: request.note.clone(),
            ops,
            salts,
        })
        .map_err(|e| ImportError::Append(r.record.clone(), e))?;
        if existing.is_some() {
            outcome.updated.push(r.record.clone());
        } else {
            outcome.created.push(r.record.clone());
        }
        staged.push((r.record, log));
    }
    for (record, log) in staged {
        store.insert(record, log);
    }
    Ok(outcome)
}

/// Deterministic salts from a counter, for tests and examples only —
/// behind the `test-util` feature so it cannot reach production by
/// accident: Tier 5 must supply random salts (§2.13 decision 5).
#[cfg(feature = "test-util")]
pub fn test_salts(seed: u8) -> impl Fn(usize) -> EntrySalts {
    use varve_core::canonical::Salt;
    move |n| EntrySalts {
        meta: Salt([seed; 32]),
        ops: (0..n).map(|i| Salt([seed.wrapping_add(i as u8 + 1); 32])).collect(),
    }
}
