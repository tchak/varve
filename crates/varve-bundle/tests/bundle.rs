//! The Q14 Tier 5 half, end to end (§2.15, §13.6): a history bundle —
//! stream + sidecar — exported from one store and imported whole into
//! another; the sidecar deterministic byte for byte; every §2.15 import
//! rule exercised as a negative.

use std::collections::BTreeMap;
use std::time::SystemTime;

use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, GroupId, RecordId, ResolverId, RowPath, SurfaceId};
use varve_files::{BlobStore, MemoryKeyring, ObjectBlobStore, PutMeta};
use varve_record::{
    Actor, ActorKind, Draft, EntryOp, EntrySalts, Origin, Outcome, RecordLog, Transition,
};
use varve_schema::{Schema, revision_id};
use varve_surface::{BlockDefaults, GroupNode, Surface};
use varve_value::{CellState, CellValue, Op, Scalar};
use varve_wire::{Intent, Line, Manifest, Mode, Stream, adopt_history, read_stream, write_lines};

use varve_bundle::{
    BundleError, block_defaults, block_defaults_line, import_sidecar, surface_line, surfaces,
    write_sidecar,
};

fn store() -> ObjectBlobStore<MemoryKeyring> {
    ObjectBlobStore::memory(MemoryKeyring::default())
}

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_787_000_000)
}

async fn put(
    store: &impl BlobStore,
    bytes: &[u8],
    content_type: &str,
) -> varve_core::canonical::ContentHash {
    store
        .put(
            PutMeta {
                content_type: content_type.into(),
                created_at: now(),
            },
            bytes,
        )
        .await
        .unwrap()
}

fn salts(n: usize) -> EntrySalts {
    EntrySalts {
        meta: Salt([9; 32]),
        ops: (0..n).map(|i| Salt([i as u8 + 1; 32])).collect(),
    }
}

/// A record whose log references one attachment blob (a cell) and one
/// payload blob (a landed resolution) — the two §2.15 blob classes.
fn sample_log(
    revision: &varve_core::RevisionId,
    attachment: varve_core::canonical::ContentHash,
    payload: varve_core::canonical::ContentHash,
) -> RecordLog {
    let mut log = RecordLog::new(RecordId::new("r1"));
    let draft = |base: u64, actor: &str, kind: ActorKind, ops: Vec<EntryOp>| Draft {
        actor: Actor {
            id: actor.into(),
            kind,
        },
        timestamp: Instant::parse("2026-08-19T10:00:00Z").unwrap(),
        revision: revision.clone(),
        base_version: base,
        origin: Origin::Entered,
        note: None,
        salts: salts(ops.len()),
        ops,
    };
    log.append(draft(
        0,
        "a1",
        ActorKind::Human,
        vec![
            Op::Set {
                column: ColumnId::new("piece"),
                path: RowPath::root(),
                state: CellState::Value(CellValue::Many(vec![Scalar::Attachment(Box::new(
                    varve_value::AttachmentRef {
                        id: "f1".into(),
                        hash: attachment,
                        filename: "f1.pdf".into(),
                        content_type: "application/pdf".into(),
                        byte_size: 11,
                    },
                ))])),
            }
            .into(),
            EntryOp::Resolution {
                anchor: GroupId::new("entreprise"),
                scope: RowPath::root(),
                transition: Transition::Request {
                    resolver: ResolverId::new("insee-sirene"),
                    resolver_version: 1,
                    mapping_version: 1,
                },
            },
        ],
    ))
    .unwrap();
    log.append(draft(
        1,
        "resolver:insee",
        ActorKind::Resolver,
        vec![EntryOp::Resolution {
            anchor: GroupId::new("entreprise"),
            scope: RowPath::root(),
            transition: Transition::Land {
                snapshot: payload,
                outcome: Outcome::default(),
            },
        }],
    ))
    .unwrap();
    log
}

fn sample_surface(revision: &varve_core::RevisionId) -> Surface {
    Surface {
        id: SurfaceId::new("depot"),
        revision: revision.clone(),
        nodes: vec![],
        ineligibility: None,
    }
}

fn sample_defaults() -> BlockDefaults {
    BlockDefaults {
        block: varve_schema::BlockRef {
            id: varve_core::BlockId::new("rib"),
            version: 1,
        },
        node: GroupNode {
            group: GroupId::new("rib"),
            prompt: None,
            visibility: None,
            children: vec![],
        },
    }
}

/// Export a full history bundle from `source`: stream bytes + sidecar.
async fn export_bundle(source: &impl BlobStore) -> (Vec<u8>, Vec<u8>, Stream) {
    let schema = Schema::default();
    let revision = revision_id(&schema);
    let attachment = put(source, b"pdf bytes  ", "application/pdf").await;
    let payload = put(source, b"{\"insee\":1}", "application/json").await;
    let log = sample_log(&revision, attachment, payload);
    let mut lines = vec![
        Line::Header(Manifest {
            format_version: varve_wire::FORMAT_VERSION,
            source_instance: "source".into(),
            mode: Mode::History,
            intent: Intent::CreateOnly,
            revisions: vec![revision.clone()],
            record_count: 1,
            blobs_bundled: true,
        }),
        Line::Revision {
            id: revision.clone(),
            schema,
        },
        surface_line(&sample_surface(&revision)),
        block_defaults_line(&sample_defaults()),
        Line::Attachment {
            hash: attachment,
            byte_size: 11,
            content_type: "application/pdf".into(),
        },
        Line::Snapshot {
            hash: payload,
            byte_size: 11,
            content_type: "application/json".into(),
        },
    ];
    lines.extend(log.entries().iter().map(|entry| Line::Entry {
        record: RecordId::new("r1"),
        entry: entry.clone(),
    }));
    let bytes = write_lines(&lines).unwrap();
    let stream = read_stream(&bytes).unwrap();
    let mut sidecar = Vec::new();
    write_sidecar(&stream, source, &mut sidecar).await.unwrap();
    (bytes, sidecar, stream)
}

#[tokio::test]
async fn a_bundle_round_trips_between_instances() {
    let source = store();
    let (bytes, sidecar, stream) = export_bundle(&source).await;

    // The receiving instance: blobs first, stream second (§2.15).
    let target = store();
    let stored = import_sidecar(&stream, &target, sidecar.as_slice(), now())
        .await
        .unwrap();
    assert_eq!(stored.len(), 2);
    for hash in &stored {
        assert!(target.has(hash).await.unwrap());
        // The §2.15 goal, literally: the bytes are the same bytes.
        let mut a = Vec::new();
        let mut b = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut source.get(hash).await.unwrap(), &mut a)
            .await
            .unwrap();
        tokio::io::AsyncReadExt::read_to_end(&mut target.get(hash).await.unwrap(), &mut b)
            .await
            .unwrap();
        assert_eq!(a, b);
    }
    let mut records = BTreeMap::new();
    adopt_history(&stream, &mut records).unwrap();
    let adopted = &records[&RecordId::new("r1")];
    adopted.verify_chain().unwrap();
    // Everything the record's log references arrived with the bundle.
    let roots = adopted.referenced_blobs();
    for root in &roots {
        assert!(target.has(root).await.unwrap(), "root {root} not stored");
    }
    assert_eq!(roots.len(), 2);

    // The stream itself is byte-stable, surfaces and defaults decode.
    assert_eq!(write_lines(&stream.lines).unwrap(), bytes);
    let decoded = surfaces(&stream).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].id, SurfaceId::new("depot"));
    let defaults = block_defaults(&stream).unwrap();
    assert_eq!(defaults, vec![sample_defaults()]);

    // Idempotent: importing the same sidecar again stores nothing new
    // and succeeds (put is idempotent by hash — the §2.15 dedup path).
    import_sidecar(&stream, &target, sidecar.as_slice(), now())
        .await
        .unwrap();
}

#[tokio::test]
async fn the_sidecar_is_deterministic() {
    let (_, sidecar_a, _) = export_bundle(&store()).await;
    let (_, sidecar_b, _) = export_bundle(&store()).await;
    assert_eq!(sidecar_a, sidecar_b, "same blob set, same bytes");
    // And it is a real tar: entries in hash order under sha256/.
    // (Structural spot-check without the tar tool: magic at 257.)
    assert_eq!(&sidecar_a[257..262], b"ustar");
    assert!(sidecar_a.len() % 512 == 0);
}

#[tokio::test]
async fn import_enforces_the_exact_described_set() {
    let source = store();
    let (_, sidecar, stream) = export_bundle(&source).await;

    // A stream that does not declare a sidecar refuses one.
    let mut referenced = stream.clone();
    referenced.manifest.blobs_bundled = false;
    assert!(matches!(
        import_sidecar(&referenced, &store(), sidecar.as_slice(), now()).await,
        Err(BundleError::NotBundled)
    ));

    // Truncated archive: a described blob is missing.
    // (Cut before the second entry's header: one entry = header + 512.)
    let truncated = [&sidecar[..1024], &[0u8; 1024][..]].concat();
    assert!(matches!(
        import_sidecar(&stream, &store(), truncated.as_slice(), now()).await,
        Err(BundleError::MissingEntries(missing)) if missing.len() == 1
    ));

    // An extra, undescribed entry is a smuggling vector: refused.
    let smuggled = {
        let wider_source = store();
        let _ = put(&wider_source, b"pdf bytes  ", "application/pdf").await;
        let _ = put(&wider_source, b"{\"insee\":1}", "application/json").await;
        let extra = put(&wider_source, b"extra", "a/b").await;
        let mut lines = stream.lines.clone();
        lines.insert(
            4,
            Line::Attachment {
                hash: extra,
                byte_size: 5,
                content_type: "a/b".into(),
            },
        );
        let wider = read_stream(&write_lines(&lines).unwrap()).unwrap();
        let mut sidecar = Vec::new();
        write_sidecar(&wider, &wider_source, &mut sidecar)
            .await
            .unwrap();
        sidecar
    };
    assert!(matches!(
        import_sidecar(&stream, &store(), smuggled.as_slice(), now()).await,
        Err(BundleError::UnknownEntry(_))
    ));

    // Corrupt bytes: the entry no longer hashes to its name.
    let mut corrupt = sidecar.clone();
    let flip = 512 + 3; // inside the first entry's data
    corrupt[flip] ^= 0xff;
    assert!(matches!(
        import_sidecar(&stream, &store(), corrupt.as_slice(), now()).await,
        Err(BundleError::HashMismatch { .. })
    ));

    // A lying size: header disagrees with the description.
    let mut lying = sidecar.clone();
    // First entry's size field (octal, offset 124 in its header).
    lying[124..136].copy_from_slice(b"00000000007\0");
    assert!(matches!(
        import_sidecar(&stream, &store(), lying.as_slice(), now()).await,
        Err(BundleError::SizeMismatch { .. })
    ));

    // Non-file entry types (symlink) are refused outright.
    let mut linked = sidecar.clone();
    linked[156] = b'2';
    assert!(matches!(
        import_sidecar(&stream, &store(), linked.as_slice(), now()).await,
        Err(BundleError::MalformedArchive(_))
    ));

    // Export refuses a stream declaring `referenced`.
    let mut out = Vec::new();
    assert!(matches!(
        write_sidecar(&referenced, &source, &mut out).await,
        Err(BundleError::NotBundled)
    ));
}
