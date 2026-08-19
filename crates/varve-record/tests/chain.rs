//! The chain tamper matrix (§2.9, §2.13): every field an attacker with
//! storage access could rewrite, and which check catches it — plus the
//! golden vectors that pin the canonical shapes the hashes commit to.

use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, RecordId, RevisionId, RowPath};
use varve_record::{
    Actor, ActorKind, AppendError, ChainError, Draft, Entry, EntryOp, EntrySalts, Origin,
    RecordLog, SnapshotError, genesis_hash,
};
use varve_value::{CellState, CellValue, Op, Scalar};

fn human(id: &str) -> Actor {
    Actor {
        id: id.into(),
        kind: ActorKind::Human,
    }
}

fn ts(minute: u8) -> Instant {
    Instant::parse(&format!("2026-08-16T10:{minute:02}:00Z")).unwrap()
}

fn set(column: &str, value: &str) -> Op {
    Op::Set {
        column: ColumnId::new(column),
        path: RowPath::root(),
        state: CellState::Value(CellValue::One(Scalar::Text(value.into()))),
    }
}

fn salts(n: usize) -> EntrySalts {
    EntrySalts {
        meta: Salt([9; 32]),
        ops: (0..n).map(|i| Salt([i as u8 + 1; 32])).collect(),
    }
}

fn draft(actor: Actor, minute: u8, base: u64, origin: Origin, ops: Vec<Op>) -> Draft {
    let n = ops.len();
    Draft {
        actor,
        timestamp: ts(minute),
        revision: RevisionId::new("rev-1"),
        base_version: base,
        origin,
        note: None,
        ops: ops.into_iter().map(EntryOp::Cell).collect(),
        salts: salts(n),
    }
}

/// Three entries: the middle one has two ops (order-sensitivity), the
/// tail is the unanchored one.
fn three() -> RecordLog {
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("name", "Dupont")],
    ))
    .unwrap();
    log.append(draft(
        human("a2"),
        1,
        1,
        Origin::Entered,
        vec![set("city", "Lyon"), set("name", "Durand")],
    ))
    .unwrap();
    log.append(draft(
        human("a1"),
        2,
        2,
        Origin::Entered,
        vec![set("name", "Martin")],
    ))
    .unwrap();
    log.verify_chain().unwrap();
    log
}

fn rehydrate(entries: Vec<Entry>) -> RecordLog {
    RecordLog::from_entries(RecordId::new("r1"), entries)
}

/// Rehydrate after mutating one entry in place.
fn tampered(log: &RecordLog, at: usize, f: impl FnOnce(&mut Entry)) -> RecordLog {
    let mut entries = log.entries().to_vec();
    f(&mut entries[at]);
    rehydrate(entries)
}

#[test]
fn prev_and_seq_tampers_are_caught_where_they_sit() {
    let log = three();
    let bad_prev = tampered(&log, 1, |e| {
        e.envelope.prev = genesis_hash(&RecordId::new("r1"))
    });
    assert_eq!(
        bad_prev.verify_chain(),
        Err(ChainError::PrevMismatch { at: 1 })
    );

    let bad_seq = tampered(&log, 1, |e| e.envelope.seq = 5);
    assert_eq!(
        bad_seq.verify_chain(),
        Err(ChainError::SeqMismatch { at: 1 })
    );
}

#[test]
fn reordering_and_removing_entries_break_contiguity() {
    let log = three();
    let mut swapped = log.entries().to_vec();
    swapped.swap(0, 1);
    assert_eq!(
        rehydrate(swapped).verify_chain(),
        Err(ChainError::SeqMismatch { at: 0 })
    );

    let mut middle_gone = log.entries().to_vec();
    middle_gone.remove(1);
    // seq 2 now sits at position 1.
    assert_eq!(
        rehydrate(middle_gone).verify_chain(),
        Err(ChainError::SeqMismatch { at: 1 })
    );
}

/// A named in-place edit of one entry.
type Tamper = (&'static str, Box<dyn Fn(&mut Entry)>);

/// Every envelope field is inside the entry hash, so an edit to a
/// non-tail entry breaks the next entry's `prev`.
fn envelope_tampers() -> Vec<Tamper> {
    vec![
        (
            "actor id",
            Box::new(|e: &mut Entry| e.envelope.actor.id = "mallory".into()),
        ),
        (
            "actor kind",
            Box::new(|e: &mut Entry| e.envelope.actor.kind = ActorKind::System),
        ),
        (
            "timestamp",
            Box::new(|e: &mut Entry| e.envelope.timestamp = ts(59)),
        ),
        (
            "revision",
            Box::new(|e: &mut Entry| e.envelope.revision = RevisionId::new("rev-9")),
        ),
        (
            "base_version",
            Box::new(|e: &mut Entry| e.envelope.base_version = 0),
        ),
    ]
}

#[test]
fn envelope_tampers_on_a_non_tail_entry_break_the_next_link() {
    let log = three();
    for (name, tamper) in envelope_tampers() {
        let t = tampered(&log, 1, tamper);
        assert_eq!(
            t.verify_chain(),
            Err(ChainError::PrevMismatch { at: 2 }),
            "{name}"
        );
    }
}

#[test]
fn the_tail_is_unanchored_by_construction_and_a_snapshot_pins_it() {
    // §2.9: the last entry's hash is nobody's `prev`. Its envelope can
    // be rewritten without the chain noticing — that is exactly what a
    // checkpoint or snapshot naming the head hash is for.
    let log = three();
    let head = log.snapshot_at(3).unwrap();
    for (name, tamper) in envelope_tampers() {
        let t = tampered(&log, 2, tamper);
        assert_eq!(
            t.verify_chain(),
            Ok(()),
            "{name}: chain alone cannot see a tail edit"
        );
        assert_eq!(
            t.verify_snapshot(&head),
            Err(SnapshotError::HashMismatch),
            "{name}: the pinned head hash catches it"
        );
    }
    // Truncation: the chain verifies, the head snapshot no longer fits.
    let mut truncated = log.entries().to_vec();
    truncated.pop();
    let truncated = rehydrate(truncated);
    assert_eq!(truncated.verify_chain(), Ok(()));
    assert_eq!(
        truncated.verify_snapshot(&head),
        Err(SnapshotError::OutOfRange)
    );
}

#[test]
fn content_tampers_break_the_commitment_even_on_the_tail() {
    // Content is committed op by op under the salts (§2.13 decision 4):
    // an edit to any of ops, origin, note or salts recomputes to a
    // different content hash — on the tail as much as anywhere.
    let log = three();
    let cases: Vec<Tamper> = vec![
        (
            "op edit",
            Box::new(|e: &mut Entry| e.content.ops[0] = set("name", "MALLORY").into()),
        ),
        (
            "origin edit",
            Box::new(|e: &mut Entry| e.content.origin = Origin::Overridden { superseded: None }),
        ),
        (
            "note added",
            Box::new(|e: &mut Entry| e.content.note = Some("quiet fix".into())),
        ),
        (
            "meta salt",
            Box::new(|e: &mut Entry| e.salts.meta = Salt([42; 32])),
        ),
        (
            "op salt",
            Box::new(|e: &mut Entry| e.salts.ops[0] = Salt([42; 32])),
        ),
    ];
    for at in [1usize, 2] {
        for (name, tamper) in &cases {
            let t = tampered(&log, at, tamper);
            assert_eq!(
                t.verify_chain(),
                Err(ChainError::ContentMismatch { at }),
                "{name} at {at}"
            );
        }
    }
    // A note edit (not just an addition).
    let mut with_note = RecordLog::new(RecordId::new("r1"));
    let mut d = draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("name", "Dupont")],
    );
    d.note = Some("original".into());
    with_note.append(d).unwrap();
    let t = tampered(&with_note, 0, |e| e.content.note = Some("edited".into()));
    assert_eq!(t.verify_chain(), Err(ChainError::ContentMismatch { at: 0 }));
}

#[test]
fn the_commitment_is_order_sensitive_over_ops() {
    let log = three();
    let t = tampered(&log, 1, |e| e.content.ops.swap(0, 1));
    assert_eq!(t.verify_chain(), Err(ChainError::ContentMismatch { at: 1 }));
    // Swapping the salts along with the ops is a *different* commitment
    // too: salt i commits op i.
    let t = tampered(&log, 1, |e| {
        e.content.ops.swap(0, 1);
        e.salts.ops.swap(0, 1);
    });
    assert_eq!(t.verify_chain(), Err(ChainError::ContentMismatch { at: 1 }));
}

#[test]
fn identical_content_under_different_salts_commits_differently() {
    // §2.13 decision 5: the commitment hides the value; two entries
    // with byte-identical content and different salts share nothing
    // hash-wise, and both verify.
    let mut a = RecordLog::new(RecordId::new("r1"));
    let mut b = RecordLog::new(RecordId::new("r1"));
    a.append(draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("name", "Dupont")],
    ))
    .unwrap();
    let mut d = draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("name", "Dupont")],
    );
    d.salts = EntrySalts {
        meta: Salt([200; 32]),
        ops: vec![Salt([201; 32])],
    };
    b.append(d).unwrap();
    let (ea, eb) = (&a.entries()[0], &b.entries()[0]);
    assert_eq!(ea.content, eb.content);
    assert_ne!(ea.envelope.content_hash, eb.envelope.content_hash);
    assert_ne!(ea.hash(), eb.hash());
    a.verify_chain().unwrap();
    b.verify_chain().unwrap();
}

#[test]
fn a_base_version_ahead_of_the_log_is_refused() {
    let mut log = three();
    let before = log.entries().to_vec();
    let refused = log.append(draft(
        human("a1"),
        3,
        4,
        Origin::Entered,
        vec![set("name", "x")],
    ));
    assert_eq!(
        refused.map(|_| ()),
        Err(AppendError::BaseVersionAhead {
            base: 4,
            version: 3
        })
    );
    assert_eq!(log.entries(), before.as_slice());
    // base == version is the ordinary case.
    log.append(draft(
        human("a1"),
        3,
        3,
        Origin::Entered,
        vec![set("name", "x")],
    ))
    .unwrap();
}

#[test]
fn golden_vectors_pin_the_canonical_shapes() {
    // These literals commit to the canonical forms of §2.13 (genesis
    // string, per-op commitment vector, envelope object). They catch
    // canonical-shape drift: a change here means the on-disk / on-wire
    // hashes of every existing instance change, and must be a
    // deliberate format decision, never a side effect.
    assert_eq!(
        genesis_hash(&RecordId::new("r1")).to_string(),
        "sha256:eb725b7004da0a5173ed5a17aa39416bb48cd1db7c6aa7a3b03a802964953eee"
    );
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("name", "Dupont")],
    ))
    .unwrap();
    let entry = &log.entries()[0];
    assert_eq!(
        entry.envelope.content_hash.to_string(),
        "sha256:de5e446fcda1406826033c9b7f651cf5b89ef7822cf615b6e4bedda6fca9f9fc"
    );
    assert_eq!(
        entry.hash().to_string(),
        "sha256:eb7310677e58a7cda78e2ae36da42ace8a38fa4978fc8f79669b819d2d51d4ce"
    );
}
