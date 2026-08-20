//! The store contract, executable (§13.2): generic checks over the
//! trait bounds — any implementation must pass them — run here against
//! [`MemoryStore`]. The recurring shape: the store enforces **index
//! rules only**; content invariants re-run in the loaders, so a
//! tampered row is accepted at write and caught at the first load.

use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{
    BlockId, ColumnId, GroupId, NomenclatureId, OptionId, RecordId, RevisionId, RowPath, SurfaceId,
};
use varve_record::{Actor, ActorKind, Draft, EntryOp, EntrySalts, Origin, RecordLog};
use varve_revision::{Publication, PublishBlockError, PublishNomenclatureError, RevisionDag};
use varve_schema::{
    Arity, Block, BlockRef, Cardinality, Column, DepthPolicy, Element, Group, OptionRow,
    ScalarType, Schema, revision_id,
};
use varve_store::load::{LoadError, load_blocks, load_dag, load_log, load_nomenclatures};
use varve_store::{
    BlockStore, LineageId, MemoryStore, NomenclatureStore, RecordLogStore, RevisionStore,
    StoreError, SurfaceStore,
};
use varve_surface::{BlockDefaults, GroupNode, Surface};
use varve_value::{CellState, CellValue, Op, Scalar};

// ---- fixtures -------------------------------------------------------

fn actor() -> Actor {
    Actor {
        id: "a1".into(),
        kind: ActorKind::Human,
    }
}

fn ts(minute: u8) -> Instant {
    Instant::parse(&format!("2026-08-20T10:{minute:02}:00Z")).unwrap()
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
        actor: actor(),
        timestamp: ts(minute),
        revision: RevisionId::new("rev-1"),
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

/// A valid log of `n` entries, minted by the kernel appender.
fn log_of(record: &str, n: u64) -> RecordLog {
    let mut log = RecordLog::new(RecordId::new(record));
    for i in 0..n {
        log.append(draft(i as u8, i, vec![set("name", &format!("v{i}"))]))
            .unwrap();
    }
    log
}

fn column(id: &str) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.to_string(),
        ty: ScalarType::Text,
        arity: Arity::One,
    })
}

fn schema(ids: &[&str]) -> Schema {
    Schema {
        root: ids.iter().map(|id| column(id)).collect(),
        resolvers: vec![],
    }
}

fn block(id: &str, version: u32, group: &str) -> Block {
    Block {
        id: BlockId::new(id),
        version,
        group: Group {
            id: GroupId::new(group),
            label: group.to_string(),
            cardinality: Cardinality::One,
            children: vec![column("street")],
            included_from: None,
        },
        resolvers: vec![],
    }
}

fn row(id: &str) -> OptionRow {
    OptionRow {
        id: OptionId::new(id),
        label: id.to_string(),
        fields: vec![],
    }
}

fn surface(revision: &str, id: &str) -> Surface {
    Surface {
        id: SurfaceId::new(id),
        revision: RevisionId::new(revision),
        nodes: vec![],
        ineligibility: None,
    }
}

// ---- record logs ----------------------------------------------------

async fn check_log_roundtrip(store: &impl RecordLogStore) {
    let record = RecordId::new("r1");
    let log = log_of("r1", 3);
    for entry in log.entries() {
        store.append(&record, entry).await.unwrap();
    }
    assert_eq!(store.version(&record).await.unwrap(), 3);

    let reloaded = load_log(store, &record).await.unwrap();
    assert_eq!(reloaded.entries(), log.entries());

    // Partial read: entries from a seq; past-the-end is empty.
    assert_eq!(store.entries(&record, 1).await.unwrap().len(), 2);
    assert_eq!(store.entries(&record, 3).await.unwrap().len(), 0);

    // Unknown record: empty, version 0 — creation is the first append.
    let ghost = RecordId::new("ghost");
    assert_eq!(store.entries(&ghost, 0).await.unwrap().len(), 0);
    assert_eq!(store.version(&ghost).await.unwrap(), 0);
    assert!(load_log(store, &ghost).await.unwrap().entries().is_empty());
}

async fn check_log_seq_conflict(store: &impl RecordLogStore) {
    let record = RecordId::new("r1");
    let log = log_of("r1", 2);
    store.append(&record, &log.entries()[0]).await.unwrap();

    // Replaying the same seq: the optimistic-concurrency refusal.
    let err = store.append(&record, &log.entries()[0]).await.unwrap_err();
    assert_eq!(
        err,
        StoreError::SeqConflict {
            record: record.clone(),
            next: 1,
            got: 0,
        }
    );
    // Skipping ahead is refused the same way.
    let mut ahead = log.entries()[1].clone();
    ahead.envelope.seq = 5;
    let err = store.append(&record, &ahead).await.unwrap_err();
    assert_eq!(
        err,
        StoreError::SeqConflict {
            record: record.clone(),
            next: 1,
            got: 5,
        }
    );
    // A refused append changes nothing.
    assert_eq!(store.version(&record).await.unwrap(), 1);
}

async fn check_loader_enforces_chain(store: &impl RecordLogStore) {
    // The store accepts any entry with the right seq — including one
    // minted for another record (wrong genesis, wrong prev). The
    // loader is where that dies.
    let record = RecordId::new("r1");
    store
        .append(&record, &log_of("r1", 1).entries()[0])
        .await
        .unwrap();
    let foreign = log_of("other", 2).entries()[1].clone();
    store.append(&record, &foreign).await.unwrap();

    let err = load_log(store, &record).await.unwrap_err();
    assert!(matches!(err, LoadError::Chain { record: r, .. } if r == record));
}

async fn check_record_enumeration(store: &impl RecordLogStore) {
    for name in ["r1", "r2", "r3", "r4", "r5"] {
        let record = RecordId::new(name);
        store
            .append(&record, &log_of(name, 1).entries()[0])
            .await
            .unwrap();
    }
    let page = store.records(None, 2).await.unwrap();
    assert_eq!(page, vec![RecordId::new("r1"), RecordId::new("r2")]);
    let rest = store.records(Some(&RecordId::new("r2")), 10).await.unwrap();
    assert_eq!(
        rest,
        vec![
            RecordId::new("r3"),
            RecordId::new("r4"),
            RecordId::new("r5")
        ]
    );
    assert!(
        store
            .records(Some(&RecordId::new("r5")), 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn log_roundtrip() {
    check_log_roundtrip(&MemoryStore::new()).await;
}

#[tokio::test]
async fn log_seq_conflict() {
    check_log_seq_conflict(&MemoryStore::new()).await;
}

#[tokio::test]
async fn loader_enforces_chain() {
    check_loader_enforces_chain(&MemoryStore::new()).await;
}

#[tokio::test]
async fn record_enumeration() {
    check_record_enumeration(&MemoryStore::new()).await;
}

// ---- revisions ------------------------------------------------------

/// Drive a kernel DAG and mirror every publication into the store,
/// index-conditionally — the write path varve-service will run.
async fn publish_through(
    store: &impl RevisionStore,
    lineage: &LineageId,
    dag: &mut RevisionDag,
    schema: Schema,
    parents: Vec<RevisionId>,
) -> RevisionId {
    let index = dag.publications().len() as u64;
    let id = dag.publish(schema.clone(), parents.clone()).unwrap();
    store
        .append_publication(
            lineage,
            index,
            &Publication {
                revision: id.clone(),
                parents,
            },
            &schema,
        )
        .await
        .unwrap();
    id
}

async fn check_revision_roundtrip(store: &impl RevisionStore) {
    let lineage = LineageId::new("proc-1");
    let mut dag = RevisionDag::new();
    let a = publish_through(store, &lineage, &mut dag, schema(&["name"]), vec![]).await;
    let b = publish_through(
        store,
        &lineage,
        &mut dag,
        schema(&["name", "city"]),
        vec![a.clone()],
    )
    .await;
    // A revert: same object, new event (§2.1).
    let again = publish_through(
        store,
        &lineage,
        &mut dag,
        schema(&["name"]),
        vec![b.clone()],
    )
    .await;
    assert_eq!(again, a);

    let reloaded = load_dag(store, &lineage).await.unwrap();
    assert_eq!(reloaded.latest(), Some(&a));
    assert_eq!(reloaded.publications(), dag.publications());
    assert!(reloaded.get(&b).is_some());

    // Point lookup: the reading-lens fetch.
    assert_eq!(store.schema(&a).await.unwrap().unwrap(), schema(&["name"]));
    assert!(
        store
            .schema(&RevisionId::new("nope"))
            .await
            .unwrap()
            .is_none()
    );

    // A second lineage converging on the same schema: same object id,
    // separate event logs.
    let other = LineageId::new("proc-2");
    let mut dag2 = RevisionDag::new();
    let a2 = publish_through(store, &other, &mut dag2, schema(&["name"]), vec![]).await;
    assert_eq!(a2, a);
    assert_eq!(
        load_dag(store, &other).await.unwrap().publications().len(),
        1
    );
    assert_eq!(
        load_dag(store, &lineage)
            .await
            .unwrap()
            .publications()
            .len(),
        3
    );
}

async fn check_publication_conflict(store: &impl RevisionStore) {
    let lineage = LineageId::new("proc-1");
    let s = schema(&["name"]);
    let publication = Publication {
        revision: revision_id(&s),
        parents: vec![],
    };
    store
        .append_publication(&lineage, 0, &publication, &s)
        .await
        .unwrap();
    let err = store
        .append_publication(&lineage, 0, &publication, &s)
        .await
        .unwrap_err();
    assert_eq!(
        err,
        StoreError::PublicationConflict {
            lineage: lineage.clone(),
            next: 1,
            got: 0,
        }
    );
}

async fn check_loader_enforces_revision_ids(store: &impl RevisionStore) {
    // An event naming an id its stored schema does not hash to: the
    // store accepts (index rule only), the loader recomputes and dies.
    let lineage = LineageId::new("proc-1");
    store
        .append_publication(
            &lineage,
            0,
            &Publication {
                revision: RevisionId::new("forged"),
                parents: vec![],
            },
            &schema(&["name"]),
        )
        .await
        .unwrap();
    let err = load_dag(store, &lineage).await.unwrap_err();
    assert!(matches!(
        err,
        LoadError::RevisionIdMismatch { index: 0, .. }
    ));
}

#[tokio::test]
async fn revision_roundtrip() {
    check_revision_roundtrip(&MemoryStore::new()).await;
}

#[tokio::test]
async fn publication_conflict() {
    check_publication_conflict(&MemoryStore::new()).await;
}

#[tokio::test]
async fn loader_enforces_revision_ids() {
    check_loader_enforces_revision_ids(&MemoryStore::new()).await;
}

// ---- blocks ---------------------------------------------------------

async fn check_block_roundtrip(store: &impl BlockStore) {
    store
        .append_block(&block("address", 1, "adr"))
        .await
        .unwrap();
    store
        .append_block(&block("address", 2, "adr"))
        .await
        .unwrap();

    // Version numbering is the store's index rule.
    let err = store
        .append_block(&block("address", 4, "adr"))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        StoreError::BlockVersionConflict {
            id: BlockId::new("address"),
            next: 3,
            got: 4,
        }
    );

    let registry = load_blocks(store, DepthPolicy::default()).await.unwrap();
    assert_eq!(
        registry.latest(&BlockId::new("address")).unwrap().version,
        2
    );
    assert!(registry.get(&BlockId::new("address"), 1).is_some());
}

async fn check_loader_enforces_block_shell(store: &impl BlockStore) {
    store
        .append_block(&block("address", 1, "adr"))
        .await
        .unwrap();
    // A shell-id change between versions: content, so the store takes
    // it; the registry replay refuses it (§2.1 — the shell id is what
    // every inclusion uses).
    store
        .append_block(&block("address", 2, "other"))
        .await
        .unwrap();
    let err = load_blocks(store, DepthPolicy::default())
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        LoadError::Block(PublishBlockError::ShellIdChanged { .. })
    ));
}

async fn check_block_defaults(store: &impl BlockStore) {
    let defaults = BlockDefaults {
        block: BlockRef {
            id: BlockId::new("address"),
            version: 1,
        },
        node: GroupNode {
            group: GroupId::new("adr"),
            prompt: Some("Votre adresse".into()),
            visibility: None,
            children: vec![],
        },
    };
    store.put_block_defaults(&defaults).await.unwrap();
    let err = store.put_block_defaults(&defaults).await.unwrap_err();
    assert_eq!(
        err,
        StoreError::DefaultsExist {
            block: BlockId::new("address"),
            version: 1,
        }
    );
    assert_eq!(
        store
            .block_defaults(&BlockId::new("address"), 1)
            .await
            .unwrap()
            .unwrap(),
        defaults
    );
    assert!(
        store
            .block_defaults(&BlockId::new("address"), 2)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn block_roundtrip() {
    check_block_roundtrip(&MemoryStore::new()).await;
}

#[tokio::test]
async fn loader_enforces_block_shell() {
    check_loader_enforces_block_shell(&MemoryStore::new()).await;
}

#[tokio::test]
async fn block_defaults() {
    check_block_defaults(&MemoryStore::new()).await;
}

// ---- nomenclatures --------------------------------------------------

async fn check_nomenclature_roundtrip(store: &impl NomenclatureStore) {
    let id = NomenclatureId::new("pays");
    store
        .append_nomenclature(&id, 1, &[row("fr"), row("de")])
        .await
        .unwrap();
    store
        .append_nomenclature(&id, 2, &[row("fr"), row("de"), row("it")])
        .await
        .unwrap();
    let err = store
        .append_nomenclature(&id, 2, &[row("fr")])
        .await
        .unwrap_err();
    assert_eq!(
        err,
        StoreError::NomenclatureVersionConflict {
            id: id.clone(),
            next: 3,
            got: 2,
        }
    );
    let registry = load_nomenclatures(store).await.unwrap();
    assert_eq!(registry.rows(&id, 2).unwrap().len(), 3);
}

async fn check_loader_enforces_append_only(store: &impl NomenclatureStore) {
    let id = NomenclatureId::new("pays");
    store
        .append_nomenclature(&id, 1, &[row("fr"), row("de")])
        .await
        .unwrap();
    // Version 2 drops an id: content (§2.11), caught on replay.
    store
        .append_nomenclature(&id, 2, &[row("fr")])
        .await
        .unwrap();
    let err = load_nomenclatures(store).await.unwrap_err();
    assert!(matches!(
        err,
        LoadError::Nomenclature(PublishNomenclatureError::RemovesIds { .. })
    ));
}

#[tokio::test]
async fn nomenclature_roundtrip() {
    check_nomenclature_roundtrip(&MemoryStore::new()).await;
}

#[tokio::test]
async fn loader_enforces_append_only() {
    check_loader_enforces_append_only(&MemoryStore::new()).await;
}

// ---- surfaces -------------------------------------------------------

async fn check_surfaces(store: &impl SurfaceStore) {
    store.put_surface(&surface("rev-1", "form")).await.unwrap();
    store
        .put_surface(&surface("rev-1", "review"))
        .await
        .unwrap();
    store.put_surface(&surface("rev-2", "form")).await.unwrap();

    let of_rev1 = store.surfaces(&RevisionId::new("rev-1")).await.unwrap();
    assert_eq!(
        of_rev1.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec!["form", "review"]
    );

    // Upsert: re-authoring replaces in place.
    let mut edited = surface("rev-1", "form");
    edited.nodes = vec![varve_surface::Node::Note(varve_surface::Note {
        title: None,
        body: "bienvenue".into(),
    })];
    store.put_surface(&edited).await.unwrap();
    assert_eq!(
        store
            .surface(&RevisionId::new("rev-1"), &SurfaceId::new("form"))
            .await
            .unwrap()
            .unwrap(),
        edited
    );
    assert!(
        store
            .surface(&RevisionId::new("rev-3"), &SurfaceId::new("form"))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn surfaces() {
    check_surfaces(&MemoryStore::new()).await;
}
