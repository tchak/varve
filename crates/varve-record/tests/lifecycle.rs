//! Exhaustive transition tables for resolution instances (§2.8) and
//! attachment scans (§2.15): every (from, to) pair, so the legal set is
//! pinned as a whole rather than sampled.

use varve_core::canonical::ContentHash;
use varve_core::{RecordId, ResolverId, RowPath, GroupId, ItemId, PathSeg};
use varve_record::{
    Resolution, ResolutionStatus, Scan, ScanStatus, ScanTransitionError, TransitionError,
    genesis_hash, pending_resolutions, pending_scans, pending_set,
};

const RESOLUTION_STATUSES: [ResolutionStatus; 6] = [
    ResolutionStatus::Pending,
    ResolutionStatus::Resolved,
    ResolutionStatus::NotFound,
    ResolutionStatus::Ambiguous,
    ResolutionStatus::Failed,
    ResolutionStatus::Abandoned,
];

const SCAN_STATUSES: [ScanStatus; 4] =
    [ScanStatus::Pending, ScanStatus::Clean, ScanStatus::Infected, ScanStatus::Failed];

fn resolution(status: ResolutionStatus, scope: RowPath) -> Resolution {
    Resolution {
        anchor: GroupId::new("entreprise"),
        resolver: ResolverId::new("insee-sirene"),
        resolver_version: 1,
        mapping_version: 1,
        scope,
        status,
        attempts: 0,
        last_error: None,
        deadline: None,
        snapshot: None,
    }
}

fn payload() -> ContentHash {
    genesis_hash(&RecordId::new("r1"))
}

fn scan(status: ScanStatus) -> Scan {
    Scan { element: "f1".into(), hash: payload(), status, attempts: 0 }
}

#[test]
fn resolution_transition_table_is_exactly_the_documented_one() {
    use ResolutionStatus::*;
    for from in RESOLUTION_STATUSES {
        for to in RESOLUTION_STATUSES {
            let mut r = resolution(from, RowPath::root());
            let result = r.transition(to);
            let expected = if to == Resolved {
                // Never through `transition`: a resolution resolves by
                // landing its payload (§2.7).
                Err(TransitionError::ResolvedWithoutSnapshot)
            } else if matches!((from, to), (Pending, NotFound | Ambiguous | Failed | Abandoned) | (Failed, Pending | Abandoned)) {
                Ok(())
            } else {
                Err(TransitionError::Illegal { from, to })
            };
            assert_eq!(result, expected, "{from:?} → {to:?}");
            // State moves only on success; attempts count only retries.
            assert_eq!(r.status, if result.is_ok() { to } else { from }, "{from:?} → {to:?}");
            assert_eq!(r.attempts, u32::from((from, to) == (Failed, Pending)), "{from:?} → {to:?}");
            assert_eq!(r.snapshot, None);
        }
    }
}

#[test]
fn landing_is_the_only_road_to_resolved_and_starts_from_pending_only() {
    use ResolutionStatus::*;
    for from in RESOLUTION_STATUSES {
        let mut r = resolution(from, RowPath::root());
        let result = r.land(payload());
        if from == Pending {
            assert_eq!(result, Ok(()));
            assert_eq!(r.status, Resolved);
            assert_eq!(r.snapshot, Some(payload()));
        } else {
            assert_eq!(result, Err(TransitionError::Illegal { from, to: Resolved }), "{from:?}");
            assert_eq!(r.status, from);
            assert_eq!(r.snapshot, None, "a refused landing records nothing");
        }
    }
    // Retries are counted across the whole life: fail, retry, fail,
    // retry, land.
    let mut r = resolution(Pending, RowPath::root());
    for _ in 0..2 {
        r.transition(Failed).unwrap();
        r.transition(Pending).unwrap();
    }
    r.land(payload()).unwrap();
    assert_eq!(r.attempts, 2);
}

#[test]
fn scan_transition_table_is_exactly_the_documented_one() {
    use ScanStatus::*;
    for from in SCAN_STATUSES {
        for to in SCAN_STATUSES {
            let mut s = scan(from);
            let result = s.transition(to);
            let legal = matches!((from, to), (Pending, Clean | Infected | Failed) | (Failed, Pending));
            let expected = if legal { Ok(()) } else { Err(ScanTransitionError { from, to }) };
            assert_eq!(result, expected, "{from:?} → {to:?}");
            assert_eq!(s.status, if legal { to } else { from }, "{from:?} → {to:?}");
            assert_eq!(s.attempts, u32::from((from, to) == (Failed, Pending)), "{from:?} → {to:?}");
        }
    }
}

#[test]
fn pending_enumerations_keep_only_pending_and_distinguish_scopes() {
    let item = |i: &str| RowPath::root().child(PathSeg { group: GroupId::new("g1"), item: ItemId::new(i) });
    let all: Vec<Resolution> = RESOLUTION_STATUSES
        .into_iter()
        .map(|status| resolution(status, RowPath::root()))
        .chain([resolution(ResolutionStatus::Pending, item("i1")), resolution(ResolutionStatus::Pending, item("i2"))])
        .collect();
    let pending = pending_resolutions(&all);
    assert_eq!(pending.len(), 3);
    assert!(pending.iter().all(|r| r.status == ResolutionStatus::Pending));
    // §2.8 rule 3: per group instance — one pair per (scope, resolver),
    // and two instances of the same resolver on two items stay apart.
    let set = pending_set(&all);
    assert_eq!(
        set,
        [RowPath::root(), item("i1"), item("i2")]
            .into_iter()
            .map(|scope| (scope, GroupId::new("entreprise")))
            .collect()
    );
    // Two pending instances at the same (scope, resolver) collapse to
    // one pair — the set is what the logic language reads.
    let twice = [resolution(ResolutionStatus::Pending, item("i1")), resolution(ResolutionStatus::Pending, item("i1"))];
    assert_eq!(pending_set(&twice).len(), 1);

    let scans: Vec<Scan> = SCAN_STATUSES.into_iter().map(scan).collect();
    let pending = pending_scans(&scans);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].status, ScanStatus::Pending);
}
