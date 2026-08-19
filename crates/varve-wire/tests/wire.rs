use std::collections::BTreeMap;

use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, RecordId, RevisionId, RowPath};
use varve_record::{Actor, ActorKind, Draft, EntryOp, EntrySalts, Origin, RecordLog};
use varve_schema::{
    Arity, Column, Element, NomenclatureRef, OptionRow, ScalarType, Schema, revision_id,
};
use varve_value::{CellAddr, CellState, CellValue, ItemsAddr, Op, RecordValues, Scalar};
use varve_wire::{
    ImportError, Intent, ItemLine, Line, Manifest, Mode, ReadError, RecordLine,
    SnapshotImportRequest, SnapshotRecord, WriteError, adopt_history, import_snapshot, read_stream,
    snapshot_records, test_salts, write_history, write_lines, write_snapshot,
};

fn schema() -> Schema {
    Schema {
        root: vec![
            Element::Column(Column {
                id: ColumnId::new("name"),
                label: "Nom".into(),
                ty: ScalarType::Text,
                arity: Arity::One,
            }),
            Element::Column(Column {
                id: ColumnId::new("statut"),
                label: "Statut".into(),
                ty: ScalarType::Enum(NomenclatureRef::Inline(vec![OptionRow {
                    id: OptionId::new("o1"),
                    label: "En cours".into(),
                    fields: vec![],
                }])),
                arity: Arity::One,
            }),
        ],
        resolvers: vec![],
    }
}

fn set(column: &str, value: &str) -> Op {
    Op::Set {
        column: ColumnId::new(column),
        path: RowPath::root(),
        state: CellState::Value(CellValue::One(Scalar::Text(value.into()))),
    }
}

fn draft(minute: u8, base: u64, ops: Vec<Op>) -> Draft {
    let n = ops.len();
    Draft {
        actor: Actor {
            id: "a1".into(),
            kind: ActorKind::Human,
        },
        timestamp: Instant::parse(&format!("2026-08-17T10:{minute:02}:00Z")).unwrap(),
        revision: revision_id(&schema()),
        base_version: base,
        origin: Origin::Entered,
        note: None,
        ops: ops.into_iter().map(EntryOp::Cell).collect(),
        salts: EntrySalts {
            meta: Salt([9; 32]),
            ops: (0..n).map(|i| Salt([i as u8 + 1; 32])).collect(),
        },
    }
}

fn manifest(mode: Mode, intent: Intent, records: u64) -> Manifest {
    Manifest {
        format_version: varve_wire::FORMAT_VERSION,
        source_instance: "instance-a".into(),
        mode,
        intent,
        revisions: vec![revision_id(&schema())],
        record_count: records,
        attachments_bundled: false,
    }
}

fn rib_block() -> varve_schema::Block {
    varve_schema::Block {
        id: varve_core::BlockId::new("rib"),
        version: 2,
        group: varve_schema::Group {
            id: varve_core::GroupId::new("rib"),
            label: "RIB".into(),
            cardinality: varve_schema::Cardinality::One,
            children: vec![Element::Column(Column {
                id: ColumnId::new("iban"),
                label: "IBAN".into(),
                ty: ScalarType::Text,
                arity: Arity::One,
            })],
            included_from: None,
        },
        resolvers: vec![],
    }
}

fn with_rib() -> Schema {
    let mut s = schema();
    rib_block().include_into(&mut s, None).unwrap();
    s
}

fn schema_lines() -> Vec<Line> {
    vec![Line::Revision {
        id: revision_id(&schema()),
        schema: schema(),
    }]
}

fn sample_log() -> RecordLog {
    let mut log = RecordLog::new(RecordId::new("r1"));
    log.append(draft(0, 0, vec![set("name", "Dupont")]))
        .unwrap();
    log.append(draft(1, 1, vec![set("name", "Durand")]))
        .unwrap();
    log
}

#[test]
fn history_round_trip_is_byte_stable_and_verifiable() {
    // M3: the corpus in and out, byte-stable.
    let log = sample_log();
    let record = RecordId::new("r1");
    let bytes = write_history(
        manifest(Mode::History, Intent::CreateOnly, 1),
        schema_lines(),
        &[(record.clone(), &log)],
    )
    .unwrap();

    let stream = read_stream(&bytes).unwrap();
    // Re-emitting the parsed lines yields the identical bytes.
    assert_eq!(write_lines(&stream.lines).unwrap(), bytes);

    // Adopt on a fresh instance: the chain verifies and continues.
    let mut store = BTreeMap::new();
    let outcome = adopt_history(&stream, &mut store).unwrap();
    assert_eq!(outcome.created, vec![record.clone()]);
    let adopted = &store[&record];
    adopted.verify_chain().unwrap();
    assert_eq!(adopted.entries().len(), 2);
    // The imported chain is the same chain: identical entry hashes —
    // tamper-evidence spans both instances (§6).
    assert_eq!(adopted.entries()[1].hash(), log.entries()[1].hash());
}

#[test]
fn snapshot_round_trip_and_import_as_log_entry() {
    let log = sample_log();
    let folded = log.fold().unwrap().values;
    let record = RecordId::new("r1");
    let bytes = write_snapshot(
        manifest(Mode::Snapshot, Intent::Upsert, 1),
        schema_lines(),
        &[SnapshotRecord {
            record: record.clone(),
            lens: revision_id(&schema()),
            values: folded.clone(),
        }],
    )
    .unwrap();
    let stream = read_stream(&bytes).unwrap();
    assert_eq!(write_lines(&stream.lines).unwrap(), bytes);

    // Import into an empty store: one entry, a patch against empty,
    // whose fold equals the exported state — and it is an ORDINARY log
    // entry (never a side door, §5).
    let mut store = BTreeMap::new();
    let salts = test_salts(7);
    let request = SnapshotImportRequest {
        actor: Actor {
            id: "importer".into(),
            kind: ActorKind::System,
        },
        timestamp: Instant::parse("2026-08-17T12:00:00Z").unwrap(),
        revision: revision_id(&schema()),
        note: Some("import".into()),
        salts_for: &salts,
    };
    let outcome = import_snapshot(&stream, &mut store, &request).unwrap();
    assert_eq!(outcome.created, vec![record.clone()]);
    let imported = &store[&record];
    assert_eq!(imported.entries().len(), 1);
    assert_eq!(imported.fold().unwrap().values, folded);
    imported.verify_chain().unwrap();
    assert_eq!(imported.entries()[0].envelope.actor.id, "importer");
}

#[test]
fn record_and_item_lines_pin_canonical_shapes() {
    // §5, as test vectors: a `record` line carries root cells keyed by
    // column (absent key = absent, `null` = empty, scalar object = one,
    // array = many); `item` lines follow, parents first, with `ord`.
    // §2.13 decisions 2–3: exact integers are strings; geometry is a
    // JSON value with ES6 numbers. Changing these bytes changes hashes.
    let mut values = RecordValues::new();
    values.cells.insert(
        CellAddr {
            column: ColumnId::new("n"),
            path: RowPath::root(),
        },
        CellState::Value(CellValue::One(Scalar::Integer(i64::MAX))),
    );
    values.cells.insert(
        CellAddr {
            column: ColumnId::new("blank"),
            path: RowPath::root(),
        },
        CellState::Empty,
    );
    values.cells.insert(
        CellAddr { column: ColumnId::new("g"), path: RowPath::root() },
        CellState::Value(CellValue::One(Scalar::Geometry(Box::new(
            varve_value::Feature::parse(
                r#"{"type":"Feature","id":1,"geometry":{"type":"Point","coordinates":[-0.0,2.5,1e21]},"properties":null}"#,
            )
            .unwrap(),
        )))),
    );
    let contacts = GroupId::new("contacts");
    let item = |i: &str| {
        RowPath::root().child(PathSeg {
            group: contacts.clone(),
            item: ItemId::new(i),
        })
    };
    values.items.insert(
        ItemsAddr {
            group: contacts.clone(),
            parent: RowPath::root(),
        },
        vec![ItemId::new("c1"), ItemId::new("c2")],
    );
    values.cells.insert(
        CellAddr {
            column: ColumnId::new("tags"),
            path: item("c2"),
        },
        CellState::Value(CellValue::Many(vec![
            Scalar::Enum(OptionId::new("a")),
            Scalar::Enum(OptionId::new("b")),
        ])),
    );
    let lens = revision_id(&schema());
    let rec = SnapshotRecord {
        record: RecordId::new("r1"),
        lens: lens.clone(),
        values,
    };
    let lines = rec.lines();
    let text = String::from_utf8(write_lines(&lines).unwrap()).unwrap();
    assert_eq!(
        text,
        format!(
            concat!(
                r#"{{"cells":{{"blank":null,"g":{{"geometry":{{"geometry":{{"coordinates":[0,2.5,1e+21],"type":"Point"}},"id":1,"properties":null,"type":"Feature"}}}},"n":{{"integer":"9223372036854775807"}}}},"id":"r1","k":"record","lens":"{lens}"}}"#,
                "\n",
                r#"{{"cells":{{}},"group":"contacts","id":"c1","k":"item","ord":0,"parent":[],"record":"r1"}}"#,
                "\n",
                r#"{{"cells":{{"tags":[{{"option":"a"}},{{"option":"b"}}]}},"group":"contacts","id":"c2","k":"item","ord":1,"parent":[],"record":"r1"}}"#,
                "\n",
            ),
            lens = lens
        )
    );
    // Reads back to the same lines, and reassembles to the same record.
    let mut prelude = vec![Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 1))];
    prelude.extend(schema_lines());
    let mut bytes = write_lines(&prelude).unwrap();
    bytes.extend_from_slice(text.as_bytes());
    let stream = read_stream(&bytes).unwrap();
    assert_eq!(&stream.lines[2..], lines.as_slice());
    assert_eq!(snapshot_records(&stream), vec![rec]);

    // A structural count that is not a JCS-safe integer is refused —
    // it can never have been produced by a JCS serializer.
    let bad = text.replace(r#""integer":"9223372036854775807""#, r#""integer":"007""#);
    let mut bytes = write_lines(&prelude).unwrap();
    bytes.extend_from_slice(bad.as_bytes());
    assert!(matches!(
        read_stream(&bytes),
        Err(ReadError::Malformed { line: 3, .. })
    ));
    // Nor can the writer produce one: a count beyond the safe range is
    // a `WriteError`, not a rounded number and not a panic.
    assert!(matches!(
        write_lines(&[Line::Header(manifest(
            Mode::Snapshot,
            Intent::Upsert,
            9007199254740993
        ))]),
        Err(WriteError::Canonical { line: 1, .. })
    ));
    // And on the read side a too-large count literal is a double, which
    // is not a count.
    let bad = write_lines(&[Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 1))]).unwrap();
    let bad = String::from_utf8(bad)
        .unwrap()
        .replace(r#""record_count":1"#, r#""record_count":9007199254740993"#);
    assert!(matches!(
        read_stream(bad.as_bytes()),
        Err(ReadError::Malformed { line: 1, .. })
    ));
}

#[test]
fn item_lines_obey_the_contiguity_rule() {
    // §5: a record's item lines follow its record line immediately,
    // parents before children, in order; any other line closes it.
    let mut prelude = vec![Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 2))];
    prelude.extend(schema_lines());
    let header = write_lines(&prelude).unwrap();
    let rec = |id: &str| {
        Line::Record(RecordLine {
            record: RecordId::new(id),
            lens: revision_id(&schema()),
            cells: Default::default(),
        })
    };
    let item = |rec: &str, group: &str, parent: RowPath, id: &str, ord: usize| {
        Line::Item(ItemLine {
            record: RecordId::new(rec),
            group: GroupId::new(group),
            parent,
            id: ItemId::new(id),
            ord,
            cells: Default::default(),
        })
    };
    let g1 = |i: &str| {
        RowPath::root().child(PathSeg {
            group: GroupId::new("g1"),
            item: ItemId::new(i),
        })
    };
    let stream = |lines: &[Line]| {
        let mut b = header.clone();
        b.extend_from_slice(&write_lines(lines).unwrap());
        read_stream(&b)
    };
    // Well-formed: r1 with two g1 items and a nested g2 item, then r2.
    let ok = [
        rec("r1"),
        item("r1", "g1", RowPath::root(), "a", 0),
        item("r1", "g1", RowPath::root(), "b", 1),
        item("r1", "g2", g1("a"), "x", 0),
        rec("r2"),
    ];
    let s = stream(&ok).unwrap();
    let records = snapshot_records(&s);
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0].values.items[&ItemsAddr {
            group: GroupId::new("g1"),
            parent: RowPath::root()
        }],
        vec![ItemId::new("a"), ItemId::new("b")]
    );
    assert_eq!(
        records[0].values.items[&ItemsAddr {
            group: GroupId::new("g2"),
            parent: g1("a")
        }],
        vec![ItemId::new("x")]
    );
    // Item for a record that is not the open one.
    assert!(matches!(
        stream(&[
            rec("r1"),
            rec("r2"),
            item("r1", "g1", RowPath::root(), "a", 0)
        ]),
        Err(ReadError::Malformed { line: 5, .. })
    ));
    // Ord out of sequence.
    assert!(matches!(
        stream(&[
            rec("r1"),
            item("r1", "g1", RowPath::root(), "a", 1),
            rec("r2")
        ]),
        Err(ReadError::Malformed { line: 4, .. })
    ));
    // Child before its parent.
    assert!(matches!(
        stream(&[rec("r1"), item("r1", "g2", g1("a"), "x", 0), rec("r2")]),
        Err(ReadError::Malformed { line: 4, .. })
    ));
    // Duplicate item id in one list.
    assert!(matches!(
        stream(&[
            rec("r1"),
            item("r1", "g1", RowPath::root(), "a", 0),
            item("r1", "g1", RowPath::root(), "a", 1),
            rec("r2")
        ]),
        Err(ReadError::Malformed { line: 5, .. })
    ));
    // Duplicate record: a stream is authoritative for a record once.
    assert!(matches!(
        stream(&[rec("r1"), rec("r1")]),
        Err(ReadError::Malformed { line: 4, .. })
    ));
}

#[test]
fn reader_refuses_the_alternative_blank_encodings() {
    // §2.4 one state, one encoding: `{"many":[]}` and an empty item
    // list are not what the writer emits and are refused on read.
    let mut prelude = vec![Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 1))];
    prelude.extend(schema_lines());
    let header = write_lines(&prelude).unwrap();
    let lens = revision_id(&schema());
    let with = |cells: &str| {
        let mut bytes = header.clone();
        bytes.extend_from_slice(
            format!(r#"{{"cells":{{{cells}}},"id":"r1","k":"record","lens":"{lens}"}}"#).as_bytes(),
        );
        bytes.push(b'\n');
        bytes
    };
    let ok = with(r#""tags":null"#);
    assert!(read_stream(&ok).is_ok());
    // Two values for one key: a JCS serializer never emits that, and a
    // stream is authoritative for each cell once — malformed, never
    // last-wins.
    let dup = with(r#""tags":null,"tags":{"text":"x"}"#);
    assert!(matches!(
        read_stream(&dup),
        Err(ReadError::Malformed { line: 3, .. })
    ));
    let empty_many = with(r#""tags":[]"#);
    assert!(matches!(
        read_stream(&empty_many),
        Err(ReadError::Malformed { line: 3, .. })
    ));
    // The old wrapped shapes are not the encoding.
    let wrapped = with(r#""tags":{"many":[{"option":"a"}]}"#);
    assert!(matches!(
        read_stream(&wrapped),
        Err(ReadError::Malformed { line: 3, .. })
    ));
    // Strict scalars: one value, one text — a multi-kind scalar object,
    // an unknown key, a non-normalized decimal or datetime, uppercase or
    // signed hex are all refused rather than read as something.
    for bad in [
        r#""x":{"text":"a","boolean":true}"#,
        r#""x":{"decimal":"1.50"}"#,
        r#""x":{"decimal":".5"}"#,
        r#""x":{"datetime":"2026-08-16T14:00:00+02:00"}"#,
        r#""x":{"attachment":{"id":"f","hash":"sha256:+f00000000000000000000000000000000000000000000000000000000000000","filename":"f","content_type":"a/b","byte_size":1}}"#,
        r#""x":{"attachment":{"id":"f","hash":"sha256:0000000000000000000000000000000000000000000000000000000000000000","filename":"f","content_type":"a/b","byte_size":1,"extra":1}}"#,
    ] {
        assert!(
            matches!(
                read_stream(&with(bad)),
                Err(ReadError::Malformed { line: 3, .. })
            ),
            "{bad}"
        );
    }
    assert!(read_stream(&with(r#""x":{"decimal":"1.5"}"#)).is_ok());
}

#[test]
fn intent_makes_id_mismatches_fail_loudly() {
    let record = RecordId::new("r1");
    let bytes = write_history(
        manifest(Mode::History, Intent::CreateOnly, 1),
        schema_lines(),
        &[(record.clone(), &sample_log())],
    )
    .unwrap();
    let stream = read_stream(&bytes).unwrap();
    // create-only into a store that already has r1: rejected.
    let mut store = BTreeMap::from([(record.clone(), sample_log())]);
    assert!(matches!(
        adopt_history(&stream, &mut store),
        Err(ImportError::AlreadyExists(r)) if r == record
    ));

    // update-only for an unknown id: rejected.
    let bytes = write_history(
        manifest(Mode::History, Intent::UpdateOnly, 1),
        schema_lines(),
        &[(record.clone(), &sample_log())],
    )
    .unwrap();
    let stream = read_stream(&bytes).unwrap();
    let mut empty = BTreeMap::new();
    assert!(matches!(
        adopt_history(&stream, &mut empty),
        Err(ImportError::NotFound(_))
    ));
}

#[test]
fn imports_are_all_or_nothing() {
    // §5: "import rejects … or commits to the whole stream". A stream
    // whose second record fails intent must leave the first uncreated.
    let fresh = RecordId::new("fresh");
    let old = RecordId::new("old");
    let mut old_log = RecordLog::new(old.clone());
    old_log
        .append(draft(0, 0, vec![set("name", "Old")]))
        .unwrap();
    let mut fresh_log = RecordLog::new(fresh.clone());
    fresh_log
        .append(draft(0, 0, vec![set("name", "New")]))
        .unwrap();

    // History mode.
    let bytes = write_history(
        manifest(Mode::History, Intent::CreateOnly, 2),
        schema_lines(),
        &[(fresh.clone(), &fresh_log), (old.clone(), &old_log)],
    )
    .unwrap();
    let stream = read_stream(&bytes).unwrap();
    let mut store = BTreeMap::from([(old.clone(), old_log.clone())]);
    assert!(
        matches!(adopt_history(&stream, &mut store), Err(ImportError::AlreadyExists(r)) if r == old)
    );
    assert!(
        !store.contains_key(&fresh),
        "first record was created despite the failure"
    );
    assert_eq!(store.len(), 1);

    // Snapshot mode.
    let folded = |log: &RecordLog| log.fold().unwrap().values;
    let bytes = write_snapshot(
        manifest(Mode::Snapshot, Intent::CreateOnly, 2),
        schema_lines(),
        &[
            SnapshotRecord {
                record: fresh.clone(),
                lens: revision_id(&schema()),
                values: folded(&fresh_log),
            },
            SnapshotRecord {
                record: old.clone(),
                lens: revision_id(&schema()),
                values: folded(&old_log),
            },
        ],
    )
    .unwrap();
    let stream = read_stream(&bytes).unwrap();
    let salts = test_salts(3);
    let request = SnapshotImportRequest {
        actor: Actor {
            id: "importer".into(),
            kind: ActorKind::System,
        },
        timestamp: Instant::parse("2026-08-17T12:00:00Z").unwrap(),
        revision: revision_id(&schema()),
        note: None,
        salts_for: &salts,
    };
    let mut store = BTreeMap::from([(old.clone(), old_log.clone())]);
    assert!(
        matches!(import_snapshot(&stream, &mut store, &request), Err(ImportError::AlreadyExists(r)) if r == old)
    );
    assert!(!store.contains_key(&fresh));
    assert_eq!(
        store[&old].entries().len(),
        1,
        "existing record was touched despite the failure"
    );
}

#[test]
fn a_schema_at_the_nesting_bound_survives_the_wire() {
    // The schema policy's structural bound exists for this: the deepest
    // valid schema stays inside the reader's JSON nesting budget.
    let policy = varve_schema::DepthPolicy::default();
    let mut el = Element::Column(Column {
        id: ColumnId::new("leaf"),
        label: "leaf".into(),
        ty: ScalarType::Text,
        arity: Arity::One,
    });
    for i in 0..policy.max_group_depth {
        el = Element::Group(varve_schema::Group {
            id: varve_core::GroupId::new(format!("g{i}")),
            label: "g".into(),
            cardinality: varve_schema::Cardinality::One,
            children: vec![el],
            included_from: None,
        });
    }
    let deep = Schema {
        root: vec![el],
        resolvers: vec![],
    };
    assert_eq!(varve_schema::validate(&deep, policy), vec![]);
    let mut m = manifest(Mode::Snapshot, Intent::Upsert, 0);
    m.revisions = vec![revision_id(&deep)];
    let bytes = write_lines(&[
        Line::Header(m),
        Line::Revision {
            id: revision_id(&deep),
            schema: deep.clone(),
        },
    ])
    .unwrap();
    let stream = read_stream(&bytes).unwrap();
    assert!(matches!(&stream.lines[1], Line::Revision { schema, .. } if schema == &deep));
}

#[test]
fn writers_refuse_a_manifest_of_the_other_mode() {
    // History and snapshot never mix (§5): a real error, in release too.
    let m = manifest(Mode::Snapshot, Intent::Upsert, 0);
    assert!(matches!(
        write_history(m, schema_lines(), &[]),
        Err(WriteError::Mode(Mode::Snapshot))
    ));
    let m = manifest(Mode::History, Intent::Upsert, 0);
    assert!(matches!(
        write_snapshot(m, schema_lines(), &[]),
        Err(WriteError::Mode(Mode::History))
    ));
}

#[test]
fn history_upsert_extends_or_diverges() {
    // Under upsert an imported history that extends the existing chain
    // replaces it; a shorter or diverging one is a conflict of
    // histories, reported as such — not as a tamper.
    let record = RecordId::new("r1");
    let mut two = sample_log();
    let mut three = two.clone();
    three
        .append(draft(2, 2, vec![set("name", "Third")]))
        .unwrap();
    let export = |log: &RecordLog| {
        read_stream(
            &write_history(
                manifest(Mode::History, Intent::Upsert, 1),
                schema_lines(),
                &[(record.clone(), log)],
            )
            .unwrap(),
        )
        .unwrap()
    };
    // Extends: adopted, reported as updated.
    let mut store = BTreeMap::from([(record.clone(), two.clone())]);
    let outcome = adopt_history(&export(&three), &mut store).unwrap();
    assert_eq!(outcome.updated, vec![record.clone()]);
    assert_eq!(store[&record].entries().len(), 3);
    // Shorter: not an extension.
    let mut store = BTreeMap::from([(record.clone(), three.clone())]);
    assert!(
        matches!(adopt_history(&export(&two), &mut store), Err(ImportError::Diverges(r)) if r == record)
    );
    assert_eq!(store[&record].entries().len(), 3);
    // Diverging at the last entry.
    two.append(draft(2, 2, vec![set("name", "Other third")]))
        .unwrap();
    let mut store = BTreeMap::from([(record.clone(), three)]);
    assert!(matches!(
        adopt_history(&export(&two), &mut store),
        Err(ImportError::Diverges(_))
    ));
}

#[test]
fn tampered_history_is_rejected_on_import() {
    let record = RecordId::new("r1");
    let bytes = write_history(
        manifest(Mode::History, Intent::CreateOnly, 1),
        schema_lines(),
        &[(record.clone(), &sample_log())],
    )
    .unwrap();
    // Rewrite a value inside the exported bytes.
    let text = String::from_utf8(bytes)
        .unwrap()
        .replace("Dupont", "Martin");
    let stream = read_stream(text.as_bytes()).unwrap();
    let mut store = BTreeMap::new();
    assert!(matches!(
        adopt_history(&stream, &mut store),
        Err(ImportError::Chain(..))
    ));
}

#[test]
fn history_with_an_unsalted_op_is_rejected_at_the_reader() {
    // Defense in depth for the chain: an entry line whose `ops` and
    // `op_salts` disagree in length is malformed on its face — the
    // reader refuses it before `verify_chain` ever sees it.
    let record = RecordId::new("r1");
    let mut entries = sample_log().entries().to_vec();
    entries[0].content.ops.push(set("name", "MALLORY").into());
    let tampered = RecordLog::from_entries(RecordId::new("r1"), entries);
    let bytes = write_history(
        manifest(Mode::History, Intent::CreateOnly, 1),
        schema_lines(),
        &[(record, &tampered)],
    )
    .unwrap();
    assert!(matches!(
        read_stream(&bytes),
        Err(ReadError::Malformed { .. })
    ));
}

#[test]
fn reader_fails_fast_and_rejects_mixed_modes() {
    // No header.
    assert!(matches!(
        read_stream(b"{\"k\":\"revision\"}\n"),
        Err(ReadError::MissingHeader)
    ));
    // Not JSON.
    let bad = format!(
        "{}\nnot json\n",
        String::from_utf8(
            write_lines(&[Line::Header(manifest(Mode::History, Intent::Upsert, 0))]).unwrap()
        )
        .unwrap()
        .trim_end()
    );
    assert!(matches!(
        read_stream(bad.as_bytes()),
        Err(ReadError::Json { line: 2 })
    ));
    // Unsupported version.
    let mut m = manifest(Mode::History, Intent::Upsert, 0);
    m.format_version = 99;
    let bytes = write_lines(&[Line::Header(m)]).unwrap();
    assert!(matches!(
        read_stream(&bytes),
        Err(ReadError::UnsupportedVersion(99))
    ));

    // A record line in a history stream: two sources of truth, rejected.
    let mixed = write_lines(&[
        Line::Header(manifest(Mode::History, Intent::Upsert, 1)),
        Line::Record(RecordLine {
            record: RecordId::new("r1"),
            lens: RevisionId::new("x"),
            cells: Default::default(),
        }),
    ])
    .unwrap();
    assert!(matches!(
        read_stream(&mixed),
        Err(ReadError::ModeMismatch { line: 2, .. })
    ));

    // Manifest count disagrees with the stream.
    let short = write_lines(&[Line::Header(manifest(Mode::History, Intent::Upsert, 3))]).unwrap();
    assert!(matches!(
        read_stream(&short),
        Err(ReadError::RecordCountMismatch {
            expected: 3,
            got: 0
        })
    ));
}

#[test]
fn reader_checks_versions_first_and_revisions_for_consistency() {
    // A header from another version may not have this version's shape:
    // the verdict must be "unsupported version", not "malformed".
    let foreign = br#"{"format_version":7,"k":"header","something":"else"}"#;
    assert!(matches!(
        read_stream(foreign),
        Err(ReadError::UnsupportedVersion(7))
    ));

    // A revision line whose id is not its schema's hash lies about
    // identity.
    let mut lines = vec![Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 0))];
    lines.push(Line::Revision {
        id: RevisionId::new("bogus"),
        schema: schema(),
    });
    let bytes = write_lines(&lines).unwrap();
    assert!(matches!(
        read_stream(&bytes),
        Err(ReadError::Malformed { line: 2, .. })
    ));

    // The manifest declares exactly the revisions the stream carries.
    let bytes = write_lines(&[Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 0))]).unwrap();
    assert!(matches!(
        read_stream(&bytes),
        Err(ReadError::RevisionsMismatch)
    ));

    // A lens the stream does not carry: the data would arrive without
    // its schema.
    let mut m = manifest(Mode::Snapshot, Intent::Upsert, 1);
    let other = revision_id(&Schema::default());
    m.revisions.push(other.clone());
    let mut lines = vec![Line::Header(m)];
    lines.extend(schema_lines());
    lines.push(Line::Revision {
        id: other,
        schema: Schema::default(),
    });
    lines.push(Line::Record(RecordLine {
        record: RecordId::new("r1"),
        lens: RevisionId::new("elsewhere"),
        cells: Default::default(),
    }));
    let bytes = write_lines(&lines).unwrap();
    assert!(
        matches!(read_stream(&bytes), Err(ReadError::RevisionNotCarried(r)) if r == RevisionId::new("elsewhere"))
    );

    // Unknown keys on a line are refused.
    let mut lines = vec![Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 0))];
    lines.extend(schema_lines());
    let text = String::from_utf8(write_lines(&lines).unwrap())
        .unwrap()
        .replace(r#""k":"revision""#, r#""k":"revision","junk":1"#);
    assert!(matches!(
        read_stream(text.as_bytes()),
        Err(ReadError::Malformed { line: 2, .. })
    ));
}

#[test]
fn schema_and_nomenclature_lines_round_trip() {
    let mut header = manifest(Mode::Snapshot, Intent::Upsert, 0);
    header.revisions.push(revision_id(&with_rib()));
    let lines = vec![
        Line::Header(header),
        Line::Revision {
            id: revision_id(&schema()),
            schema: schema(),
        },
        Line::Nomenclature {
            id: varve_core::NomenclatureId::new("cog"),
            version: 3,
            rows: vec![OptionRow {
                id: OptionId::new("01053"),
                label: "Bourg-en-Bresse".into(),
                fields: vec![("departement".into(), "01".into())],
            }],
        },
        Line::Attachment {
            hash: varve_record::genesis_hash(&RecordId::new("r1")),
            byte_size: 1234,
            content_type: "application/pdf".into(),
        },
        // A published block travels like a nomenclature (§2.1); a
        // schema that includes it carries the provenance.
        Line::Block(rib_block()),
        Line::Revision {
            id: revision_id(&with_rib()),
            schema: with_rib(),
        },
    ];
    let bytes = write_lines(&lines).unwrap();
    let stream = read_stream(&bytes).unwrap();
    assert_eq!(stream.lines, lines);
    assert_eq!(write_lines(&stream.lines).unwrap(), bytes);
    // The line's canonical shape: `k` plus the block's own fields.
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains(r#"{"group":{"group":{"cardinality":"one","children":[{"column":{"arity":"one","id":"iban","label":"IBAN","type":{"kind":"text"}}}],"id":"rib","label":"RIB"}},"id":"rib","k":"block","resolvers":[],"version":2}"#), "{text}");
    assert!(text.contains(r#""included_from":{"id":"rib","version":2}"#));
    // A record line with an empty root cell round-trips too.
    let rec = Line::Record(RecordLine {
        record: RecordId::new("r9"),
        lens: revision_id(&schema()),
        cells: [(ColumnId::new("name"), CellState::Empty)]
            .into_iter()
            .collect(),
    });
    let mut lines = vec![Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 1))];
    lines.extend(schema_lines());
    lines.push(rec.clone());
    let bytes = write_lines(&lines).unwrap();
    assert_eq!(read_stream(&bytes).unwrap().lines[2], rec);
}
