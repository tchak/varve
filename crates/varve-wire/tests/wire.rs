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
    ImportError, Intent, Line, Manifest, Mode, ReadError, RecordLine,
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
    );

    let stream = read_stream(&bytes).unwrap();
    // Re-emitting the parsed lines yields the identical bytes.
    assert_eq!(write_lines(&stream.lines), bytes);

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
    );
    let stream = read_stream(&bytes).unwrap();
    assert_eq!(write_lines(&stream.lines), bytes);

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
fn intent_makes_id_mismatches_fail_loudly() {
    let record = RecordId::new("r1");
    let bytes = write_history(
        manifest(Mode::History, Intent::CreateOnly, 1),
        schema_lines(),
        &[(record.clone(), &sample_log())],
    );
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
    );
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
    );
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
    );
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
        ))]))
        .unwrap()
        .trim_end()
    );
    assert!(matches!(read_stream(bad.as_bytes()), Err(ReadError::Json { line: 2 })));
    // Unsupported version.
    let mut m = manifest(Mode::History, Intent::Upsert, 0);
    m.format_version = 99;
    let bytes = write_lines(&[Line::Header(m)]);
    assert!(matches!(read_stream(&bytes), Err(ReadError::UnsupportedVersion(99))));

    // A record line in a history stream: two sources of truth, rejected.
    let mixed = write_lines(&[
        Line::Header(manifest(Mode::History, Intent::Upsert, 1)),
        Line::Record(RecordLine {
            record: RecordId::new("r1"),
            lens: RevisionId::new("x"),
            values: RecordValues::new(),
        }),
    ]);
    assert!(matches!(
        read_stream(&mixed),
        Err(ReadError::ModeMismatch { line: 2, .. })
    ));

    // Manifest count disagrees with the stream.
    let short = write_lines(&[Line::Header(manifest(Mode::History, Intent::Upsert, 3))]);
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
    let bytes = write_lines(&lines);
    let stream = read_stream(&bytes).unwrap();
    assert_eq!(stream.lines, lines);
    assert_eq!(write_lines(&stream.lines), bytes);
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
    let bytes = write_lines(&[Line::Header(manifest(Mode::Snapshot, Intent::Upsert, 1)), rec.clone()]);
    assert_eq!(read_stream(&bytes).unwrap().lines[1], rec);
}
