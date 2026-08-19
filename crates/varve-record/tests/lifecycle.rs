//! Exhaustive transition tables for resolution instances (§2.8) and
//! attachment scans (§2.15): every (from, to) pair, so the legal set is
//! pinned as a whole rather than sampled.
//!
//! Resolutions are folds of lifecycle ops in the log (settled
//! 2026-08-19), so the table is driven through `append`: a log holding
//! an instance in each state, and every transition tried against it.

use varve_core::canonical::{ContentHash, Salt};
use varve_core::primitives::Instant;
use varve_core::{GroupId, ItemId, PathSeg, RecordId, ResolverId, RevisionId, RowPath};
use varve_record::{
    AbandonReason, Actor, ActorKind, AppendError, Draft, EntryOp, EntrySalts, LifecycleError,
    Origin, Outcome, RecordLog, ResolutionStatus, Scan, ScanStatus, ScanTransitionError,
    Transition, genesis_hash, pending_scans,
};

fn payload() -> ContentHash {
    genesis_hash(&RecordId::new("r1"))
}

/// One transition of each kind — the columns of the table.
fn transitions() -> Vec<Transition> {
    vec![
        Transition::Request {
            resolver: ResolverId::new("insee-sirene"),
            resolver_version: 1,
            mapping_version: 1,
        },
        Transition::Land {
            snapshot: payload(),
            outcome: Outcome::default(),
        },
        Transition::NotFound {
            outcome: Outcome::default(),
        },
        Transition::Ambiguous {
            outcome: Outcome::default(),
        },
        Transition::Failed {
            outcome: Outcome::default(),
        },
        Transition::Abandon {
            reason: AbandonReason::Deadline,
            outcome: Outcome {
                attempts: 212,
                last_error: Some("503".into()),
            },
        },
    ]
}

fn actor() -> Actor {
    Actor {
        id: "a1".into(),
        kind: ActorKind::System,
    }
}

fn draft(base: u64, ops: Vec<EntryOp>) -> Draft {
    let n = ops.len();
    Draft {
        actor: actor(),
        timestamp: Instant::parse("2026-08-19T10:00:00Z").unwrap(),
        revision: RevisionId::new("rev-1"),
        base_version: base,
        origin: Origin::Entered,
        note: None,
        ops,
        salts: EntrySalts {
            meta: Salt([9; 32]),
            ops: (0..n).map(|i| Salt([i as u8 + 1; 32])).collect(),
        },
    }
}

fn op(scope: RowPath, transition: Transition) -> EntryOp {
    EntryOp::Resolution {
        anchor: GroupId::new("entreprise"),
        scope,
        transition,
    }
}

/// A log whose root instance is in `state`: `None` = never requested,
/// otherwise requested and then taken there.
fn log_in(state: Option<ResolutionStatus>) -> RecordLog {
    let mut log = RecordLog::new(RecordId::new("r1"));
    let Some(state) = state else { return log };
    log.append(draft(
        0,
        vec![op(RowPath::root(), transitions()[0].clone())],
    ))
    .unwrap();
    if state != ResolutionStatus::Pending {
        let to = transitions()
            .into_iter()
            .find(|t| t.status() == state)
            .unwrap();
        log.append(draft(1, vec![op(RowPath::root(), to)])).unwrap();
    }
    let folded = log.fold().unwrap();
    assert_eq!(
        folded.resolutions[&(GroupId::new("entreprise"), RowPath::root())].status,
        state
    );
    log
}

const STATES: [Option<ResolutionStatus>; 7] = [
    None,
    Some(ResolutionStatus::Pending),
    Some(ResolutionStatus::Resolved),
    Some(ResolutionStatus::NotFound),
    Some(ResolutionStatus::Ambiguous),
    Some(ResolutionStatus::Failed),
    Some(ResolutionStatus::Abandoned(AbandonReason::Deadline)),
];

#[test]
fn resolution_transition_table_is_exactly_the_documented_one() {
    // pending → resolved | not_found | ambiguous | failed | abandoned;
    // (absent | any terminal) → pending by `request`; nothing else.
    for from in STATES {
        for to in transitions() {
            let mut log = log_in(from);
            let base = log.version();
            let result = log.append(draft(base, vec![op(RowPath::root(), to.clone())]));
            let is_request = matches!(to, Transition::Request { .. });
            let pending = from == Some(ResolutionStatus::Pending);
            match (is_request, pending) {
                (true, false) | (false, true) => {
                    assert!(result.is_ok(), "{from:?} --{to:?}--> should be legal");
                    let r = log.fold().unwrap().resolutions
                        [&(GroupId::new("entreprise"), RowPath::root())]
                        .clone();
                    assert_eq!(r.status, to.status());
                    if is_request {
                        assert_eq!((r.requested_at, r.closed_at), (base, None));
                        assert_eq!((r.snapshot, r.outcome), (None, None));
                    } else {
                        assert_eq!(r.closed_at, Some(base));
                        assert!(r.outcome.is_some(), "terminal ops carry their summary");
                        assert_eq!(
                            r.snapshot.is_some(),
                            matches!(to, Transition::Land { .. }),
                            "only a landing carries a snapshot"
                        );
                    }
                }
                (true, true) => assert!(
                    matches!(
                        result,
                        Err(AppendError::IllegalTransition(
                            LifecycleError::AlreadyPending { .. }
                        ))
                    ),
                    "request while pending must be refused"
                ),
                (false, false) => {
                    assert!(
                        matches!(
                            result,
                            Err(AppendError::IllegalTransition(LifecycleError::NotPending {
                                status, ..
                            })) if status == from
                        ),
                        "{from:?} --{to:?}--> should be refused"
                    );
                    assert_eq!(log.version(), base, "a refused append leaves the log alone");
                }
            }
        }
    }
}

#[test]
fn instances_are_per_anchor_group_instance() {
    // §2.8 rule 3 / Q17: two items each have their own instance; the
    // root's is a third. Pending enumerations keep only pending ones
    // and the logic set is per (scope, anchor group).
    let item = |i: &str| {
        RowPath::root().child(PathSeg {
            group: GroupId::new("g1"),
            item: ItemId::new(i),
        })
    };
    let request = transitions()[0].clone();
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(
        0,
        vec![
            op(RowPath::root(), request.clone()),
            op(item("i1"), request.clone()),
            op(item("i2"), request),
            // A different anchor at root: a fourth instance.
            EntryOp::Resolution {
                anchor: GroupId::new("adresse"),
                scope: RowPath::root(),
                transition: transitions()[0].clone(),
            },
        ],
    ))
    .unwrap();
    // Landing i1 touches nothing else.
    log.append(draft(1, vec![op(item("i1"), transitions()[1].clone())]))
        .unwrap();
    let state = log.fold().unwrap();
    assert_eq!(state.resolutions.len(), 4);
    assert_eq!(state.pending_resolutions().count(), 3);
    assert_eq!(
        state.pending_set(),
        [
            (RowPath::root(), GroupId::new("entreprise")),
            (RowPath::root(), GroupId::new("adresse")),
            (item("i2"), GroupId::new("entreprise")),
        ]
        .into_iter()
        .collect()
    );
}

// ---------------------------------------------------------------- scans

const SCAN_STATUSES: [ScanStatus; 4] = [
    ScanStatus::Pending,
    ScanStatus::Clean,
    ScanStatus::Infected,
    ScanStatus::Failed,
];

fn scan(status: ScanStatus) -> Scan {
    Scan {
        element: "f1".into(),
        hash: payload(),
        status,
        attempts: 0,
    }
}

#[test]
fn scan_transition_table_is_exactly_the_documented_one() {
    use ScanStatus::*;
    for from in SCAN_STATUSES {
        for to in SCAN_STATUSES {
            let mut s = scan(from);
            let result = s.transition(to);
            let legal = matches!(
                (from, to),
                (Pending, Clean | Infected | Failed) | (Failed, Pending)
            );
            let expected = if legal {
                Ok(())
            } else {
                Err(ScanTransitionError { from, to })
            };
            assert_eq!(result, expected, "{from:?} → {to:?}");
            assert_eq!(s.status, if legal { to } else { from }, "{from:?} → {to:?}");
            assert_eq!(
                s.attempts,
                u32::from((from, to) == (Failed, Pending)),
                "{from:?} → {to:?}"
            );
        }
    }
    let scans: Vec<Scan> = SCAN_STATUSES.into_iter().map(scan).collect();
    let pending = pending_scans(&scans);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, ScanStatus::Pending);
}
