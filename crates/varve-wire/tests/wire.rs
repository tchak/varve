use std::collections::BTreeMap;

use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, OptionId, RecordId, RevisionId, RowPath};
use varve_record::{Actor, ActorKind, Draft, EntrySalts, Origin, RecordLog};
use varve_schema::{
    Arity, Column, Element, NomenclatureRef, OptionRow, ScalarType, Schema, revision_id,
};
use varve_value::{CellAddr, CellState, CellValue, Op, RecordValues, Scalar};
use varve_wire::{
    ImportError, Intent, Line, Manifest, Mode, ReadError, RecordLine, WriteError,
    SnapshotImportRequest, adopt_history, import_snapshot, read_stream, test_salts,
    write_history, write_lines, write_snapshot,
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
        actor: Actor { id: "a1".into(), kind: ActorKind::Human },
        timestamp: Instant::parse(&format!("2026-08-17T10:{minute:02}:00Z")).unwrap(),
        revision: revision_id(&schema()),
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

fn schema_lines() -> Vec<Line> {
    vec![Line::Revision { id: revision_id(&schema()), schema: schema() }]
}

fn sample_log() -> RecordLog {
    let mut log = RecordLog::new();
    log.append(draft(0, 0, vec![set("name", "Dupont")])).unwrap();
    log.append(draft(1, 1, vec![set("name", "Durand")])).unwrap();
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
    ).unwrap();

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
        vec![RecordLine {
            record: record.clone(),
            lens: revision_id(&schema()),
            values: folded.clone(),
        }],
    ).unwrap();
    let stream = read_stream(&bytes).unwrap();
    assert_eq!(write_lines(&stream.lines).unwrap(), bytes);

    // Import into an empty store: one entry, a patch against empty,
    // whose fold equals the exported state — and it is an ORDINARY log
    // entry (never a side door, §5).
    let mut store = BTreeMap::new();
    let salts = test_salts(7);
    let request = SnapshotImportRequest {
        actor: Actor { id: "importer".into(), kind: ActorKind::System },
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
fn record_line_pins_canonical_number_shapes() {
    // §2.13 decisions 2–3, as test vectors: exact integers are strings
    // (a JSON number is a JCS double and cannot carry a full i64);
    // geometry is embedded as a JSON value with ES6 numbers, never a
    // stringified blob. Changing these bytes changes every record hash.
    let mut values = RecordValues::new();
    values.cells.insert(
        CellAddr { column: ColumnId::new("n"), path: RowPath::root() },
        CellState::Value(CellValue::One(Scalar::Integer(i64::MAX))),
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
    let line = Line::Record(RecordLine {
        record: RecordId::new("r1"),
        lens: RevisionId::new("lens"),
        values,
    });
    let text = String::from_utf8(write_lines(std::slice::from_ref(&line)).unwrap()).unwrap();
    assert_eq!(
        text,
        concat!(
            r#"{"cells":["#,
            r#"{"column":"g","path":[],"state":{"one":{"geometry":{"geometry":{"coordinates":[0,2.5,1e+21],"type":"Point"},"id":1,"properties":null,"type":"Feature"}}}},"#,
            r#"{"column":"n","path":[],"state":{"one":{"integer":"9223372036854775807"}}}"#,
            r#"],"id":"r1","items":[],"k":"record","lens":"lens"}"#,
            "\n"
        )
    );
    // And it reads back to the same line.
    let header = write_lines(&[Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 1))]).unwrap();
    let mut bytes = header;
    bytes.extend_from_slice(text.as_bytes());
    let stream = read_stream(&bytes).unwrap();
    assert_eq!(stream.lines[1], line);

    // A structural count that is not a JCS-safe integer is refused —
    // it can never have been produced by a JCS serializer.
    let bad = text.replace(r#""integer":"9223372036854775807""#, r#""integer":"007""#);
    let mut bytes = write_lines(&[Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 1))]).unwrap();
    bytes.extend_from_slice(bad.as_bytes());
    assert!(matches!(read_stream(&bytes), Err(ReadError::Malformed { line: 2, .. })));
    // Nor can the writer produce one: a count beyond the safe range is
    // a `WriteError`, not a rounded number and not a panic.
    assert!(matches!(
        write_lines(&[Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 9007199254740993))]),
        Err(WriteError { line: 1, .. })
    ));
    // And on the read side a too-large count literal is a double, which
    // is not a count.
    let bad = write_lines(&[Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 1))]).unwrap();
    let bad = String::from_utf8(bad).unwrap().replace(r#""record_count":1"#, r#""record_count":9007199254740993"#);
    assert!(matches!(read_stream(bad.as_bytes()), Err(ReadError::Malformed { line: 1, .. })));
}

#[test]
fn reader_refuses_the_alternative_blank_encodings() {
    // §2.4 one state, one encoding: `{"many":[]}` and an empty item
    // list are not what the writer emits and are refused on read.
    let header = write_lines(&[Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 1))]).unwrap();
    let with = |cells: &str, items: &str| {
        let mut bytes = header.clone();
        bytes.extend_from_slice(
            format!(r#"{{"cells":[{cells}],"id":"r1","items":[{items}],"k":"record","lens":"lens"}}"#)
                .as_bytes(),
        );
        bytes.push(b'\n');
        bytes
    };
    let ok = with(r#"{"column":"tags","path":[],"state":"empty"}"#, "");
    assert!(read_stream(&ok).is_ok());
    let empty_many = with(r#"{"column":"tags","path":[],"state":{"many":[]}}"#, "");
    assert!(matches!(read_stream(&empty_many), Err(ReadError::Malformed { line: 2, .. })));
    let empty_items = with("", r#"{"group":"contacts","items":[],"parent":[]}"#);
    assert!(matches!(read_stream(&empty_items), Err(ReadError::Malformed { line: 2, .. })));
}

#[test]
fn intent_makes_id_mismatches_fail_loudly() {
    let record = RecordId::new("r1");
    let bytes = write_history(
        manifest(Mode::History, Intent::CreateOnly, 1),
        schema_lines(),
        &[(record.clone(), &sample_log())],
    ).unwrap();
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
    ).unwrap();
    let stream = read_stream(&bytes).unwrap();
    let mut empty = BTreeMap::new();
    assert!(matches!(
        adopt_history(&stream, &mut empty),
        Err(ImportError::NotFound(_))
    ));
}

#[test]
fn tampered_history_is_rejected_on_import() {
    let record = RecordId::new("r1");
    let bytes = write_history(
        manifest(Mode::History, Intent::CreateOnly, 1),
        schema_lines(),
        &[(record.clone(), &sample_log())],
    ).unwrap();
    // Rewrite a value inside the exported bytes.
    let text = String::from_utf8(bytes).unwrap().replace("Dupont", "Martin");
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
    entries[0].content.ops.push(set("name", "MALLORY"));
    let tampered = RecordLog::from_entries(entries);
    let bytes = write_history(
        manifest(Mode::History, Intent::CreateOnly, 1),
        schema_lines(),
        &[(record, &tampered)],
    ).unwrap();
    assert!(matches!(read_stream(&bytes), Err(ReadError::Malformed { .. })));
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
        String::from_utf8(write_lines(&[Line::Header(manifest(
            Mode::History,
            Intent::Upsert,
            0
        ))]).unwrap())
        .unwrap()
        .trim_end()
    );
    assert!(matches!(read_stream(bad.as_bytes()), Err(ReadError::Json { line: 2 })));
    // Unsupported version.
    let mut m = manifest(Mode::History, Intent::Upsert, 0);
    m.format_version = 99;
    let bytes = write_lines(&[Line::Header(m)]).unwrap();
    assert!(matches!(read_stream(&bytes), Err(ReadError::UnsupportedVersion(99))));

    // A record line in a history stream: two sources of truth, rejected.
    let mixed = write_lines(&[
        Line::Header(manifest(Mode::History, Intent::Upsert, 1)),
        Line::Record(RecordLine {
            record: RecordId::new("r1"),
            lens: RevisionId::new("x"),
            values: RecordValues::new(),
        }),
    ]).unwrap();
    assert!(matches!(
        read_stream(&mixed),
        Err(ReadError::ModeMismatch { line: 2, .. })
    ));

    // Manifest count disagrees with the stream.
    let short = write_lines(&[Line::Header(manifest(Mode::History, Intent::Upsert, 3))]).unwrap();
    assert!(matches!(
        read_stream(&short),
        Err(ReadError::RecordCountMismatch { expected: 3, got: 0 })
    ));
}

#[test]
fn schema_and_nomenclature_lines_round_trip() {
    let lines = vec![
        Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 0)),
        Line::Revision { id: revision_id(&schema()), schema: schema() },
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
            hash: varve_record::genesis_hash(),
            byte_size: 1234,
            content_type: "application/pdf".into(),
        },
    ];
    let bytes = write_lines(&lines).unwrap();
    let stream = read_stream(&bytes).unwrap();
    assert_eq!(stream.lines, lines);
    assert_eq!(write_lines(&stream.lines).unwrap(), bytes);
    // A cell address round-trips too (record line with items).
    let mut values = RecordValues::new();
    values.cells.insert(
        CellAddr { column: ColumnId::new("name"), path: RowPath::root() },
        CellState::Empty,
    );
    let rec = Line::Record(RecordLine {
        record: RecordId::new("r9"),
        lens: revision_id(&schema()),
        values,
    });
    let bytes = write_lines(&[Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 1)), rec.clone()]).unwrap();
    assert_eq!(read_stream(&bytes).unwrap().lines[1], rec);
}
