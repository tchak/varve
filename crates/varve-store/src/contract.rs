//! The store contract, executable against any implementation (§13.2,
//! platform P.8: the harness both the reference [`crate::MemoryStore`]
//! and the platform's Toasty store run). Behind the `test-util`
//! feature — deterministic fixtures (fixed salts, fixed timestamps)
//! that must never reach production (§2.13 decision 5).
//!
//! The recurring shape the checks pin down: the store enforces
//! **index rules only**; content invariants re-run in the loaders, so
//! a tampered row is accepted at write and caught at the first load.
//!
//! Record-log checks only for now; the registry checks join here when
//! a second implementation of those traits exists to run them.

use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, RecordId, RevisionId, RowPath};
use varve_record::{Actor, ActorKind, Draft, EntryOp, EntrySalts, Origin, RecordLog};
use varve_value::{CellState, CellValue, Op, Scalar};

use crate::load::{LoadError, load_log};
use crate::{RecordLogStore, StoreError};

fn actor() -> Actor {
    Actor {
        id: "a1".into(),
        kind: ActorKind::Human,
    }
}

fn ts(minute: u8) -> Instant {
    Instant::parse(&format!("2026-08-20T10:{minute:02}:00Z")).unwrap()
}

fn set(column: &str, value: &str) -> Op {
    Op::Set {
        column: ColumnId::new(column),
        path: RowPath::root(),
        state: CellState::Value(CellValue::One(Scalar::Text(value.into()))),
    }
}

fn draft(minute: u8, base: u64, ops: Vec<Op>) -> Draft {
    let n = ops.len();
    Draft {
        actor: actor(),
        timestamp: ts(minute),
        revision: RevisionId::new("rev-1"),
        base_version: base,
        origin: Origin::Entered,
        note: None,
        ops: ops.into_iter().map(EntryOp::Cell).collect(),
        salts: EntrySalts {
            meta: Salt([9; 32]),
            ops: (0..n).map(|i| Salt([i as u8 + 1; 32])).collect(),
        },
    }
}

/// A valid log of `n` entries, minted by the kernel appender.
fn log_of(record: &str, n: u64) -> RecordLog {
    let mut log = RecordLog::new(RecordId::new(record));
    for i in 0..n {
        log.append(draft(i as u8, i, vec![set("name", &format!("v{i}"))]))
            .unwrap();
    }
    log
}

/// Appended entries read back equal — through [`load_log`], so chain
/// verification re-runs — and partial reads are HTTP-range shaped.
pub async fn check_log_roundtrip(store: &impl RecordLogStore) {
    let record = RecordId::new("r1");
    let log = log_of("r1", 3);
    for entry in log.entries() {
        store.append(&record, entry).await.unwrap();
    }
    assert_eq!(store.version(&record).await.unwrap(), 3);

    let reloaded = load_log(store, &record).await.unwrap();
    assert_eq!(reloaded.entries(), log.entries());

    // Partial read: entries from a seq; past-the-end is empty.
    assert_eq!(store.entries(&record, 1).await.unwrap().len(), 2);
    assert_eq!(store.entries(&record, 3).await.unwrap().len(), 0);

    // Unknown record: empty, version 0 — creation is the first append.
    let ghost = RecordId::new("ghost");
    assert_eq!(store.entries(&ghost, 0).await.unwrap().len(), 0);
    assert_eq!(store.version(&ghost).await.unwrap(), 0);
    assert!(load_log(store, &ghost).await.unwrap().entries().is_empty());
}

/// The next-seq rule: replays and skips refuse with
/// [`StoreError::SeqConflict`] and change nothing.
pub async fn check_log_seq_conflict(store: &impl RecordLogStore) {
    let record = RecordId::new("r1");
    let log = log_of("r1", 2);
    store.append(&record, &log.entries()[0]).await.unwrap();

    // Replaying the same seq: the optimistic-concurrency refusal.
    let err = store.append(&record, &log.entries()[0]).await.unwrap_err();
    assert_eq!(
        err,
        StoreError::SeqConflict {
            record: record.clone(),
            next: 1,
            got: 0,
        }
    );
    // Skipping ahead is refused the same way.
    let mut ahead = log.entries()[1].clone();
    ahead.envelope.seq = 5;
    let err = store.append(&record, &ahead).await.unwrap_err();
    assert_eq!(
        err,
        StoreError::SeqConflict {
            record: record.clone(),
            next: 1,
            got: 5,
        }
    );
    // A refused append changes nothing.
    assert_eq!(store.version(&record).await.unwrap(), 1);
}

/// The store accepts any entry with the right seq — including one
/// minted for another record (wrong genesis, wrong prev). The loader
/// is where that dies.
pub async fn check_loader_enforces_chain(store: &impl RecordLogStore) {
    let record = RecordId::new("r1");
    store
        .append(&record, &log_of("r1", 1).entries()[0])
        .await
        .unwrap();
    let foreign = log_of("other", 2).entries()[1].clone();
    store.append(&record, &foreign).await.unwrap();

    let err = load_log(store, &record).await.unwrap_err();
    assert!(matches!(err, LoadError::Chain { record: r, .. } if r == record));
}

/// `records` pages known ids ascending, strictly after the cursor.
pub async fn check_record_enumeration(store: &impl RecordLogStore) {
    for name in ["r1", "r2", "r3", "r4", "r5"] {
        let record = RecordId::new(name);
        store
            .append(&record, &log_of(name, 1).entries()[0])
            .await
            .unwrap();
    }
    let page = store.records(None, 2).await.unwrap();
    assert_eq!(page, vec![RecordId::new("r1"), RecordId::new("r2")]);
    let rest = store.records(Some(&RecordId::new("r2")), 10).await.unwrap();
    assert_eq!(
        rest,
        vec![
            RecordId::new("r3"),
            RecordId::new("r4"),
            RecordId::new("r5")
        ]
    );
    assert!(
        store
            .records(Some(&RecordId::new("r5")), 10)
            .await
            .unwrap()
            .is_empty()
    );
}
