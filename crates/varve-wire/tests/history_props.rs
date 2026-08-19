//! The history-mode wire law (§5, M3) over the **full** op / origin /
//! scalar space: logs built from generated record states — every op
//! kind reached through `diff` — under every actor kind, every origin
//! shape and notes, written as `entry` lines, read back identical, byte
//! stable, and adopted as the same chain (§6).

mod common;

use std::collections::BTreeMap;

use proptest::prelude::*;
use varve_core::canonical::{CanonicalValue, hash_plain};
use varve_core::primitives::Instant;
use varve_core::{RecordId, ResolverId};
use varve_record::{Actor, ActorKind, Derivation, Draft, Origin, RecordLog};
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

/// One record's history: successive target states, each with the
/// metadata of the entry that reaches it.
fn record_history() -> impl Strategy<Value = Vec<(RecordValues, Meta)>> {
    proptest::collection::vec((common::shared_universe_values(), meta()), 1..=4)
}

/// Build the log: `diff(previous, next)` is the entry's op list, so
/// every entry applies by construction. A resolver's derived write onto
/// a human-authored cell is *suppressed* by the fold (§2.8 rule 2) —
/// the append still succeeds and the chain is what the wire carries, so
/// the fold is not asserted against the generated targets here.
fn build_log(record: &RecordId, history: &[(RecordValues, Meta)]) -> RecordLog {
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
            ops,
            salts,
        })
        .expect("diff ops apply by construction");
        previous = target.clone();
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
    /// prefix), and the outcome names them all as updated.
    #[test]
    fn a_history_extends_itself_under_upsert(logs in logs()) {
        let mut store = BTreeMap::new();
        adopt_history(&read_stream(&export(&logs, Intent::CreateOnly)).unwrap(), &mut store).unwrap();
        let again = read_stream(&export(&logs, Intent::Upsert)).unwrap();
        let outcome = adopt_history(&again, &mut store).unwrap();
        let ids: Vec<RecordId> = logs.iter().map(|(r, _)| r.clone()).collect();
        prop_assert!(outcome.created.is_empty());
        prop_assert_eq!(&outcome.updated, &ids);
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

/// The generator does reach every op kind — otherwise the law above
/// would be weaker than it claims.
#[test]
fn generated_histories_cover_every_op_kind() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    use varve_value::Op;
    let mut runner = TestRunner::deterministic();
    let mut seen = [false; 5];
    for _ in 0..200 {
        let history = record_history().new_tree(&mut runner).unwrap().current();
        let log = build_log(&RecordId::new("r"), &history);
        for entry in log.entries() {
            for op in &entry.content.ops {
                seen[match op {
                    Op::Set { .. } => 0,
                    Op::Unset { .. } => 1,
                    Op::AddItem { .. } => 2,
                    Op::RemoveItem { .. } => 3,
                    Op::Reorder { .. } => 4,
                }] = true;
            }
        }
        if seen.iter().all(|s| *s) {
            return;
        }
    }
    panic!("op kinds not all generated: {seen:?}");
}
