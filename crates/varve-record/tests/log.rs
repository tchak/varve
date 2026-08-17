use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, GroupId, ItemId, PathSeg, RecordId, ResolverId, RevisionId, RowPath};
use varve_record::{
    Actor, ActorKind, AppendError, ChainError, Checkpoint, CheckpointViolation,
    Derivation, Draft, EntrySalts, ExpectedResolution, Origin, RecordLog,
    Resolution, ResolutionStatus, SaltCountMismatch, pending_resolutions,
    validate_after_checkpoint,
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
    let mut log = RecordLog::new(RecordId::new("r1"));
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
    let tampered = RecordLog::from_entries(RecordId::new("r1"), entries);
    assert!(tampered.verify_chain().is_err());
}

#[test]
fn a_log_verifies_only_under_its_own_record() {
    // The genesis commits to the record id (§2.9): record A's stored
    // log rehydrated under record B's id is a chain error at entry 0 —
    // logs cannot be transplanted. Still per-record: nothing global.
    let mut log = RecordLog::new(RecordId::new("A"));
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![set("name", "Dupont")]))
        .unwrap();
    log.verify_chain().unwrap();
    let transplanted = RecordLog::from_entries(RecordId::new("B"), log.entries().to_vec());
    assert_eq!(transplanted.verify_chain(), Err(ChainError::PrevMismatch { at: 0 }));
    let same = RecordLog::from_entries(RecordId::new("A"), log.entries().to_vec());
    assert_eq!(same.verify_chain(), Ok(()));
}

#[test]
fn injected_op_without_a_salt_is_detected() {
    // The commitment is a vector over (op, salt) pairs. An op appended
    // to a stored entry *without* a salt must not slip outside the
    // commitment: the chain must reject it, not verify around it.
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![set("name", "Dupont")]))
        .unwrap();
    log.append(draft(human("a1"), 1, 1, Origin::Entered, vec![set("city", "Lyon")]))
        .unwrap();

    let mut entries = log.entries().to_vec();
    entries[0].content.ops.push(set("name", "MALLORY"));
    let tampered = RecordLog::from_entries(RecordId::new("r1"), entries);
    assert_eq!(
        tampered.verify_chain(),
        Err(ChainError::SaltCount { at: 0, mismatch: SaltCountMismatch { ops: 2, salts: 1 } })
    );

    // Same for a tail entry — the last entry is otherwise unanchored.
    let mut entries = log.entries().to_vec();
    entries[1].content.ops.push(set("name", "MALLORY"));
    let tampered = RecordLog::from_entries(RecordId::new("r1"), entries);
    assert!(matches!(tampered.verify_chain(), Err(ChainError::SaltCount { at: 1, .. })));

    // And the mirror image: a salt without an op.
    let mut entries = log.entries().to_vec();
    entries[0].salts.ops.push(Salt([42; 32]));
    let tampered = RecordLog::from_entries(RecordId::new("r1"), entries);
    assert_eq!(
        tampered.verify_chain(),
        Err(ChainError::SaltCount { at: 0, mismatch: SaltCountMismatch { ops: 1, salts: 2 } })
    );
}

#[test]
fn append_refuses_a_salt_count_mismatch() {
    let mut log = RecordLog::new(RecordId::new("r1"));
    let mut d = draft(human("a1"), 0, 0, Origin::Entered, vec![set("a", "1"), set("b", "2")]);
    d.salts = salts(1);
    assert_eq!(
        log.append(d).map(|_| ()),
        Err(AppendError::SaltCount(SaltCountMismatch { ops: 2, salts: 1 }))
    );
    assert_eq!(log.version(), 0);
}

#[test]
fn conflicts_are_detected_not_merged() {
    let mut log = RecordLog::new(RecordId::new("r1"));
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
    let mut log = RecordLog::new(RecordId::new("r1"));
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
    let mut log = RecordLog::new(RecordId::new("r1"));
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
    let mut log = RecordLog::new(RecordId::new("r1"));
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

    let blobs = log.referenced_blobs(&[]);
    assert!(blobs.contains(&blob("v1")));
    assert!(blobs.contains(&blob("v2")));
    assert!(blobs.contains(&derivation().snapshot_ref));
    assert_eq!(blobs.len(), 3);

    // A payload that landed on a resolution instance is a root too —
    // it may be referenced from nowhere else (§2.8 rule 2).
    let mut resolution = pending_resolution();
    resolution.land(blob("payload")).unwrap();
    let blobs = log.referenced_blobs(&[resolution]);
    assert!(blobs.contains(&blob("payload")));
    assert_eq!(blobs.len(), 4);
}

fn pending_resolution() -> Resolution {
    Resolution {
        resolver: ResolverId::new("insee-sirene"),
        resolver_version: 1,
        mapping_version: 1,
        scope: RowPath::root(),
        status: ResolutionStatus::Pending,
        attempts: 0,
        last_error: None,
        deadline: None,
        snapshot: None,
    }
}

#[test]
fn scan_lifecycle() {
    use varve_record::{Scan, ScanStatus, pending_scans};
    let mut scan = Scan {
        element: "f1".into(),
        hash: varve_record::genesis_hash(&RecordId::new("r1")),
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
    use varve_record::{TransitionError, pending_set};
    let mut resolution = pending_resolution();
    let list = [resolution.clone()];
    assert_eq!(pending_resolutions(&list).len(), 1);
    // What the logic language reads: (scope, resolver) pairs.
    assert_eq!(
        pending_set(&list),
        [(RowPath::root(), ResolverId::new("insee-sirene"))].into_iter().collect()
    );

    resolution.transition(ResolutionStatus::Failed).unwrap();
    resolution.transition(ResolutionStatus::Pending).unwrap();
    assert_eq!(resolution.attempts, 1);
    // A resolution never resolves without its payload (§2.7).
    assert_eq!(
        resolution.transition(ResolutionStatus::Resolved),
        Err(TransitionError::ResolvedWithoutSnapshot)
    );
    let payload = derivation().snapshot_ref;
    resolution.land(payload).unwrap();
    assert_eq!(resolution.status, ResolutionStatus::Resolved);
    assert_eq!(resolution.snapshot, Some(payload));
    assert!(pending_set(&[resolution.clone()]).is_empty());
    // Terminal: no way back, and abandonment of a resolved instance is
    // meaningless.
    assert!(resolution.transition(ResolutionStatus::Pending).is_err());
    assert!(resolution.transition(ResolutionStatus::Abandoned).is_err());
    assert!(resolution.land(payload).is_err());
}

#[test]
fn override_wins_over_late_resolution() {
    // §2.8 rule 2 (RATIFIED), enforced by the fold. The applicant fills
    // `raison_sociale` by hand while the SIRET lookup is pending; the
    // lookup lands later. The human value stays; the late derivation is
    // retained on the cell as `superseded` — divergence visible, restore
    // one `set` away — and the suppressed write is reported.
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![set("raison_sociale", "ACME (typed)")]))
        .unwrap();
    log.append(draft(
        resolver_actor(),
        1,
        1,
        Origin::Derived(derivation()),
        vec![set("raison_sociale", "ACME SARL"), set("adresse", "1 rue X")],
    ))
    .unwrap();
    let folded = log.fold().unwrap();
    assert_eq!(
        folded.values.cells.get(&addr("raison_sociale")),
        Some(&CellState::Value(CellValue::One(Scalar::Text("ACME (typed)".into()))))
    );
    assert_eq!(
        folded.provenance.get(&addr("raison_sociale")),
        Some(&Origin::Overridden { superseded: Some(derivation()) })
    );
    // The untouched target still lands.
    assert_eq!(
        folded.values.cells.get(&addr("adresse")),
        Some(&CellState::Value(CellValue::One(Scalar::Text("1 rue X".into()))))
    );
    assert!(matches!(folded.provenance.get(&addr("adresse")), Some(Origin::Derived(_))));
    assert_eq!(
        folded.suppressed,
        vec![varve_record::Suppressed { seq: 1, addr: addr("raison_sociale") }]
    );

    // A human writing over a derived cell becomes `overridden` with the
    // replaced derivation retained, even though the entry said `entered`
    // — provenance is derived, not copied (§2.7).
    log.append(draft(human("a1"), 2, 2, Origin::Entered, vec![set("adresse", "2 rue Y")]))
        .unwrap();
    let folded = log.fold().unwrap();
    assert_eq!(
        folded.provenance.get(&addr("adresse")),
        Some(&Origin::Overridden { superseded: Some(derivation()) })
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
        Some(&CellState::Value(CellValue::One(Scalar::Text("2 rue Y".into()))))
    );
    assert_eq!(folded.suppressed.len(), 2);

    // Restore (§2.7): a *human* re-derives from the retained snapshot —
    // a deliberate act with a Derived origin, not a late machine write —
    // and it applies. Machine values never win by force; humans may
    // choose them.
    log.append(draft(human("a1"), 4, 4, Origin::Derived(derivation()), vec![set("adresse", "3 rue Z")]))
        .unwrap();
    let folded = log.fold().unwrap();
    assert_eq!(
        folded.values.cells.get(&addr("adresse")),
        Some(&CellState::Value(CellValue::One(Scalar::Text("3 rue Z".into()))))
    );
    assert!(matches!(folded.provenance.get(&addr("adresse")), Some(Origin::Derived(_))));
}

#[test]
fn checkpoint_freezes_its_surface_and_reports_writes_into_it() {
    use std::collections::BTreeSet;
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(human("a1"), 0, 0, Origin::Entered, vec![set("name", "Dupont")]))
        .unwrap();
    // Taken through the applicant form: `name`, `raison_sociale`,
    // `adresse` and the repetition `g1` are frozen; the instructor's
    // `annotation` column is not on that surface.
    let checkpoint = Checkpoint {
        name: "submission".into(),
        entry: log.entries()[0].hash(),
        reading_revision: RevisionId::new("rev-1"),
        expected: vec![ExpectedResolution {
            resolver: ResolverId::new("insee-sirene"),
            scope: RowPath::root(),
        }],
        frozen_columns: ["name", "raison_sociale", "adresse"]
            .into_iter()
            .map(ColumnId::new)
            .collect(),
        frozen_groups: [GroupId::new("g1")].into_iter().collect(),
    };

    // Expected late derived write into the frozen set: legal.
    log.append(draft(
        resolver_actor(),
        1,
        1,
        Origin::Derived(derivation()),
        vec![set("raison_sociale", "ACME SARL")],
    ))
    .unwrap();
    // An instructor annotating: outside the frozen set, not the
    // checkpoint's business (§2.9 — the record stays a case file).
    log.append(draft(human("instructor"), 2, 2, Origin::Entered, vec![set("annotation", "ok")]))
        .unwrap();
    assert_eq!(validate_after_checkpoint(&log, &checkpoint, None), vec![]);

    // A human edit into the frozen set — even mixed with a legal
    // annotation write: reported, naming the frozen columns touched.
    log.append(draft(
        human("a1"),
        3,
        3,
        Origin::Entered,
        vec![set("name", "X"), set("annotation", "still fine")],
    ))
    .unwrap();
    assert_eq!(
        validate_after_checkpoint(&log, &checkpoint, None),
        vec![CheckpointViolation::IllegalWrite {
            seq: 3,
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
        4,
        4,
        Origin::Derived(other),
        vec![set("adresse", "1 rue de la Paix")],
    ))
    .unwrap();
    log.append(draft(
        human("a1"),
        5,
        5,
        Origin::Entered,
        vec![Op::AddItem {
            group: GroupId::new("g1"),
            parent: RowPath::root(),
            item: ItemId::new("i9"),
            at: 0,
        }],
    ))
    .unwrap();
    let violations = validate_after_checkpoint(&log, &checkpoint, None);
    assert_eq!(violations.len(), 3);
    assert!(matches!(
        &violations[2],
        CheckpointViolation::IllegalWrite { seq: 5, groups, .. } if groups.contains(&GroupId::new("g1"))
    ));

    // Out-of-scope derived write (targets outside the expected scope).
    let scoped = Checkpoint {
        expected: vec![ExpectedResolution {
            resolver: ResolverId::new("insee-sirene"),
            scope: RowPath::root().child(PathSeg {
                group: GroupId::new("g1"),
                item: ItemId::new("i1"),
            }),
        }],
        ..checkpoint.clone()
    };
    assert!(
        validate_after_checkpoint(&log, &scoped, None)
            .iter()
            .any(|v| matches!(v, CheckpointViolation::IllegalWrite { seq: 1, .. }))
    );

    // A superseding checkpoint (back to construction) ends this one's
    // regime: entries after it are its business, not ours.
    let reopened = Checkpoint {
        name: "reopened".into(),
        entry: log.entries()[2].hash(),
        expected: vec![],
        frozen_columns: BTreeSet::new(),
        frozen_groups: BTreeSet::new(),
        ..checkpoint.clone()
    };
    assert_eq!(validate_after_checkpoint(&log, &checkpoint, Some(&reopened)), vec![]);
    assert_eq!(validate_after_checkpoint(&log, &reopened, None), vec![]);
    // A superseding checkpoint must come after this one.
    let earlier = Checkpoint { entry: varve_record::genesis_hash(&RecordId::new("r1")), ..reopened.clone() };
    assert_eq!(
        validate_after_checkpoint(&log, &checkpoint, Some(&earlier)),
        vec![CheckpointViolation::UnknownSupersedingEntry]
    );
    let unknown = Checkpoint { entry: varve_record::genesis_hash(&RecordId::new("r1")), ..checkpoint.clone() };
    assert_eq!(
        validate_after_checkpoint(&log, &unknown, None),
        vec![CheckpointViolation::UnknownEntry]
    );
}
