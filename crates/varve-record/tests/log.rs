use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, GroupId, ItemId, PathSeg, RecordId, ResolverId, RevisionId, RowPath};
use varve_record::{
    AbandonReason, Actor, ActorKind, AppendError, ChainError, Checkpoint, CheckpointViolation,
    Derivation, Draft, EntryOp, EntrySalts, ExpectedResolution, LifecycleError, Origin, Outcome,
    RecordLog, ResolutionStatus, SaltCountMismatch, Transition, validate_after_checkpoint,
};
use varve_value::{CellAddr, CellState, CellValue, Op, Scalar};

fn human(id: &str) -> Actor {
    Actor {
        id: id.into(),
        kind: ActorKind::Human,
    }
}

fn resolver_actor() -> Actor {
    Actor {
        id: "resolver:insee".into(),
        kind: ActorKind::Resolver,
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

/// Like `draft`, over entry ops (cell and lifecycle alike).
fn entry_draft(actor: Actor, minute: u8, base: u64, origin: Origin, ops: Vec<EntryOp>) -> Draft {
    let n = ops.len();
    Draft {
        actor,
        timestamp: ts(minute),
        revision: RevisionId::new("rev-1"),
        base_version: base,
        origin,
        note: None,
        ops,
        salts: salts(n),
    }
}

fn resolution(transition: Transition) -> EntryOp {
    EntryOp::Resolution {
        anchor: GroupId::new("entreprise"),
        scope: RowPath::root(),
        transition,
    }
}

fn request() -> EntryOp {
    resolution(Transition::Request {
        resolver: ResolverId::new("insee-sirene"),
        resolver_version: 1,
        mapping_version: 1,
    })
}

fn expected() -> ExpectedResolution {
    ExpectedResolution {
        anchor: GroupId::new("entreprise"),
        scope: RowPath::root(),
        resolver: ResolverId::new("insee-sirene"),
        resolver_version: 1,
        mapping_version: 1,
    }
}

fn derivation() -> Derivation {
    Derivation {
        source: ResolverId::new("insee-sirene"),
        source_version: 1,
        mapping_version: 1,
        snapshot_ref: varve_record::genesis_hash(&RecordId::new("r1")),
    }
}

fn addr(column: &str) -> CellAddr {
    CellAddr {
        column: ColumnId::new(column),
        path: RowPath::root(),
    }
}

#[test]
fn append_fold_and_verify() {
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
        resolver_actor(),
        1,
        1,
        Origin::Derived(derivation()),
        vec![set("raison_sociale", "ACME SARL")],
    ))
    .unwrap();
    log.append(draft(
        human("a1"),
        2,
        2,
        Origin::Entered,
        vec![set("name", "Durand")],
    ))
    .unwrap();

    log.verify_chain().unwrap();
    // A log point past the end is an error, never the head.
    assert!(matches!(
        log.fold_at(4),
        Err(varve_record::FoldError::OutOfRange {
            upto: 4,
            version: 3
        })
    ));
    assert!(log.diff_between(0, 9).is_err());

    let folded = log.fold().unwrap();
    assert_eq!(
        folded.values.cells.get(&addr("name")),
        Some(&CellState::Value(CellValue::One(Scalar::Text(
            "Durand".into()
        ))))
    );
    // Provenance: last writer's origin, per cell.
    assert_eq!(folded.provenance.get(&addr("name")), Some(&Origin::Entered));
    assert!(matches!(
        folded.provenance.get(&addr("raison_sociale")),
        Some(Origin::Derived(_))
    ));
}

#[test]
fn tampering_breaks_the_chain() {
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
        human("a1"),
        1,
        1,
        Origin::Entered,
        vec![set("name", "Durand")],
    ))
    .unwrap();
    log.verify_chain().unwrap();

    // A "quiet correction" of history: rewrite the value inside a
    // stored entry, then rehydrate — as an attacker with storage access
    // would.
    let mut entries = log.entries().to_vec();
    if let EntryOp::Cell(Op::Set { state, .. }) = &mut entries[0].content.ops[0] {
        *state = CellState::Value(CellValue::One(Scalar::Text("Martin".into())));
    }
    let tampered = RecordLog::from_entries(RecordId::new("r1"), entries);
    assert!(tampered.verify_chain().is_err());
}

#[test]
fn append_refuses_ops_that_do_not_apply() {
    // The §2.9 detect-don't-merge scenario: two actors remove the same
    // item from the same base. The second entry must not be appended —
    // an entry the fold cannot apply would poison the log (every later
    // fold failing forever). It is refused, the log is unchanged, and
    // the fold still works.
    let mut log = RecordLog::new(RecordId::new("r1"));
    let add = Op::AddItem {
        group: GroupId::new("g1"),
        parent: RowPath::root(),
        item: ItemId::new("i1"),
        at: 0,
    };
    let remove = Op::RemoveItem {
        group: GroupId::new("g1"),
        parent: RowPath::root(),
        item: ItemId::new("i1"),
    };
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![add]))
        .unwrap();
    log.append(draft(
        human("a1"),
        1,
        1,
        Origin::Entered,
        vec![remove.clone()],
    ))
    .unwrap();
    let before = log.entries().len();
    let refused = log.append(draft(human("b2"), 2, 1, Origin::Entered, vec![remove]));
    assert!(matches!(
        refused,
        Err(AppendError::DoesNotApply(
            varve_value::ApplyError::UnknownItem(..)
        ))
    ));
    assert_eq!(log.entries().len(), before);
    assert!(log.fold().is_ok());
    // A zero-length list is refused the same way (§2.4 one encoding).
    let empty_many = Op::Set {
        column: ColumnId::new("tags"),
        path: RowPath::root(),
        state: CellState::Value(CellValue::Many(vec![])),
    };
    assert!(matches!(
        log.append(draft(human("a1"), 3, 2, Origin::Entered, vec![empty_many])),
        Err(AppendError::DoesNotApply(
            varve_value::ApplyError::EmptyList(_)
        ))
    ));
    // A poisoned log (rehydrated with an entry that does not apply)
    // refuses appends until repaired, instead of growing the damage.
    let mut entries = log.entries().to_vec();
    entries[1].content.ops.push(EntryOp::Cell(Op::RemoveItem {
        group: GroupId::new("g1"),
        parent: RowPath::root(),
        item: ItemId::new("ghost"),
    }));
    let mut poisoned = RecordLog::from_entries(RecordId::new("r1"), entries);
    assert!(matches!(
        poisoned.append(draft(
            human("a1"),
            4,
            2,
            Origin::Entered,
            vec![set("name", "x")]
        )),
        Err(AppendError::Unfoldable(_))
    ));
}

#[test]
fn a_log_verifies_only_under_its_own_record() {
    // The genesis commits to the record id (§2.9): record A's stored
    // log rehydrated under record B's id is a chain error at entry 0 —
    // logs cannot be transplanted. Still per-record: nothing global.
    let mut log = RecordLog::new(RecordId::new("A"));
    log.append(draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("name", "Dupont")],
    ))
    .unwrap();
    log.verify_chain().unwrap();
    let transplanted = RecordLog::from_entries(RecordId::new("B"), log.entries().to_vec());
    assert_eq!(
        transplanted.verify_chain(),
        Err(ChainError::PrevMismatch { at: 0 })
    );
    let same = RecordLog::from_entries(RecordId::new("A"), log.entries().to_vec());
    assert_eq!(same.verify_chain(), Ok(()));
}

#[test]
fn injected_op_without_a_salt_is_detected() {
    // The commitment is a vector over (op, salt) pairs. An op appended
    // to a stored entry *without* a salt must not slip outside the
    // commitment: the chain must reject it, not verify around it.
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
        human("a1"),
        1,
        1,
        Origin::Entered,
        vec![set("city", "Lyon")],
    ))
    .unwrap();

    let mut entries = log.entries().to_vec();
    entries[0].content.ops.push(set("name", "MALLORY").into());
    let tampered = RecordLog::from_entries(RecordId::new("r1"), entries);
    assert_eq!(
        tampered.verify_chain(),
        Err(ChainError::SaltCount {
            at: 0,
            mismatch: SaltCountMismatch { ops: 2, salts: 1 }
        })
    );

    // Same for a tail entry — the last entry is otherwise unanchored.
    let mut entries = log.entries().to_vec();
    entries[1].content.ops.push(set("name", "MALLORY").into());
    let tampered = RecordLog::from_entries(RecordId::new("r1"), entries);
    assert!(matches!(
        tampered.verify_chain(),
        Err(ChainError::SaltCount { at: 1, .. })
    ));

    // And the mirror image: a salt without an op.
    let mut entries = log.entries().to_vec();
    entries[0].salts.ops.push(Salt([42; 32]));
    let tampered = RecordLog::from_entries(RecordId::new("r1"), entries);
    assert_eq!(
        tampered.verify_chain(),
        Err(ChainError::SaltCount {
            at: 0,
            mismatch: SaltCountMismatch { ops: 1, salts: 2 }
        })
    );
}

#[test]
fn append_refuses_a_salt_count_mismatch() {
    let mut log = RecordLog::new(RecordId::new("r1"));
    let mut d = draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("a", "1"), set("b", "2")],
    );
    d.salts = salts(1);
    assert_eq!(
        log.append(d).map(|_| ()),
        Err(AppendError::SaltCount(SaltCountMismatch {
            ops: 2,
            salts: 1
        }))
    );
    assert_eq!(log.version(), 0);
}

#[test]
fn conflicts_are_detected_not_merged() {
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("name", "Dupont")],
    ))
    .unwrap();
    // Two instructors both edit from version 1.
    log.append(draft(
        human("a2"),
        1,
        1,
        Origin::Entered,
        vec![set("name", "Durand")],
    ))
    .unwrap();
    log.append(draft(
        human("a3"),
        2,
        1,
        Origin::Entered,
        vec![set("name", "Martin")],
    ))
    .unwrap();
    // A fourth write that saw everything: no conflict.
    log.append(draft(
        human("a2"),
        3,
        3,
        Origin::Entered,
        vec![set("name", "Bernard")],
    ))
    .unwrap();
    // The same actor rewriting its own cell from a stale base is not a
    // two-actor conflict.
    log.append(draft(
        human("a2"),
        4,
        3,
        Origin::Entered,
        vec![set("name", "Bernard-Durand")],
    ))
    .unwrap();

    let conflicts = log.detect_conflicts();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].addr, addr("name"));
    assert_eq!((conflicts[0].earlier, conflicts[0].later), (1, 2));
    // LWW still holds: last write wins in the fold.
    let folded = log.fold().unwrap();
    assert_eq!(
        folded.values.cells.get(&addr("name")),
        Some(&CellState::Value(CellValue::One(Scalar::Text(
            "Bernard-Durand".into()
        ))))
    );
}

#[test]
fn lost_updates_with_different_bases_are_conflicts() {
    // Same base is the boundary case, not the criterion (§2.9): here the
    // two rival writers read at *different* versions — an unrelated
    // entry landed in between — and the later still never saw the
    // earlier's write.
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("name", "Dupont")],
    ))
    .unwrap();
    // Unrelated write, so the rivals' bases differ.
    log.append(draft(
        human("a2"),
        1,
        1,
        Origin::Entered,
        vec![set("email", "d@ex.fr")],
    ))
    .unwrap();
    // a1 read at version 1 (before the email entry), a3 at version 2 —
    // neither saw the other's "name" write.
    log.append(draft(
        human("a1"),
        2,
        1,
        Origin::Entered,
        vec![set("name", "Durand")],
    ))
    .unwrap();
    log.append(draft(
        human("a3"),
        3,
        2,
        Origin::Entered,
        vec![set("name", "Martin")],
    ))
    .unwrap();

    let conflicts = log.detect_conflicts();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].addr, addr("name"));
    assert_eq!((conflicts[0].earlier, conflicts[0].later), (2, 3));
}

#[test]
fn diff_between_log_points() {
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
        human("a1"),
        1,
        1,
        Origin::Entered,
        vec![
            set("name", "Durand"),
            Op::AddItem {
                group: GroupId::new("contacts"),
                parent: RowPath::root(),
                item: ItemId::new("i1"),
                at: 0,
            },
        ],
    ))
    .unwrap();

    let ops = log.diff_between(1, 2).unwrap();
    assert_eq!(ops.len(), 2);
    // And an empty diff for identical points.
    assert!(log.diff_between(2, 2).unwrap().is_empty());
}

#[test]
fn snapshots_verify_and_detect_tampering() {
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
        human("a1"),
        1,
        1,
        Origin::Entered,
        vec![set("name", "Durand")],
    ))
    .unwrap();

    let snapshot = log.snapshot_at(1).unwrap();
    log.verify_snapshot(&snapshot).unwrap();

    let mut forged = snapshot.clone();
    forged.state.values.cells.insert(
        addr("name"),
        CellState::Value(CellValue::One(Scalar::Text("Martin".into()))),
    );
    assert!(log.verify_snapshot(&forged).is_err());
}

#[test]
fn referenced_blobs_cover_history_and_snapshots() {
    use varve_core::canonical::{CanonicalValue, hash_plain};
    use varve_value::AttachmentRef;
    let blob = |s: &str| hash_plain(&CanonicalValue::String(s.into())).unwrap();
    let file = |name: &str, content: &str| Op::Set {
        column: ColumnId::new("piece"),
        path: RowPath::root(),
        state: CellState::Value(CellValue::Many(vec![Scalar::Attachment(Box::new(
            AttachmentRef {
                id: name.into(),
                hash: blob(content),
                filename: format!("{name}.pdf"),
                content_type: "application/pdf".into(),
                byte_size: 10,
            },
        ))])),
    };
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![file("f1", "v1")],
    ))
    .unwrap();
    // The file is replaced — the superseded blob must STILL be a root:
    // erasure covers history, so GC must not collect what the log
    // references.
    log.append(draft(
        human("a1"),
        1,
        1,
        Origin::Entered,
        vec![file("f1", "v2")],
    ))
    .unwrap();
    // A derived write carries a snapshot ref.
    log.append(draft(
        resolver_actor(),
        2,
        2,
        Origin::Derived(derivation()),
        vec![set("raison_sociale", "ACME")],
    ))
    .unwrap();

    let blobs = log.referenced_blobs();
    assert!(blobs.contains(&blob("v1")));
    assert!(blobs.contains(&blob("v2")));
    assert!(blobs.contains(&derivation().snapshot_ref));
    assert_eq!(blobs.len(), 3);

    // A payload landed by a `land` op is a root too — it may be
    // referenced from nowhere else (§2.8 rule 2: the targets were
    // overridden, so no origin carries it).
    log.append(entry_draft(
        human("a1"),
        3,
        3,
        Origin::Entered,
        vec![request()],
    ))
    .unwrap();
    log.append(entry_draft(
        resolver_actor(),
        4,
        4,
        Origin::Entered,
        vec![resolution(Transition::Land {
            snapshot: blob("payload"),
            outcome: Outcome::default(),
        })],
    ))
    .unwrap();
    let blobs = log.referenced_blobs();
    assert!(blobs.contains(&blob("payload")));
    assert_eq!(blobs.len(), 4);
}

#[test]
fn scan_lifecycle_is_a_fold_of_the_log() {
    // §2.15, aligned with §2.8: the applicant uploads a file (request in
    // the same entry), the scanner's verdict lands later as an op; the
    // blob named by the request is a GC root; a rescan after new
    // signatures is a deliberate new request.
    use varve_core::canonical::{CanonicalValue, hash_plain};
    use varve_record::{ScanStatus, ScanTransition};
    use varve_value::AttachmentRef;
    let blob = hash_plain(&CanonicalValue::String("bytes".into())).unwrap();
    let scan = |transition: ScanTransition| EntryOp::Scan {
        element: "f1".into(),
        transition,
    };
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(entry_draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![
            Op::Set {
                column: ColumnId::new("piece"),
                path: RowPath::root(),
                state: CellState::Value(CellValue::Many(vec![Scalar::Attachment(Box::new(
                    AttachmentRef {
                        id: "f1".into(),
                        hash: blob,
                        filename: "f1.pdf".into(),
                        content_type: "application/pdf".into(),
                        byte_size: 10,
                    },
                ))])),
            }
            .into(),
            scan(ScanTransition::Request { hash: blob }),
        ],
    ))
    .unwrap();
    let state = log.fold().unwrap();
    assert_eq!(state.scans["f1"].status, ScanStatus::Pending);
    assert_eq!(state.pending_scans().count(), 1);
    assert!(log.referenced_blobs().contains(&blob));

    log.append(entry_draft(
        Actor {
            id: "scanner:clamav".into(),
            kind: ActorKind::System,
        },
        1,
        1,
        Origin::Entered,
        vec![scan(ScanTransition::Clean {
            outcome: Outcome {
                attempts: 3,
                last_error: Some("clamd: connection refused".into()),
            },
        })],
    ))
    .unwrap();
    let state = log.fold().unwrap();
    assert_eq!(state.scans["f1"].status, ScanStatus::Clean);
    assert_eq!(state.scans["f1"].closed_at, Some(1));
    assert_eq!(state.pending_scans().count(), 0);
    // Terminal: no second verdict.
    assert!(matches!(
        log.append(entry_draft(
            human("a1"),
            2,
            2,
            Origin::Entered,
            vec![scan(ScanTransition::Infected {
                threat: None,
                outcome: Outcome::default()
            })]
        )),
        Err(AppendError::IllegalTransition(LifecycleError::Scan(_)))
    ));
    // Rescan against new signatures: a fresh request, then the verdict.
    log.append(entry_draft(
        human("a1"),
        2,
        2,
        Origin::Entered,
        vec![scan(ScanTransition::Request { hash: blob })],
    ))
    .unwrap();
    log.append(entry_draft(
        human("a1"),
        3,
        3,
        Origin::Entered,
        vec![scan(ScanTransition::Infected {
            threat: Some("EICAR-Test-File".into()),
            outcome: Outcome::default(),
        })],
    ))
    .unwrap();
    let s = &log.fold().unwrap().scans["f1"];
    assert_eq!(
        (s.status, s.requested_at, s.closed_at),
        (ScanStatus::Infected, 2, Some(3))
    );
    assert_eq!(s.threat.as_deref(), Some("EICAR-Test-File"));
}

#[test]
fn resolution_lifecycle_is_a_fold_of_the_log() {
    // §2.8 (settled 2026-08-19): the instance is the fold of lifecycle
    // ops in chained entries — requested at submit, landed by the
    // resolver in the same entry as its derived writes.
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(entry_draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("siret", "123").into(), request()],
    ))
    .unwrap();
    let state = log.fold().unwrap();
    let r = &state.resolutions[&(GroupId::new("entreprise"), RowPath::root())];
    assert_eq!(r.status, ResolutionStatus::Pending);
    assert_eq!(r.requested_at, 0);
    assert_eq!(r.closed_at, None);
    assert_eq!(state.pending_resolutions().count(), 1);
    // What the logic language reads: (scope, anchor group) pairs.
    assert_eq!(
        state.pending_set(),
        [(RowPath::root(), GroupId::new("entreprise"))]
            .into_iter()
            .collect()
    );

    // Transient failures never reach the record (§2.8): the scheduler
    // retried for three days, and the record sees one landing carrying
    // the summary.
    let payload = derivation().snapshot_ref;
    log.append(entry_draft(
        resolver_actor(),
        1,
        1,
        Origin::Derived(derivation()),
        vec![
            resolution(Transition::Land {
                snapshot: payload,
                outcome: Outcome {
                    attempts: 212,
                    last_error: Some("503".into()),
                },
            }),
            set("raison_sociale", "ACME").into(),
        ],
    ))
    .unwrap();
    let state = log.fold().unwrap();
    let r = &state.resolutions[&(GroupId::new("entreprise"), RowPath::root())];
    assert_eq!(r.status, ResolutionStatus::Resolved);
    assert_eq!(r.snapshot, Some(payload));
    assert_eq!(r.closed_at, Some(1));
    assert_eq!(r.outcome.as_ref().unwrap().attempts, 212);
    assert!(state.pending_set().is_empty());

    // Terminal: a second landing, or an abandonment, is refused at
    // append — the log never holds an illegal transition.
    for t in [
        Transition::Land {
            snapshot: payload,
            outcome: Outcome::default(),
        },
        Transition::Abandon {
            reason: AbandonReason::Operator,
            outcome: Outcome::default(),
        },
    ] {
        assert!(matches!(
            log.append(entry_draft(
                resolver_actor(),
                2,
                2,
                Origin::Entered,
                vec![resolution(t)]
            )),
            Err(AppendError::IllegalTransition(
                LifecycleError::NotPending { .. }
            ))
        ));
    }
    assert_eq!(log.version(), 2, "refused appends leave the log alone");

    // Re-request is deliberate and recorded (§2.8): the SIRET changed,
    // a fresh request follows; the earlier outcome stays in the log.
    log.append(entry_draft(
        human("a1"),
        3,
        2,
        Origin::Entered,
        vec![set("siret", "456").into(), request()],
    ))
    .unwrap();
    let state = log.fold().unwrap();
    let r = &state.resolutions[&(GroupId::new("entreprise"), RowPath::root())];
    assert_eq!(r.status, ResolutionStatus::Pending);
    assert_eq!(r.requested_at, 2);
    assert_eq!((r.closed_at, r.snapshot, &r.outcome), (None, None, &None));
    // But not while pending: end it first (`superseded`).
    assert!(matches!(
        log.append(entry_draft(
            human("a1"),
            4,
            3,
            Origin::Entered,
            vec![request()]
        )),
        Err(AppendError::IllegalTransition(
            LifecycleError::AlreadyPending { .. }
        ))
    ));
    log.append(entry_draft(
        human("a1"),
        4,
        3,
        Origin::Entered,
        vec![
            resolution(Transition::Abandon {
                reason: AbandonReason::Superseded,
                outcome: Outcome::default(),
            }),
            request(),
        ],
    ))
    .unwrap();
    let state = log.fold().unwrap();
    let r = &state.resolutions[&(GroupId::new("entreprise"), RowPath::root())];
    assert_eq!((r.status, r.requested_at), (ResolutionStatus::Pending, 3));
}

#[test]
fn override_wins_over_late_resolution() {
    // §2.8 rule 2 (RATIFIED), enforced by the fold. The applicant fills
    // `raison_sociale` by hand while the SIRET lookup is pending; the
    // lookup lands later. The human value stays; the late derivation is
    // retained on the cell as `superseded` — divergence visible, restore
    // one `set` away — and the suppressed write is reported.
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("raison_sociale", "ACME (typed)")],
    ))
    .unwrap();
    log.append(draft(
        resolver_actor(),
        1,
        1,
        Origin::Derived(derivation()),
        vec![
            set("raison_sociale", "ACME SARL"),
            set("adresse", "1 rue X"),
        ],
    ))
    .unwrap();
    let folded = log.fold().unwrap();
    assert_eq!(
        folded.values.cells.get(&addr("raison_sociale")),
        Some(&CellState::Value(CellValue::One(Scalar::Text(
            "ACME (typed)".into()
        ))))
    );
    assert_eq!(
        folded.provenance.get(&addr("raison_sociale")),
        Some(&Origin::Overridden {
            superseded: Some(derivation())
        })
    );
    // The untouched target still lands.
    assert_eq!(
        folded.values.cells.get(&addr("adresse")),
        Some(&CellState::Value(CellValue::One(Scalar::Text(
            "1 rue X".into()
        ))))
    );
    assert!(matches!(
        folded.provenance.get(&addr("adresse")),
        Some(Origin::Derived(_))
    ));
    assert_eq!(
        folded.suppressed,
        vec![varve_record::Suppressed {
            seq: 1,
            addr: addr("raison_sociale")
        }]
    );

    // A human writing over a derived cell becomes `overridden` with the
    // replaced derivation retained, even though the entry said `entered`
    // — provenance is derived, not copied (§2.7).
    log.append(draft(
        human("a1"),
        2,
        2,
        Origin::Entered,
        vec![set("adresse", "2 rue Y")],
    ))
    .unwrap();
    let folded = log.fold().unwrap();
    assert_eq!(
        folded.provenance.get(&addr("adresse")),
        Some(&Origin::Overridden {
            superseded: Some(derivation())
        })
    );
    // A later resolver write onto that override is refused too.
    log.append(draft(
        resolver_actor(),
        3,
        3,
        Origin::Derived(derivation()),
        vec![set("adresse", "3 rue Z")],
    ))
    .unwrap();
    let folded = log.fold().unwrap();
    assert_eq!(
        folded.values.cells.get(&addr("adresse")),
        Some(&CellState::Value(CellValue::One(Scalar::Text(
            "2 rue Y".into()
        ))))
    );
    assert_eq!(folded.suppressed.len(), 2);

    // Restore (§2.7): a *human* re-derives from the retained snapshot —
    // a deliberate act with a Derived origin, not a late machine write —
    // and it applies. Machine values never win by force; humans may
    // choose them.
    log.append(draft(
        human("a1"),
        4,
        4,
        Origin::Derived(derivation()),
        vec![set("adresse", "3 rue Z")],
    ))
    .unwrap();
    let folded = log.fold().unwrap();
    assert_eq!(
        folded.values.cells.get(&addr("adresse")),
        Some(&CellState::Value(CellValue::One(Scalar::Text(
            "3 rue Z".into()
        ))))
    );
    assert!(matches!(
        folded.provenance.get(&addr("adresse")),
        Some(Origin::Derived(_))
    ));
}

#[test]
fn checkpoint_freezes_its_surface_and_reports_writes_into_it() {
    use std::collections::BTreeSet;
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(entry_draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("name", "Dupont").into(), request()],
    ))
    .unwrap();
    // Taken through the applicant form: `name`, `raison_sociale`,
    // `adresse` and the repetition `g1` are frozen; the instructor's
    // `annotation` column is not on that surface. The checkpoint is an
    // entry (§2.9, settled 2026-08-19): its position pins the content.
    let checkpoint = Checkpoint {
        name: "submission".into(),
        reading_revision: RevisionId::new("rev-1"),
        expected: vec![expected()],
        frozen_columns: ["name", "raison_sociale", "adresse"]
            .into_iter()
            .map(ColumnId::new)
            .collect(),
        frozen_groups: [GroupId::new("g1")].into_iter().collect(),
    };
    log.append(entry_draft(
        human("a1"),
        1,
        1,
        Origin::Entered,
        vec![EntryOp::Checkpoint(checkpoint.clone())],
    ))
    .unwrap();
    let found = log.checkpoints();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].seq, &found[0].checkpoint), (1, &checkpoint));
    assert_eq!(found[0].entry_hash, log.entries()[1].hash());

    // Expected late derived write into the frozen set: legal.
    log.append(entry_draft(
        resolver_actor(),
        2,
        2,
        Origin::Derived(derivation()),
        vec![
            resolution(Transition::Land {
                snapshot: derivation().snapshot_ref,
                outcome: Outcome::default(),
            }),
            set("raison_sociale", "ACME SARL").into(),
        ],
    ))
    .unwrap();
    // An instructor annotating: outside the frozen set, not the
    // checkpoint's business (§2.9 — the record stays a case file).
    log.append(draft(
        human("instructor"),
        3,
        3,
        Origin::Entered,
        vec![set("annotation", "ok")],
    ))
    .unwrap();
    assert_eq!(validate_after_checkpoint(&log, 1), vec![]);

    // A human edit into the frozen set — even mixed with a legal
    // annotation write: reported, naming the frozen columns touched.
    log.append(draft(
        human("a1"),
        4,
        4,
        Origin::Entered,
        vec![set("name", "X"), set("annotation", "still fine")],
    ))
    .unwrap();
    assert_eq!(
        validate_after_checkpoint(&log, 1),
        vec![CheckpointViolation::IllegalWrite {
            seq: 4,
            columns: [ColumnId::new("name")].into_iter().collect(),
            groups: BTreeSet::new(),
        }]
    );

    // A derived write from a resolver NOT on the expected list, and an
    // item added to a frozen repetition by a human: both reported.
    let mut other = derivation();
    other.source = ResolverId::new("ban-address");
    log.append(draft(
        resolver_actor(),
        5,
        5,
        Origin::Derived(other),
        vec![set("adresse", "1 rue de la Paix")],
    ))
    .unwrap();
    log.append(draft(
        human("a1"),
        6,
        6,
        Origin::Entered,
        vec![Op::AddItem {
            group: GroupId::new("g1"),
            parent: RowPath::root(),
            item: ItemId::new("i9"),
            at: 0,
        }],
    ))
    .unwrap();
    let violations = validate_after_checkpoint(&log, 1);
    assert_eq!(violations.len(), 3);
    assert!(matches!(
        &violations[2],
        CheckpointViolation::IllegalWrite { seq: 6, groups, .. } if groups.contains(&GroupId::new("g1"))
    ));

    // A superseding checkpoint (back to construction) ends this one's
    // regime: entries after it are its business, not ours.
    log.append(entry_draft(
        human("a1"),
        7,
        7,
        Origin::Entered,
        vec![EntryOp::Checkpoint(Checkpoint {
            name: "reopened".into(),
            reading_revision: RevisionId::new("rev-1"),
            expected: vec![],
            frozen_columns: BTreeSet::new(),
            frozen_groups: BTreeSet::new(),
        })],
    ))
    .unwrap();
    log.append(draft(
        human("a1"),
        8,
        8,
        Origin::Entered,
        vec![set("name", "after reopening")],
    ))
    .unwrap();
    assert_eq!(validate_after_checkpoint(&log, 1).len(), 3);
    assert_eq!(validate_after_checkpoint(&log, 7), vec![]);
    assert_eq!(log.checkpoints().len(), 2);
    // Not a checkpoint entry: reported as such, nothing else.
    assert_eq!(
        validate_after_checkpoint(&log, 0),
        vec![CheckpointViolation::UnknownCheckpoint { seq: 0 }]
    );
    assert_eq!(
        validate_after_checkpoint(&log, 99),
        vec![CheckpointViolation::UnknownCheckpoint { seq: 99 }]
    );
}

#[test]
fn a_checkpoint_may_only_expect_what_is_pending() {
    // §2.8 rule 1 meets §2.9: the expectation names the versions bound
    // at request time; expecting other versions, or a lookup never
    // requested, is refused at append — a checkpoint cannot lie about
    // the lookups outstanding under it.
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(entry_draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![request()],
    ))
    .unwrap();
    let checkpoint = |expected: Vec<ExpectedResolution>| {
        EntryOp::Checkpoint(Checkpoint {
            name: "submission".into(),
            reading_revision: RevisionId::new("rev-1"),
            expected,
            frozen_columns: Default::default(),
            frozen_groups: Default::default(),
        })
    };
    let rebound = ExpectedResolution {
        resolver_version: 2,
        ..expected()
    };
    assert!(matches!(
        log.append(entry_draft(
            human("a1"),
            1,
            1,
            Origin::Entered,
            vec![checkpoint(vec![rebound.clone()])]
        )),
        Err(AppendError::IllegalTransition(LifecycleError::ExpectedNotPending(e))) if e == rebound
    ));
    let elsewhere = ExpectedResolution {
        scope: RowPath::root().child(PathSeg {
            group: GroupId::new("g1"),
            item: ItemId::new("i1"),
        }),
        ..expected()
    };
    assert!(matches!(
        log.append(entry_draft(
            human("a1"),
            1,
            1,
            Origin::Entered,
            vec![checkpoint(vec![elsewhere])]
        )),
        Err(AppendError::IllegalTransition(
            LifecycleError::ExpectedNotPending(_)
        ))
    ));
    // Two checkpoints in one entry: a checkpoint pins one position.
    assert!(matches!(
        log.append(entry_draft(
            human("a1"),
            1,
            1,
            Origin::Entered,
            vec![checkpoint(vec![]), checkpoint(vec![])]
        )),
        Err(AppendError::IllegalTransition(
            LifecycleError::MultipleCheckpoints
        ))
    ));
    // The request and the checkpoint may share an entry — submit —
    // as long as the request comes first.
    let mut log2 = RecordLog::new(RecordId::new("r2"));
    log2.append(entry_draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![request(), checkpoint(vec![expected()])],
    ))
    .unwrap();
    assert!(
        log2.append(entry_draft(
            human("a1"),
            1,
            1,
            Origin::Entered,
            vec![
                resolution(Transition::Abandon {
                    reason: AbandonReason::Operator,
                    outcome: Outcome::default()
                }),
                request(),
                checkpoint(vec![expected()]),
            ],
        ))
        .is_ok()
    );
    // Once landed it is not pending: a later checkpoint cannot expect it.
    log.append(entry_draft(
        resolver_actor(),
        2,
        1,
        Origin::Entered,
        vec![resolution(Transition::NotFound {
            outcome: Outcome::default(),
        })],
    ))
    .unwrap();
    assert!(matches!(
        log.append(entry_draft(
            human("a1"),
            3,
            2,
            Origin::Entered,
            vec![checkpoint(vec![expected()])]
        )),
        Err(AppendError::IllegalTransition(
            LifecycleError::ExpectedNotPending(_)
        ))
    ));
}
