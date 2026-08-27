//! The store contract, executable (§13.2): generic checks over the
//! trait bounds — any implementation must pass them — run here against
//! [`MemoryStore`]. The recurring shape: the store enforces **index
//! rules only**; content invariants re-run in the loaders, so a
//! tampered row is accepted at write and caught at the first load.

use varve_core::{BlockId, ColumnId, GroupId, NomenclatureId, OptionId, RevisionId, SurfaceId};
use varve_revision::{Publication, PublishBlockError, PublishNomenclatureError, RevisionDag};
use varve_schema::{
    Arity, Block, BlockRef, Cardinality, Column, DepthPolicy, Element, Group, OptionRow,
    ScalarType, Schema, revision_id,
};
use varve_store::contract;
use varve_store::load::{LoadError, load_blocks, load_dag, load_nomenclatures};
use varve_store::{
    BlockStore, LineageId, MemoryStore, NomenclatureStore, RevisionStore, StoreError, SurfaceStore,
};
use varve_surface::{BlockDefaults, GroupNode, Surface};

// ---- fixtures -------------------------------------------------------

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
// The checks live in `varve_store::contract` (the executable store
// contract, platform P.8) so the platform's Toasty implementation
// runs the same ones; here they run against the reference store.

#[tokio::test]
async fn log_roundtrip() {
    contract::check_log_roundtrip(&MemoryStore::new()).await;
}

#[tokio::test]
async fn log_seq_conflict() {
    contract::check_log_seq_conflict(&MemoryStore::new()).await;
}

#[tokio::test]
async fn loader_enforces_chain() {
    contract::check_loader_enforces_chain(&MemoryStore::new()).await;
}

#[tokio::test]
async fn record_enumeration() {
    contract::check_record_enumeration(&MemoryStore::new()).await;
}

// ---- revisions ------------------------------------------------------

/// Drive a kernel DAG and mirror every publication into the store,
/// index-conditionally — the write path varve-service will run.
async fn publish_through(
    store: &impl RevisionStore,
    lineage: &LineageId,
    dag: &mut RevisionDag,
    schema: Schema,
    parents: Vec<varve_core::PublicationId>,
) -> varve_core::PublicationId {
    let index = dag.publications().len() as u64;
    let id = dag
        .publish(schema.clone(), std::collections::BTreeMap::new(), parents)
        .unwrap();
    let (_, publication) = dag.publications().last().unwrap().clone();
    store
        .append_publication(lineage, index, &publication, &schema)
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
    // A revert: same object, new event under a fresh publication id
    // (§2.1, §2.13 decision 9 — its parents differ).
    let again = publish_through(
        store,
        &lineage,
        &mut dag,
        schema(&["name"]),
        vec![b.clone()],
    )
    .await;
    assert_ne!(again, a);

    let reloaded = load_dag(store, &lineage).await.unwrap();
    assert_eq!(reloaded.latest(), Some(&again));
    assert_eq!(reloaded.publications(), dag.publications());
    let rev_b = reloaded.publication(&b).unwrap().revision.clone();
    assert!(reloaded.get(&rev_b).is_some());
    assert_eq!(
        reloaded.publication(&again).unwrap().revision,
        revision_id(&schema(&["name"]))
    );

    // Point lookup: the reading-lens fetch.
    assert_eq!(
        store
            .schema(&revision_id(&schema(&["name"])))
            .await
            .unwrap()
            .unwrap(),
        schema(&["name"])
    );
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
    // Identical content — schema, no parents, no surfaces — converges
    // on the same publication id too: content addressing end to end.
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
        surfaces: std::collections::BTreeMap::new(),
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
                surfaces: std::collections::BTreeMap::new(),
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
    // Content-addressed and immutable (§2.13 decision 9): the put
    // returns the hash, an identical re-put is idempotent, distinct
    // content — even for one (revision, id) — lands beside, never
    // over, what is stored.
    let form = surface("rev-1", "form");
    let h1 = store.put_surface(&form).await.unwrap();
    assert_eq!(h1, form.content_hash());
    assert_eq!(store.put_surface(&form).await.unwrap(), h1);

    let mut edited = surface("rev-1", "form");
    edited.nodes = vec![varve_surface::Node::Note(varve_surface::Note {
        id: varve_core::NodeId::new("bienvenue"),
        title: None,
        body: "bienvenue".into(),
    })];
    let h2 = store.put_surface(&edited).await.unwrap();
    assert_ne!(h2, h1);
    assert_eq!(store.surface(&h1).await.unwrap().unwrap(), form);
    assert_eq!(store.surface(&h2).await.unwrap().unwrap(), edited);
    assert!(
        store
            .surface(&surface("rev-1", "review").content_hash())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn surfaces() {
    check_surfaces(&MemoryStore::new()).await;
}
