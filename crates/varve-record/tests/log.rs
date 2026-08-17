use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, GroupId, ItemId, PathSeg, ResolverId, RevisionId, RowPath};
use varve_record::{
    Actor, ActorKind, Checkpoint, CheckpointViolation, Derivation, Draft,
    EntrySalts, ExpectedResolution, Origin, RecordLog, Resolution,
    ResolutionStatus, pending_resolutions, validate_after_checkpoint,
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
        ops,
        salts: salts(n),
    }
}

fn derivation() -> Derivation {
    Derivation {
        source: ResolverId::new("insee-sirene"),
        source_version: 1,
        mapping_version: 1,
        snapshot_ref: varve_record::genesis_hash(),
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
    let mut log = RecordLog::new();
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![set("name", "Dupont")]))
        .unwrap();
    log.append(draft(
        resolver_actor(),
        1,
        1,
        Origin::Derived(derivation()),
        vec![set("raison_sociale", "ACME SARL")],
    ))
    .unwrap();
    log.append(draft(human("a1"), 2, 2, Origin::Entered, vec![set("name", "Durand")]))
        .unwrap();

    log.verify_chain().unwrap();

    let folded = log.fold().unwrap();
    assert_eq!(
        folded.values.cells.get(&addr("name")),
        Some(&CellState::Value(CellValue::One(Scalar::Text("Durand".into()))))
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
    let mut log = RecordLog::new();
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![set("name", "Dupont")]))
        .unwrap();
    log.append(draft(human("a1"), 1, 1, Origin::Entered, vec![set("name", "Durand")]))
        .unwrap();
    log.verify_chain().unwrap();

    // A "quiet correction" of history: rewrite the value inside a
    // stored entry, then rehydrate — as an attacker with storage access
    // would.
    let mut entries = log.entries().to_vec();
    if let Op::Set { state, .. } = &mut entries[0].content.ops[0] {
        *state = CellState::Value(CellValue::One(Scalar::Text("Martin".into())));
    }
    let tampered = RecordLog::from_entries(entries);
    assert!(tampered.verify_chain().is_err());
}

#[test]
fn conflicts_are_detected_not_merged() {
    let mut log = RecordLog::new();
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![set("name", "Dupont")]))
        .unwrap();
    // Two instructors both edit from version 1.
    log.append(draft(human("a2"), 1, 1, Origin::Entered, vec![set("name", "Durand")]))
        .unwrap();
    log.append(draft(human("a3"), 2, 1, Origin::Entered, vec![set("name", "Martin")]))
        .unwrap();
    // A fourth write that saw everything: no conflict.
    log.append(draft(human("a2"), 3, 3, Origin::Entered, vec![set("name", "Bernard")]))
        .unwrap();

    let conflicts = log.detect_conflicts();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].addr, addr("name"));
    assert_eq!((conflicts[0].earlier, conflicts[0].later), (1, 2));
    // LWW still holds: last write wins in the fold.
    let folded = log.fold().unwrap();
    assert_eq!(
        folded.values.cells.get(&addr("name")),
        Some(&CellState::Value(CellValue::One(Scalar::Text("Bernard".into()))))
    );
}

#[test]
fn diff_between_log_points() {
    let mut log = RecordLog::new();
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![set("name", "Dupont")]))
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
    let mut log = RecordLog::new();
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![set("name", "Dupont")]))
        .unwrap();
    log.append(draft(human("a1"), 1, 1, Origin::Entered, vec![set("name", "Durand")]))
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
    let file = |name: &str, content: &str| {
        Op::Set {
            column: ColumnId::new("piece"),
            path: RowPath::root(),
            state: CellState::Value(CellValue::Many(vec![Scalar::Attachment(
                Box::new(AttachmentRef {
                    id: name.into(),
                    hash: blob(content),
                    filename: format!("{name}.pdf"),
                    content_type: "application/pdf".into(),
                    byte_size: 10,
                }),
            )])),
        }
    };
    let mut log = RecordLog::new();
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![file("f1", "v1")]))
        .unwrap();
    // The file is replaced — the superseded blob must STILL be a root:
    // erasure covers history, so GC must not collect what the log
    // references.
    log.append(draft(human("a1"), 1, 1, Origin::Entered, vec![file("f1", "v2")]))
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
}

#[test]
fn scan_lifecycle() {
    use varve_record::{Scan, ScanStatus, pending_scans};
    let mut scan = Scan {
        element: "f1".into(),
        hash: varve_record::genesis_hash(),
        status: ScanStatus::Pending,
        attempts: 0,
    };
    assert_eq!(pending_scans(std::slice::from_ref(&scan)).len(), 1);
    scan.transition(ScanStatus::Failed).unwrap();
    scan.transition(ScanStatus::Pending).unwrap();
    assert_eq!(scan.attempts, 1);
    scan.transition(ScanStatus::Clean).unwrap();
    // Terminal: no rescanning a clean verdict into anything else.
    assert!(scan.transition(ScanStatus::Pending).is_err());
    assert!(scan.transition(ScanStatus::Infected).is_err());
}

#[test]
fn resolution_lifecycle() {
    let mut resolution = Resolution {
        resolver: ResolverId::new("insee-sirene"),
        resolver_version: 1,
        mapping_version: 1,
        scope: RowPath::root(),
        status: ResolutionStatus::Pending,
        attempts: 0,
        last_error: None,
        deadline: None,
    };
    let list = [resolution.clone()];
    assert_eq!(pending_resolutions(&list).len(), 1);

    resolution.transition(ResolutionStatus::Failed).unwrap();
    resolution.transition(ResolutionStatus::Pending).unwrap();
    assert_eq!(resolution.attempts, 1);
    resolution.transition(ResolutionStatus::Resolved).unwrap();
    // Terminal: no way back, and abandonment of a resolved instance is
    // meaningless.
    assert!(resolution.transition(ResolutionStatus::Pending).is_err());
    assert!(resolution.transition(ResolutionStatus::Abandoned).is_err());
}

#[test]
fn checkpoint_rejects_unexpected_late_writes() {
    let mut log = RecordLog::new();
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![set("name", "Dupont")]))
        .unwrap();
    let checkpoint = Checkpoint {
        name: "submission".into(),
        entry: log.entries()[0].hash(),
        reading_revision: RevisionId::new("rev-1"),
        expected: vec![ExpectedResolution {
            resolver: ResolverId::new("insee-sirene"),
            scope: RowPath::root(),
        }],
    };

    // Expected late derived write: legal.
    log.append(draft(
        resolver_actor(),
        1,
        1,
        Origin::Derived(derivation()),
        vec![set("raison_sociale", "ACME SARL")],
    ))
    .unwrap();
    assert_eq!(validate_after_checkpoint(&log, &checkpoint), vec![]);

    // A human edit after the checkpoint: rejected.
    log.append(draft(human("a1"), 2, 2, Origin::Entered, vec![set("name", "X")]))
        .unwrap();
    assert_eq!(
        validate_after_checkpoint(&log, &checkpoint),
        vec![CheckpointViolation::IllegalWrite { seq: 2 }]
    );

    // A derived write from a resolver NOT on the expected list: rejected.
    let mut other = derivation();
    other.source = ResolverId::new("ban-address");
    log.append(draft(
        resolver_actor(),
        3,
        3,
        Origin::Derived(other),
        vec![set("adresse", "1 rue de la Paix")],
    ))
    .unwrap();
    assert_eq!(validate_after_checkpoint(&log, &checkpoint).len(), 2);

    // Out-of-scope derived write (targets outside the expected scope).
    let scoped = Checkpoint {
        expected: vec![ExpectedResolution {
            resolver: ResolverId::new("insee-sirene"),
            scope: RowPath::root().child(PathSeg {
                group: GroupId::new("g1"),
                item: ItemId::new("i1"),
            }),
        }],
        ..checkpoint
    };
    assert!(
        validate_after_checkpoint(&log, &scoped)
            .contains(&CheckpointViolation::IllegalWrite { seq: 1 })
    );
}
