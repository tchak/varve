//! The history-mode wire law (§5, M3) over the **full** op / origin /
//! scalar space: logs built from generated record states — every cell
//! op kind reached through `diff`, every lifecycle op kind (§2.8
//! resolution transitions, §2.9 checkpoints) appended legally against
//! the fold — under every actor kind, every origin shape and notes,
//! written as `entry` lines, read back identical, byte stable, and
//! adopted as the same chain (§6).

mod common;

use std::collections::BTreeMap;

use proptest::prelude::*;
use varve_core::canonical::{CanonicalValue, hash_plain};
use varve_core::primitives::Instant;
use varve_core::{GroupId, ItemId, PathSeg, RecordId, ResolverId, RowPath};
use varve_record::{
    AbandonReason, Actor, ActorKind, Checkpoint, Derivation, Draft, EntryOp, ExpectedResolution,
    Origin, Outcome, RecordLog, ScanTransition, Transition,
};
use varve_value::{RecordValues, diff};
use varve_wire::{
    Intent, Line, Mode, adopt_history, read_stream, test_salts, write_history, write_lines,
};

/// Everything an entry carries besides its ops (§2.9/§2.13): actor,
/// timestamp, origin, note.
#[derive(Debug, Clone)]
struct Meta {
    actor: Actor,
    timestamp: Instant,
    origin: Origin,
    note: Option<String>,
}

fn derivation() -> impl Strategy<Value = Derivation> {
    ("[a-z-]{1,8}", any::<u32>(), any::<u32>(), "[a-z]{0,6}").prop_map(
        |(source, source_version, mapping_version, payload)| Derivation {
            source: ResolverId::new(source),
            source_version,
            mapping_version,
            snapshot_ref: hash_plain(&CanonicalValue::String(payload)).unwrap(),
        },
    )
}

fn origin() -> impl Strategy<Value = Origin> {
    prop_oneof![
        Just(Origin::Entered),
        derivation().prop_map(Origin::Derived),
        Just(Origin::Overridden { superseded: None }),
        derivation().prop_map(|d| Origin::Overridden {
            superseded: Some(d)
        }),
    ]
}

fn actor() -> impl Strategy<Value = Actor> {
    (
        "[a-z0-9:-]{1,10}",
        prop_oneof![
            Just(ActorKind::Human),
            Just(ActorKind::Resolver),
            Just(ActorKind::System)
        ],
    )
        .prop_map(|(id, kind)| Actor { id, kind })
}

fn meta() -> impl Strategy<Value = Meta> {
    (
        actor(),
        common::instant(),
        origin(),
        proptest::option::of("\\PC{0,16}"),
    )
        .prop_map(|(actor, timestamp, origin, note)| Meta {
            actor,
            timestamp,
            origin,
            note,
        })
}

/// One lifecycle step to append after the cell history: an instance
/// (anchor, scope), a transition, and whether a checkpoint follows in
/// the same entry. `build_log` makes each step legal against the fold.
#[derive(Debug, Clone)]
struct LifecycleStep {
    anchor: GroupId,
    scope: RowPath,
    transition: Transition,
    /// An attachment scan step (§2.15) riding the same entry, if any.
    scan: Option<(String, ScanTransition)>,
    checkpoint: bool,
}

fn scan_transition() -> impl Strategy<Value = ScanTransition> {
    prop_oneof![
        "[a-z]{0,6}".prop_map(|bytes| ScanTransition::Request {
            hash: hash_plain(&CanonicalValue::String(bytes)).unwrap(),
        }),
        outcome().prop_map(|outcome| ScanTransition::Clean { outcome }),
        (proptest::option::of("[A-Za-z.-]{1,12}"), outcome())
            .prop_map(|(threat, outcome)| ScanTransition::Infected { threat, outcome }),
        outcome().prop_map(|outcome| ScanTransition::Failed { outcome }),
        (
            prop_oneof![
                Just(AbandonReason::Deadline),
                Just(AbandonReason::Operator),
                Just(AbandonReason::Unavailable),
                Just(AbandonReason::Superseded),
            ],
            outcome()
        )
            .prop_map(|(reason, outcome)| ScanTransition::Abandon { reason, outcome }),
    ]
}

fn outcome() -> impl Strategy<Value = Outcome> {
    (any::<u32>(), proptest::option::of("\\PC{0,12}")).prop_map(|(attempts, last_error)| Outcome {
        attempts,
        last_error,
    })
}

fn transition() -> impl Strategy<Value = Transition> {
    prop_oneof![
        ("[a-z-]{1,8}", any::<u32>(), any::<u32>()).prop_map(|(r, rv, mv)| Transition::Request {
            resolver: ResolverId::new(r),
            resolver_version: rv,
            mapping_version: mv,
        }),
        ("[a-z]{0,6}", outcome()).prop_map(|(payload, outcome)| Transition::Land {
            snapshot: hash_plain(&CanonicalValue::String(payload)).unwrap(),
            outcome,
        }),
        outcome().prop_map(|outcome| Transition::NotFound { outcome }),
        outcome().prop_map(|outcome| Transition::Ambiguous { outcome }),
        outcome().prop_map(|outcome| Transition::Failed { outcome }),
        (
            prop_oneof![
                Just(AbandonReason::Deadline),
                Just(AbandonReason::Operator),
                Just(AbandonReason::Unavailable),
                Just(AbandonReason::Superseded),
            ],
            outcome()
        )
            .prop_map(|(reason, outcome)| Transition::Abandon { reason, outcome }),
    ]
}

fn lifecycle_step() -> impl Strategy<Value = LifecycleStep> {
    (
        "[a-c]",
        prop_oneof![
            Just(RowPath::root()),
            "[a-z0-9]{1,3}".prop_map(|i| RowPath::root().child(PathSeg {
                group: GroupId::new("rep"),
                item: ItemId::new(i),
            })),
        ],
        transition(),
        proptest::option::of(("[f][1-3]", scan_transition())),
        any::<bool>(),
    )
        .prop_map(
            |(anchor, scope, transition, scan, checkpoint)| LifecycleStep {
                anchor: GroupId::new(anchor),
                scope,
                transition,
                scan,
                checkpoint,
            },
        )
}

/// One record's history: successive target states, each with the
/// metadata of the entry that reaches it, then lifecycle steps.
fn record_history() -> impl Strategy<Value = (Vec<(RecordValues, Meta)>, Vec<LifecycleStep>)> {
    (
        proptest::collection::vec((common::shared_universe_values(), meta()), 1..=4),
        proptest::collection::vec(lifecycle_step(), 0..=3),
    )
}

/// Build the log: `diff(previous, next)` is the entry's op list, so
/// every entry applies by construction. A resolver's derived write onto
/// a human-authored cell is *suppressed* by the fold (§2.8 rule 2) —
/// the append still succeeds and the chain is what the wire carries, so
/// the fold is not asserted against the generated targets here.
fn build_log(
    record: &RecordId,
    (history, steps): &(Vec<(RecordValues, Meta)>, Vec<LifecycleStep>),
) -> RecordLog {
    let mut log = RecordLog::new(record.clone());
    let mut previous = RecordValues::new();
    for (i, (target, meta)) in history.iter().enumerate() {
        let ops = diff(&previous, target);
        let salts = test_salts(i as u8)(ops.len());
        log.append(Draft {
            actor: meta.actor.clone(),
            timestamp: meta.timestamp,
            revision: common::lens(),
            base_version: log.version(),
            origin: meta.origin.clone(),
            note: meta.note.clone(),
            ops: ops.into_iter().map(EntryOp::Cell).collect(),
            salts,
        })
        .expect("diff ops apply by construction");
        previous = target.clone();
    }
    // Lifecycle steps, made legal against the fold (§2.8 table): a
    // request while pending is preceded by `abandon(superseded)`; a
    // terminal transition on a non-pending instance is preceded by a
    // request. A checkpoint expects exactly what is pending after them.
    let meta = &history.last().expect("at least one state").1;
    for (i, step) in steps.iter().enumerate() {
        let state = log.fold().expect("the log folds");
        let key = (step.anchor.clone(), step.scope.clone());
        let pending = state
            .resolutions
            .get(&key)
            .is_some_and(|r| r.status.is_pending());
        let lifecycle = |t: Transition| EntryOp::Resolution {
            anchor: step.anchor.clone(),
            scope: step.scope.clone(),
            transition: t,
        };
        let mut ops = Vec::new();
        match (&step.transition, pending) {
            (Transition::Request { .. }, true) => ops.push(lifecycle(Transition::Abandon {
                reason: AbandonReason::Superseded,
                outcome: Outcome::default(),
            })),
            (Transition::Request { .. }, false) => {}
            (_, true) => {}
            (_, false) => ops.push(lifecycle(Transition::Request {
                resolver: ResolverId::new("insee-sirene"),
                resolver_version: 1,
                mapping_version: 1,
            })),
        }
        ops.push(lifecycle(step.transition.clone()));
        // Same discipline for the scan step (§2.15 mirrors §2.8).
        if let Some((element, transition)) = &step.scan {
            let pending = state
                .scans
                .get(element)
                .is_some_and(|s| s.status.is_pending());
            let scan = |t: ScanTransition| EntryOp::Scan {
                element: element.clone(),
                transition: t,
            };
            match (transition, pending) {
                (ScanTransition::Request { .. }, true) => ops.push(scan(ScanTransition::Abandon {
                    reason: AbandonReason::Superseded,
                    outcome: Outcome::default(),
                })),
                (ScanTransition::Request { .. }, false) | (_, true) => {}
                (_, false) => ops.push(scan(ScanTransition::Request {
                    hash: hash_plain(&CanonicalValue::String("bytes".into())).unwrap(),
                })),
            }
            ops.push(scan(transition.clone()));
        }
        if step.checkpoint {
            // What will be pending once these ops fold: re-fold a
            // scratch copy rather than second-guess the table.
            let mut scratch = log.clone();
            let n = ops.len();
            scratch
                .append(Draft {
                    actor: meta.actor.clone(),
                    timestamp: meta.timestamp,
                    revision: common::lens(),
                    base_version: scratch.version(),
                    origin: Origin::Entered,
                    note: None,
                    ops: ops.clone(),
                    salts: test_salts(200 + i as u8)(n),
                })
                .expect("made legal above");
            let expected = scratch
                .fold()
                .unwrap()
                .pending_resolutions()
                .map(|r| ExpectedResolution {
                    anchor: r.anchor.clone(),
                    scope: r.scope.clone(),
                    resolver: r.resolver.clone(),
                    resolver_version: r.resolver_version,
                    mapping_version: r.mapping_version,
                })
                .collect();
            ops.push(EntryOp::Checkpoint(Checkpoint {
                name: format!("cp{i}"),
                reading_revision: common::lens(),
                expected,
                frozen_columns: ["a", "b"]
                    .into_iter()
                    .map(varve_core::ColumnId::new)
                    .collect(),
                frozen_groups: [GroupId::new("rep")].into_iter().collect(),
            }));
        }
        let n = ops.len();
        log.append(Draft {
            actor: meta.actor.clone(),
            timestamp: meta.timestamp,
            revision: common::lens(),
            base_version: log.version(),
            origin: Origin::Entered,
            note: None,
            ops,
            salts: test_salts(200 + i as u8)(n),
        })
        .expect("lifecycle steps are made legal");
    }
    log
}

fn logs() -> impl Strategy<Value = Vec<(RecordId, RecordLog)>> {
    proptest::collection::btree_map("[a-z0-9]{1,6}", record_history(), 1..=3).prop_map(|records| {
        records
            .into_iter()
            .map(|(id, history)| {
                let record = RecordId::new(id);
                let log = build_log(&record, &history);
                (record, log)
            })
            .collect()
    })
}

fn export(logs: &[(RecordId, RecordLog)], intent: Intent) -> Vec<u8> {
    let refs: Vec<(RecordId, &RecordLog)> = logs.iter().map(|(r, l)| (r.clone(), l)).collect();
    write_history(
        common::manifest(Mode::History, intent, logs.len() as u64),
        vec![common::revision_line()],
        &refs,
    )
    .unwrap()
}

proptest! {
    // Each case builds up to 12 entries and round-trips them through
    // JSON; 64 cases keep the suite fast while still sweeping every op
    // kind, origin shape and scalar kind.
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// M3 for history mode: read ∘ write is identity on every entry
    /// line, the bytes are stable, and adoption on a fresh instance
    /// yields the very same chain (§6: tamper-evidence spans instances).
    #[test]
    fn history_streams_round_trip(logs in logs()) {
        let bytes = export(&logs, Intent::CreateOnly);
        let stream = read_stream(&bytes).unwrap();
        prop_assert_eq!(write_lines(&stream.lines).unwrap(), bytes);

        // The entry lines are the logs' entries, record by record, in
        // seq order.
        let expected: Vec<Line> = logs
            .iter()
            .flat_map(|(record, log)| {
                log.entries().iter().map(move |entry| Line::Entry {
                    record: record.clone(),
                    entry: entry.clone(),
                })
            })
            .collect();
        prop_assert_eq!(&stream.lines[2..], expected.as_slice());

        let mut store = BTreeMap::new();
        let outcome = adopt_history(&stream, &mut store).unwrap();
        let ids: Vec<RecordId> = logs.iter().map(|(r, _)| r.clone()).collect();
        prop_assert_eq!(&outcome.created, &ids);
        prop_assert!(outcome.updated.is_empty());
        for (record, log) in &logs {
            let adopted = &store[record];
            prop_assert_eq!(adopted.entries(), log.entries());
            prop_assert_eq!(adopted.verify_chain(), Ok(()));
            prop_assert!(adopted.fold().is_ok());
        }
    }

    /// Re-importing a history over the store it produced, under
    /// `Upsert`, is accepted: every chain extends itself (§6 — same
    /// prefix) with nothing new, and the outcome names them all as
    /// unchanged — `updated` implies a change.
    #[test]
    fn a_history_extends_itself_under_upsert(logs in logs()) {
        let mut store = BTreeMap::new();
        adopt_history(&read_stream(&export(&logs, Intent::CreateOnly)).unwrap(), &mut store).unwrap();
        let again = read_stream(&export(&logs, Intent::Upsert)).unwrap();
        let outcome = adopt_history(&again, &mut store).unwrap();
        let ids: Vec<RecordId> = logs.iter().map(|(r, _)| r.clone()).collect();
        prop_assert!(outcome.created.is_empty());
        prop_assert!(outcome.updated.is_empty());
        prop_assert_eq!(&outcome.unchanged, &ids);
        for (record, log) in &logs {
            prop_assert_eq!(store[record].entries(), log.entries());
        }
        // And under `UpdateOnly` too; under `CreateOnly` it is refused.
        let update_only = read_stream(&export(&logs, Intent::UpdateOnly)).unwrap();
        prop_assert!(adopt_history(&update_only, &mut store).is_ok());
        let create_only = read_stream(&export(&logs, Intent::CreateOnly)).unwrap();
        prop_assert!(matches!(
            adopt_history(&create_only, &mut store),
            Err(varve_wire::ImportError::AlreadyExists(_))
        ));
    }
}

/// The generator does reach every op kind — cell and lifecycle —
/// otherwise the law above would be weaker than it claims.
#[test]
fn generated_histories_cover_every_op_kind() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    use varve_value::Op;
    let mut runner = TestRunner::deterministic();
    let mut seen = [false; 17];
    for _ in 0..200 {
        let history = record_history().new_tree(&mut runner).unwrap().current();
        let log = build_log(&RecordId::new("r"), &history);
        for entry in log.entries() {
            for op in &entry.content.ops {
                seen[match op {
                    EntryOp::Cell(Op::Set { .. }) => 0,
                    EntryOp::Cell(Op::Unset { .. }) => 1,
                    EntryOp::Cell(Op::AddItem { .. }) => 2,
                    EntryOp::Cell(Op::RemoveItem { .. }) => 3,
                    EntryOp::Cell(Op::Reorder { .. }) => 4,
                    EntryOp::Resolution { transition, .. } => match transition {
                        Transition::Request { .. } => 5,
                        Transition::Land { .. } => 6,
                        Transition::NotFound { .. } => 7,
                        Transition::Ambiguous { .. } => 8,
                        Transition::Failed { .. } => 9,
                        Transition::Abandon { .. } => 10,
                    },
                    EntryOp::Checkpoint(_) => 11,
                    EntryOp::Scan { transition, .. } => match transition {
                        ScanTransition::Request { .. } => 12,
                        ScanTransition::Clean { .. } => 13,
                        ScanTransition::Infected { .. } => 14,
                        ScanTransition::Failed { .. } => 15,
                        ScanTransition::Abandon { .. } => 16,
                    },
                }] = true;
            }
        }
        if seen.iter().all(|s| *s) {
            return;
        }
    }
    panic!("op kinds not all generated: {seen:?}");
}
