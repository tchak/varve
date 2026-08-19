//! Checkpoint negatives (§2.8, §2.9): the entries a checkpoint cannot
//! locate, the superseding regime, and every way a write into the
//! frozen set fails to be "the expected derived write".

use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, GroupId, ItemId, PathSeg, RecordId, ResolverId, RevisionId, RowPath};
use varve_record::{
    Actor, ActorKind, Checkpoint, CheckpointViolation, Derivation, Draft, EntrySalts,
    ExpectedResolution, Origin, RecordLog, validate_after_checkpoint,
};
use varve_value::{CellState, CellValue, Op, Scalar};

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

fn set_at(column: &str, path: RowPath, value: &str) -> Op {
    Op::Set {
        column: ColumnId::new(column),
        path,
        state: CellState::Value(CellValue::One(Scalar::Text(value.into()))),
    }
}

fn set(column: &str, value: &str) -> Op {
    set_at(column, RowPath::root(), value)
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

fn item(i: &str) -> RowPath {
    RowPath::root().child(PathSeg {
        group: GroupId::new("g1"),
        item: ItemId::new(i),
    })
}

/// A checkpoint on entry `at`, freezing `name`/`raison_sociale`/`adresse`
/// and the repetition `g1`, expecting insee-sirene@1/1 at `scope`.
fn checkpoint(log: &RecordLog, at: usize, scope: RowPath) -> Checkpoint {
    Checkpoint {
        name: "submission".into(),
        entry: log.entries()[at].hash(),
        reading_revision: RevisionId::new("rev-1"),
        expected: vec![ExpectedResolution {
            resolver: ResolverId::new("insee-sirene"),
            scope,
            resolver_version: 1,
            mapping_version: 1,
        }],
        frozen_columns: ["name", "raison_sociale", "adresse"]
            .into_iter()
            .map(ColumnId::new)
            .collect(),
        frozen_groups: [GroupId::new("g1")].into_iter().collect(),
    }
}

/// One entry: name=Dupont, then the checkpoint on it.
fn base() -> (RecordLog, Checkpoint) {
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![set("name", "Dupont")],
    ))
    .unwrap();
    let cp = checkpoint(&log, 0, RowPath::root());
    (log, cp)
}

fn illegal(seq: u64, columns: &[&str], groups: &[&str]) -> CheckpointViolation {
    CheckpointViolation::IllegalWrite {
        seq,
        columns: columns.iter().map(|c| ColumnId::new(*c)).collect(),
        groups: groups.iter().map(|g| GroupId::new(*g)).collect(),
    }
}

#[test]
fn a_checkpoint_naming_an_unknown_entry_reports_only_that() {
    let (log, mut cp) = base();
    cp.entry = varve_record::genesis_hash(&RecordId::new("elsewhere"));
    assert_eq!(
        validate_after_checkpoint(&log, &cp, None),
        vec![CheckpointViolation::UnknownEntry]
    );
}

#[test]
fn a_superseding_checkpoint_must_name_a_later_entry_of_this_log() {
    let (mut log, cp) = base();
    log.append(draft(
        human("a1"),
        1,
        1,
        Origin::Entered,
        vec![set("name", "X")],
    ))
    .unwrap();
    log.append(draft(
        human("a1"),
        2,
        2,
        Origin::Entered,
        vec![set("annotation", "ok")],
    ))
    .unwrap();

    // Not in the log at all.
    let mut ghost = checkpoint(&log, 2, RowPath::root());
    ghost.entry = varve_record::genesis_hash(&RecordId::new("elsewhere"));
    assert_eq!(
        validate_after_checkpoint(&log, &cp, Some(&ghost)),
        vec![CheckpointViolation::UnknownSupersedingEntry]
    );
    // The same entry as the checkpoint itself: not *after* it.
    let same = checkpoint(&log, 0, RowPath::root());
    assert_eq!(
        validate_after_checkpoint(&log, &cp, Some(&same)),
        vec![CheckpointViolation::UnknownSupersedingEntry]
    );
    // Before it: a superseding checkpoint cannot precede what it
    // supersedes.
    let later = checkpoint(&log, 2, RowPath::root());
    let earlier = checkpoint(&log, 0, RowPath::root());
    assert_eq!(
        validate_after_checkpoint(&log, &later, Some(&earlier)),
        vec![CheckpointViolation::UnknownSupersedingEntry]
    );
}

#[test]
fn the_superseding_entry_is_judged_under_the_old_regime_and_nothing_after_it_is() {
    // DN's "back to construction": the applicant edits (entry 1 — an
    // illegal write under the first checkpoint), then resubmits at
    // entry 2, which the second checkpoint names. Entry 2's own frozen
    // write is the last one the first checkpoint sees; entry 3 belongs
    // to the second regime only.
    let (mut log, first) = base();
    log.append(draft(
        human("a1"),
        1,
        1,
        Origin::Entered,
        vec![set("name", "edited")],
    ))
    .unwrap();
    log.append(draft(
        human("a1"),
        2,
        2,
        Origin::Entered,
        vec![set("adresse", "resubmitted")],
    ))
    .unwrap();
    let second = checkpoint(&log, 2, RowPath::root());
    log.append(draft(
        human("a1"),
        3,
        3,
        Origin::Entered,
        vec![set("name", "after")],
    ))
    .unwrap();

    assert_eq!(
        validate_after_checkpoint(&log, &first, Some(&second)),
        vec![illegal(1, &["name"], &[]), illegal(2, &["adresse"], &[])]
    );
    assert_eq!(
        validate_after_checkpoint(&log, &second, None),
        vec![illegal(3, &["name"], &[])]
    );
    // Without the superseder, the first checkpoint sees everything.
    assert_eq!(
        validate_after_checkpoint(&log, &first, None),
        vec![
            illegal(1, &["name"], &[]),
            illegal(2, &["adresse"], &[]),
            illegal(3, &["name"], &[])
        ]
    );
}

#[test]
fn an_expected_write_must_match_the_bound_versions_exactly() {
    // §2.8 rule 1: versions bind at request time. A derived write under
    // another resolver or mapping version is a re-map, not the expected
    // resolution — reported.
    let (mut log, cp) = base();
    let mut other_resolver = derivation();
    other_resolver.source_version = 2;
    let mut other_mapping = derivation();
    other_mapping.mapping_version = 2;
    log.append(draft(
        resolver_actor(),
        1,
        1,
        Origin::Derived(other_resolver),
        vec![set("adresse", "x")],
    ))
    .unwrap();
    log.append(draft(
        resolver_actor(),
        2,
        2,
        Origin::Derived(other_mapping),
        vec![set("adresse", "y")],
    ))
    .unwrap();
    // The exact versions: legal.
    log.append(draft(
        resolver_actor(),
        3,
        3,
        Origin::Derived(derivation()),
        vec![set("adresse", "z")],
    ))
    .unwrap();
    assert_eq!(
        validate_after_checkpoint(&log, &cp, None),
        vec![illegal(1, &["adresse"], &[]), illegal(2, &["adresse"], &[])]
    );
}

#[test]
fn an_expected_write_must_stay_within_the_expected_scope() {
    // The expectation is per group instance (§2.8): a resolution
    // expected on item i1 does not license a root-level write, nor a
    // write into item i2.
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(
        human("a1"),
        0,
        0,
        Origin::Entered,
        vec![
            Op::AddItem {
                group: GroupId::new("g1"),
                parent: RowPath::root(),
                item: ItemId::new("i1"),
                at: 0,
            },
            Op::AddItem {
                group: GroupId::new("g1"),
                parent: RowPath::root(),
                item: ItemId::new("i2"),
                at: 1,
            },
        ],
    ))
    .unwrap();
    let cp = checkpoint(&log, 0, item("i1"));
    log.append(draft(
        resolver_actor(),
        1,
        1,
        Origin::Derived(derivation()),
        vec![set("adresse", "root")],
    ))
    .unwrap();
    log.append(draft(
        resolver_actor(),
        2,
        2,
        Origin::Derived(derivation()),
        vec![set_at("adresse", item("i2"), "sibling")],
    ))
    .unwrap();
    log.append(draft(
        resolver_actor(),
        3,
        3,
        Origin::Derived(derivation()),
        vec![set_at("adresse", item("i1"), "own")],
    ))
    .unwrap();
    // One entry mixing an in-scope and an out-of-scope op: not wholly
    // within, so not expected.
    log.append(draft(
        resolver_actor(),
        4,
        4,
        Origin::Derived(derivation()),
        vec![set_at("adresse", item("i1"), "own"), set("adresse", "root")],
    ))
    .unwrap();
    assert_eq!(
        validate_after_checkpoint(&log, &cp, None),
        vec![
            illegal(1, &["adresse"], &[]),
            illegal(2, &["adresse"], &[]),
            illegal(4, &["adresse"], &[])
        ]
    );
}

#[test]
fn only_a_resolver_actor_with_a_derived_origin_can_be_expected() {
    // A human re-deriving from a snapshot (§2.7 restore) is a deliberate
    // human act, not the expected late resolution; a resolver writing
    // as `entered` is not a derived write at all. Both are reported.
    let (mut log, cp) = base();
    log.append(draft(
        human("a1"),
        1,
        1,
        Origin::Derived(derivation()),
        vec![set("adresse", "x")],
    ))
    .unwrap();
    log.append(draft(
        resolver_actor(),
        2,
        2,
        Origin::Entered,
        vec![set("adresse", "y")],
    ))
    .unwrap();
    // `overridden` carrying the same derivation is not `derived` either.
    log.append(draft(
        resolver_actor(),
        3,
        3,
        Origin::Overridden {
            superseded: Some(derivation()),
        },
        vec![set("adresse", "z")],
    ))
    .unwrap();
    assert_eq!(
        validate_after_checkpoint(&log, &cp, None),
        vec![
            illegal(1, &["adresse"], &[]),
            illegal(2, &["adresse"], &[]),
            illegal(3, &["adresse"], &[])
        ]
    );
}

#[test]
fn a_mixed_write_names_every_frozen_column_and_group_it_touched() {
    let (mut log, cp) = base();
    log.append(draft(
        human("a1"),
        1,
        1,
        Origin::Entered,
        vec![
            set("name", "X"),
            set("adresse", "Y"),
            set("annotation", "not frozen"),
            Op::AddItem {
                group: GroupId::new("g1"),
                parent: RowPath::root(),
                item: ItemId::new("i1"),
                at: 0,
            },
            Op::AddItem {
                group: GroupId::new("other"),
                parent: RowPath::root(),
                item: ItemId::new("i1"),
                at: 0,
            },
        ],
    ))
    .unwrap();
    // Unset and reorder/remove count as writes too.
    log.append(draft(
        human("a1"),
        2,
        2,
        Origin::Entered,
        vec![
            Op::Unset {
                column: ColumnId::new("name"),
                path: RowPath::root(),
            },
            Op::Reorder {
                group: GroupId::new("g1"),
                parent: RowPath::root(),
                order: vec![ItemId::new("i1")],
            },
        ],
    ))
    .unwrap();
    log.append(draft(
        human("a1"),
        3,
        3,
        Origin::Entered,
        vec![Op::RemoveItem {
            group: GroupId::new("g1"),
            parent: RowPath::root(),
            item: ItemId::new("i1"),
        }],
    ))
    .unwrap();
    assert_eq!(
        validate_after_checkpoint(&log, &cp, None),
        vec![
            illegal(1, &["adresse", "name"], &["g1"]),
            illegal(2, &["name"], &["g1"]),
            illegal(3, &[], &["g1"]),
        ]
    );
}
