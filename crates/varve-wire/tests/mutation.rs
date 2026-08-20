//! Mutation fuzz of **valid** streams: byte-level and line-level damage
//! to well-formed history and snapshot streams must never panic the
//! reader or the importers, and whatever survives to adoption must be a
//! verified chain (§5 reader is total; §6 adoption verifies). Then
//! targeted single-field edits of an `entry` line: none may be accepted
//! with changed content — refused at read, refused at adoption, or a
//! no-op.

mod common;

use std::collections::BTreeMap;

use proptest::prelude::*;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, GroupId, ItemId, PathSeg, RecordId, RowPath};
use varve_record::{Actor, ActorKind, Draft, Entry, EntryOp, Origin, RecordLog};
use varve_value::{CellState, CellValue, Op, Scalar};
use varve_wire::{
    Intent, Mode, SnapshotImportRequest, SnapshotRecord, adopt_history, import_snapshot,
    read_stream, snapshot_records, test_salts, write_history, write_snapshot,
};

fn set_at(column: &str, path: RowPath, value: &str) -> Op {
    Op::Set {
        column: ColumnId::new(column),
        path,
        state: CellState::Value(CellValue::One(Scalar::Text(value.into()))),
    }
}

fn item(id: &str) -> RowPath {
    RowPath::root().child(PathSeg {
        group: GroupId::new("g1"),
        item: ItemId::new(id),
    })
}

fn draft(minute: u8, base: u64, ops: Vec<Op>) -> Draft {
    let salts = test_salts(minute)(ops.len());
    Draft {
        actor: Actor {
            id: "a1".into(),
            kind: ActorKind::Human,
        },
        timestamp: Instant::parse(&format!("2026-08-17T10:{minute:02}:00Z")).unwrap(),
        revision: common::lens(),
        base_version: base,
        origin: Origin::Entered,
        note: Some("n".into()),
        ops: ops.into_iter().map(EntryOp::Cell).collect(),
        salts,
    }
}

/// Two records; `r1` has three entries touching every op kind, so a
/// non-tail entry exists to mutate.
fn logs() -> Vec<(RecordId, RecordLog)> {
    let mut r1 = RecordLog::new(RecordId::new("r1"));
    r1.append(draft(
        0,
        0,
        vec![
            set_at("name", RowPath::root(), "Dupont"),
            Op::AddItem {
                group: GroupId::new("g1"),
                parent: RowPath::root(),
                item: ItemId::new("i1"),
                at: 0,
            },
            set_at("col", item("i1"), "x"),
        ],
    ))
    .unwrap();
    r1.append(draft(
        1,
        1,
        vec![
            Op::AddItem {
                group: GroupId::new("g1"),
                parent: RowPath::root(),
                item: ItemId::new("i2"),
                at: 1,
            },
            Op::Reorder {
                group: GroupId::new("g1"),
                parent: RowPath::root(),
                order: vec![ItemId::new("i2"), ItemId::new("i1")],
            },
            Op::Unset {
                column: ColumnId::new("col"),
                path: item("i1"),
            },
            set_at("name", RowPath::root(), "Durand"),
        ],
    ))
    .unwrap();
    r1.append(draft(
        2,
        2,
        vec![Op::RemoveItem {
            group: GroupId::new("g1"),
            parent: RowPath::root(),
            item: ItemId::new("i1"),
        }],
    ))
    .unwrap();
    let mut r2 = RecordLog::new(RecordId::new("r2"));
    r2.append(draft(3, 0, vec![set_at("name", RowPath::root(), "Martin")]))
        .unwrap();
    vec![(RecordId::new("r1"), r1), (RecordId::new("r2"), r2)]
}

fn history_bytes() -> Vec<u8> {
    let logs = logs();
    let refs: Vec<(RecordId, &RecordLog)> = logs.iter().map(|(r, l)| (r.clone(), l)).collect();
    write_history(
        common::manifest(Mode::History, Intent::CreateOnly, 2),
        vec![common::revision_line()],
        &refs,
    )
    .unwrap()
}

fn snapshot_bytes() -> Vec<u8> {
    let records: Vec<SnapshotRecord> = logs()
        .iter()
        .map(|(record, log)| SnapshotRecord {
            record: record.clone(),
            lens: common::lens(),
            values: log.fold().unwrap().values,
        })
        .collect();
    write_snapshot(
        common::manifest(Mode::Snapshot, Intent::Upsert, 2),
        vec![common::revision_line()],
        &records,
    )
    .unwrap()
}

#[derive(Debug, Clone)]
enum Mutation {
    Flip { pos: u32, byte: u8 },
    Insert { pos: u32, byte: u8 },
    Delete { pos: u32 },
    DupLine { line: u32 },
    DropLine { line: u32 },
    SwapLines { a: u32, b: u32 },
    Truncate { pos: u32 },
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        (any::<u32>(), any::<u8>()).prop_map(|(pos, byte)| Mutation::Flip { pos, byte }),
        (any::<u32>(), any::<u8>()).prop_map(|(pos, byte)| Mutation::Insert { pos, byte }),
        any::<u32>().prop_map(|pos| Mutation::Delete { pos }),
        any::<u32>().prop_map(|line| Mutation::DupLine { line }),
        any::<u32>().prop_map(|line| Mutation::DropLine { line }),
        (any::<u32>(), any::<u32>()).prop_map(|(a, b)| Mutation::SwapLines { a, b }),
        any::<u32>().prop_map(|pos| Mutation::Truncate { pos }),
    ]
}

fn split_lines(bytes: &[u8]) -> Vec<Vec<u8>> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

fn join_lines(lines: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines {
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    out
}

fn mutate(bytes: &mut Vec<u8>, m: &Mutation) {
    let at = |pos: u32, len: usize| if len == 0 { 0 } else { pos as usize % len };
    match *m {
        Mutation::Flip { pos, byte } => {
            if !bytes.is_empty() {
                let i = at(pos, bytes.len());
                bytes[i] ^= byte;
            }
        }
        Mutation::Insert { pos, byte } => {
            let i = at(pos, bytes.len() + 1);
            bytes.insert(i, byte);
        }
        Mutation::Delete { pos } => {
            if !bytes.is_empty() {
                let i = at(pos, bytes.len());
                bytes.remove(i);
            }
        }
        Mutation::Truncate { pos } => {
            let i = at(pos, bytes.len() + 1);
            bytes.truncate(i);
        }
        Mutation::DupLine { line } => {
            let mut lines = split_lines(bytes);
            if !lines.is_empty() {
                let i = at(line, lines.len());
                let dup = lines[i].clone();
                lines.insert(i, dup);
                *bytes = join_lines(&lines);
            }
        }
        Mutation::DropLine { line } => {
            let mut lines = split_lines(bytes);
            if !lines.is_empty() {
                let i = at(line, lines.len());
                lines.remove(i);
                *bytes = join_lines(&lines);
            }
        }
        Mutation::SwapLines { a, b } => {
            let mut lines = split_lines(bytes);
            if !lines.is_empty() {
                let (i, j) = (at(a, lines.len()), at(b, lines.len()));
                lines.swap(i, j);
                *bytes = join_lines(&lines);
            }
        }
    }
}

proptest! {
    /// Damaged history streams: the reader is total; whatever it accepts
    /// either fails adoption or adopts as chains that verify and fold —
    /// adoption never stores damage (§6).
    #[test]
    fn damaged_history_streams_never_panic_and_never_adopt_damage(
        mutations in proptest::collection::vec(mutation(), 1..=3),
    ) {
        let mut bytes = history_bytes();
        for m in &mutations {
            mutate(&mut bytes, m);
        }
        let Ok(stream) = read_stream(&bytes) else { return Ok(()) };
        let mut store = BTreeMap::new();
        if adopt_history(&stream, &mut store).is_ok() {
            for log in store.values() {
                prop_assert_eq!(log.verify_chain(), Ok(()));
                prop_assert!(log.fold().is_ok());
            }
        }
    }

    /// Damaged snapshot streams: reader, reassembly and import are all
    /// total.
    #[test]
    fn damaged_snapshot_streams_never_panic(
        mutations in proptest::collection::vec(mutation(), 1..=3),
    ) {
        let mut bytes = snapshot_bytes();
        for m in &mutations {
            mutate(&mut bytes, m);
        }
        let Ok(stream) = read_stream(&bytes) else { return Ok(()) };
        let _ = snapshot_records(&stream);
        let mut store = BTreeMap::new();
        let salts = test_salts(9);
        let request = SnapshotImportRequest {
            actor: Actor { id: "importer".into(), kind: ActorKind::System },
            timestamp: Instant::parse("2026-08-17T12:00:00Z").unwrap(),
                        note: None,
            salts_for: &salts,
        };
        let _ = import_snapshot(&stream, &mut store, &request);
    }
}

// ---------------------------------------------------------------------
// Targeted single-field edits of an entry line.

/// The stream's lines as text, and the index of the first `entry` line
/// of `r1` — a **non-tail** entry: every envelope field of a non-tail
/// entry is anchored by its successor's `prev` (§2.9; the tail's
/// envelope is anchored only by a checkpoint or snapshot).
fn history_lines() -> (Vec<String>, usize) {
    let bytes = history_bytes();
    let lines: Vec<String> = String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let first_entry = lines
        .iter()
        .position(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["k"] == "entry" && v["record"] == "r1"
        })
        .unwrap();
    (lines, first_entry)
}

fn original_entries() -> BTreeMap<RecordId, Vec<Entry>> {
    logs()
        .into_iter()
        .map(|(r, l)| (r, l.entries().to_vec()))
        .collect()
}

/// A mutation is harmless iff the stream is refused at read, refused at
/// adoption, or adopts exactly the original entries (the edit was a
/// no-op). Returns what happened, for the assertion message.
fn outcome_of(lines: &[String]) -> &'static str {
    let bytes = lines.join("\n").into_bytes();
    let Ok(stream) = read_stream(&bytes) else {
        return "refused at read";
    };
    let mut store = BTreeMap::new();
    if adopt_history(&stream, &mut store).is_err() {
        return "refused at adoption";
    }
    let adopted: BTreeMap<RecordId, Vec<Entry>> = store
        .into_iter()
        .map(|(r, l)| (r, l.entries().to_vec()))
        .collect();
    if adopted == original_entries() {
        "no-op"
    } else {
        "ACCEPTED WITH CHANGED CONTENT"
    }
}

/// Bump the last hex digit of a hash/salt string, keeping it lowercase
/// hex.
fn nudge_hex(s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    let last = chars.last_mut().unwrap();
    *last = if *last == '0' { '1' } else { '0' };
    chars.into_iter().collect()
}

type Edit = Box<dyn Fn(&mut serde_json::Value)>;

#[test]
fn single_field_edits_of_a_non_tail_entry_are_never_accepted() {
    use serde_json::{Value, json};
    let (lines, at) = history_lines();
    let edits: Vec<(&str, Edit)> = vec![
        (
            "seq",
            Box::new(|v| v["seq"] = json!(v["seq"].as_u64().unwrap() + 1)),
        ),
        (
            "prev",
            Box::new(|v| v["prev"] = json!(nudge_hex(v["prev"].as_str().unwrap()))),
        ),
        (
            "content_hash",
            Box::new(|v| v["content_hash"] = json!(nudge_hex(v["content_hash"].as_str().unwrap()))),
        ),
        (
            "op value",
            Box::new(|v| v["ops"][0]["state"]["text"] = json!("MALLORY")),
        ),
        (
            "op removed",
            Box::new(|v| {
                v["ops"].as_array_mut().unwrap().pop();
            }),
        ),
        (
            "op salt",
            Box::new(|v| v["op_salts"][0] = json!(nudge_hex(v["op_salts"][0].as_str().unwrap()))),
        ),
        (
            "meta salt",
            Box::new(|v| v["meta_salt"] = json!(nudge_hex(v["meta_salt"].as_str().unwrap()))),
        ),
        ("actor", Box::new(|v| v["actor"] = json!("mallory"))),
        (
            "actor_kind",
            Box::new(|v| v["actor_kind"] = json!("system")),
        ),
        (
            "timestamp",
            Box::new(|v| v["timestamp"] = json!("2026-08-17T10:59:00Z")),
        ),
        (
            "revision",
            Box::new(|v| {
                v["revision"] =
                    json!("sha256:0000000000000000000000000000000000000000000000000000000000000000")
            }),
        ),
        (
            "base_version",
            Box::new(|v| v["base_version"] = json!(v["base_version"].as_u64().unwrap() + 1)),
        ),
        (
            "origin",
            Box::new(|v| v["origin"] = json!({"overridden": null})),
        ),
        ("note edited", Box::new(|v| v["note"] = json!("edited"))),
        (
            "note dropped",
            Box::new(|v| {
                v.as_object_mut().unwrap().remove("note");
            }),
        ),
        ("note nulled", Box::new(|v| v["note"] = Value::Null)),
        ("unknown key", Box::new(|v| v["x"] = json!(1))),
        ("record", Box::new(|v| v["record"] = json!("r2"))),
    ];
    for (name, edit) in edits {
        let mut lines = lines.clone();
        let mut value: Value = serde_json::from_str(&lines[at]).unwrap();
        edit(&mut value);
        lines[at] = serde_json::to_string(&value).unwrap();
        let outcome = outcome_of(&lines);
        assert_ne!(outcome, "ACCEPTED WITH CHANGED CONTENT", "edit `{name}`");
        assert_ne!(
            outcome, "no-op",
            "edit `{name}` should have changed something"
        );
    }
}

/// Re-serializing an untouched entry line through serde_json (which
/// reorders keys and re-escapes) is a genuine no-op — the reader is
/// key-order-agnostic; only the writer pins JCS bytes.
#[test]
fn a_reserialized_entry_line_is_a_no_op() {
    let (mut lines, at) = history_lines();
    let value: serde_json::Value = serde_json::from_str(&lines[at]).unwrap();
    lines[at] = serde_json::to_string_pretty(&value)
        .unwrap()
        .replace('\n', " ");
    assert_eq!(outcome_of(&lines), "no-op");
}

/// The tail entry: content and salt edits are still caught (the content
/// commitment is self-contained), and so are seq/prev; its other
/// envelope fields are unanchored by the chain alone (§2.9 — a
/// checkpoint or snapshot pins the head), which this test documents.
#[test]
fn tail_entry_edits() {
    use serde_json::{Value, json};
    let (lines, _) = history_lines();
    let tail = lines
        .iter()
        .rposition(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            v["k"] == "entry" && v["record"] == "r1"
        })
        .unwrap();
    let apply = |edit: &dyn Fn(&mut Value)| {
        let mut lines = lines.clone();
        let mut value: Value = serde_json::from_str(&lines[tail]).unwrap();
        edit(&mut value);
        lines[tail] = serde_json::to_string(&value).unwrap();
        outcome_of(&lines)
    };
    assert_eq!(
        apply(&|v| v["ops"][0]["item"] = json!("i9")),
        "refused at adoption"
    );
    assert_eq!(
        apply(&|v| v["op_salts"][0] = json!(nudge_hex(v["op_salts"][0].as_str().unwrap()))),
        "refused at adoption"
    );
    assert_eq!(
        apply(&|v| v["note"] = json!("edited")),
        "refused at adoption"
    );
    assert_eq!(apply(&|v| v["seq"] = json!(7)), "refused at adoption");
    assert_eq!(
        apply(&|v| v["prev"] = json!(nudge_hex(v["prev"].as_str().unwrap()))),
        "refused at adoption"
    );
    // Unanchored by the chain: the head's actor and timestamp.
    assert_eq!(
        apply(&|v| v["actor"] = json!("mallory")),
        "ACCEPTED WITH CHANGED CONTENT"
    );
    assert_eq!(
        apply(&|v| v["timestamp"] = json!("2026-08-17T10:59:00Z")),
        "ACCEPTED WITH CHANGED CONTENT"
    );
}
