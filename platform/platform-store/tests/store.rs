//! The store contract against Postgres (the varve-store/tests/store.rs
//! points that RevisionStore + SurfaceStore own), DB-gated on
//! `VARVE_TEST_DATABASE_URL` like every platform DB suite. Each test
//! scopes the store over its own transaction (the P.3 shape) and
//! mints unique lineages — the database is shared.

use toasty::Executor;
use tokio::sync::Mutex;
use varve_core::{ColumnId, RevisionId, SurfaceId};
use varve_revision::Publication;
use varve_schema::{Arity, Column, Element, ScalarType, Schema, revision_id};
use varve_store::load::load_dag;
use varve_store::{LineageId, RevisionStore, StoreError, SurfaceStore};
use varve_surface::{ColumnNode, Node, Surface, WritePolicy};

use platform_store::PlatformStore;

async fn test_db() -> Option<toasty::Db> {
    let url = match std::env::var("VARVE_TEST_DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("skipped: VARVE_TEST_DATABASE_URL not set");
            return None;
        }
    };
    Some(
        platform_core::connect_with(
            &url,
            platform_store::models(),
            &[&platform_store::MIGRATIONS],
        )
        .await
        .expect("connect to test database"),
    )
}

fn lineage(tag: &str) -> LineageId {
    LineageId::new(format!("{tag}-{}", uuid_like()))
}

/// Unique-enough without a uuid dependency: nanos since the epoch
/// plus the test's address-space entropy is overkill already, but
/// lineages are strings — anything unique works.
fn uuid_like() -> String {
    format!("{:x}", std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos())
}

fn schema(ids: &[&str]) -> Schema {
    Schema {
        root: ids
            .iter()
            .map(|id| {
                Element::Column(Column {
                    id: ColumnId::new(*id),
                    label: id.to_string(),
                    ty: ScalarType::Text,
                    arity: Arity::One,
                })
            })
            .collect(),
        resolvers: vec![],
    }
}

fn surface(id: &str, schema: &Schema) -> Surface {
    Surface {
        id: SurfaceId::new(id),
        revision: revision_id(schema),
        nodes: schema
            .root
            .iter()
            .map(|element| match element {
                Element::Column(c) => Node::Column(ColumnNode {
                    column: c.id.clone(),
                    prompt: None,
                    help: None,
                    visibility: None,
                    required: None,
                    write: WritePolicy::default(),
                    format: None,
                }),
                _ => unreachable!("tests use flat schemas"),
            })
            .collect(),
        ineligibility: None,
    }
}

fn publication(schema: &Schema, parents: &[&Publication]) -> Publication {
    Publication {
        revision: revision_id(schema),
        parents: parents.iter().map(|p| p.id()).collect(),
        surfaces: std::collections::BTreeMap::new(),
    }
}

#[tokio::test]
async fn publications_append_load_and_verify() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let mut tx = db.transaction().await.unwrap();
    let shared = Mutex::new(&mut tx as &mut dyn Executor);
    let store = PlatformStore::new(&shared);

    let line = lineage("dag");
    let v1 = schema(&["a"]);
    let v2 = schema(&["a", "b"]);
    let r1 = revision_id(&v1);

    let p1 = publication(&v1, &[]);
    let p2 = publication(&v2, &[&p1]);
    store
        .append_publication(&line, 0, &p1, &v1)
        .await
        .expect("first");
    store
        .append_publication(&line, 1, &p2, &v2)
        .await
        .expect("second");

    // The loader replays through the kernel constructors and
    // recomputes every content address.
    let dag = load_dag(&store, &line).await.expect("load");
    assert_eq!(dag.publications().len(), 2);
    assert_eq!(dag.latest(), Some(&p2.id()));

    // Point lookup serves the reading lens.
    let fetched = store.schema(&r1).await.expect("schema");
    assert_eq!(fetched, Some(v1));
    assert_eq!(
        store
            .schema(&RevisionId::new("absent"))
            .await
            .expect("none"),
        None
    );
}

#[tokio::test]
async fn the_index_rule_detects_lost_races() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let mut tx = db.transaction().await.unwrap();
    let shared = Mutex::new(&mut tx as &mut dyn Executor);
    let store = PlatformStore::new(&shared);

    let line = lineage("conflict");
    let v1 = schema(&["a"]);
    store
        .append_publication(&line, 0, &publication(&v1, &[]), &v1)
        .await
        .expect("first");

    // Appending #0 again — the caller has not seen the head.
    let err = store
        .append_publication(
            &line,
            0,
            &publication(&schema(&["b"]), &[]),
            &schema(&["b"]),
        )
        .await
        .expect_err("conflict");
    assert!(
        matches!(
            err,
            StoreError::PublicationConflict {
                next: 1,
                got: 0,
                ..
            }
        ),
        "{err}"
    );

    // Skipping ahead is refused the same way.
    let err = store
        .append_publication(
            &line,
            5,
            &publication(&schema(&["b"]), &[]),
            &schema(&["b"]),
        )
        .await
        .expect_err("gap");
    assert!(
        matches!(
            err,
            StoreError::PublicationConflict {
                next: 1,
                got: 5,
                ..
            }
        ),
        "{err}"
    );
}

#[tokio::test]
async fn revision_objects_converge_on_the_content_address() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let mut tx = db.transaction().await.unwrap();
    let shared = Mutex::new(&mut tx as &mut dyn Executor);
    let store = PlatformStore::new(&shared);

    // Two lineages publish the identical schema (the corpus's 19.7%
    // dedup, §2.13) — and a revert republishes an existing object.
    let line_a = lineage("conv-a");
    let line_b = lineage("conv-b");
    let v1 = schema(&["same"]);

    let p1 = publication(&v1, &[]);
    store
        .append_publication(&line_a, 0, &p1, &v1)
        .await
        .expect("a");
    store
        .append_publication(&line_b, 0, &p1, &v1)
        .await
        .expect("b");
    let v2 = schema(&["same", "more"]);
    let p2 = publication(&v2, &[&p1]);
    store
        .append_publication(&line_a, 1, &p2, &v2)
        .await
        .expect("a v2");
    // A revert: the same object again, a fresh event (§2.13 dec. 9).
    let p3 = publication(&v1, &[&p2]);
    store
        .append_publication(&line_a, 2, &p3, &v1)
        .await
        .expect("revert");

    let dag = load_dag(&store, &line_a).await.expect("load");
    assert_eq!(dag.publications().len(), 3);
    assert_eq!(dag.latest(), Some(&p3.id()));
    assert_eq!(
        dag.publication(&p3.id()).expect("event").revision,
        revision_id(&v1)
    );
}

#[tokio::test]
async fn surfaces_are_content_addressed_and_immutable() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let mut tx = db.transaction().await.unwrap();
    let shared = Mutex::new(&mut tx as &mut dyn Executor);
    let store = PlatformStore::new(&shared);

    // Surfaces reference their revision by FK: the object first.
    let v1 = schema(&["a"]);
    let line = lineage("surf");
    store
        .append_publication(&line, 0, &publication(&v1, &[]), &v1)
        .await
        .expect("publish");

    // §2.13 decision 9: keyed by content hash, idempotent, and a
    // re-authored surface lands beside — never over — the stored one.
    let reviewer = surface("reviewer", &v1);
    let h1 = store.put_surface(&reviewer).await.expect("reviewer");
    assert_eq!(h1, reviewer.content_hash());
    assert_eq!(store.put_surface(&reviewer).await.expect("again"), h1);

    let mut reauthored = reviewer.clone();
    reauthored.nodes.clear();
    let h2 = store.put_surface(&reauthored).await.expect("reauthored");
    assert_ne!(h2, h1);
    assert_eq!(
        store.surface(&h1).await.expect("get").expect("kept"),
        reviewer
    );
    assert_eq!(
        store.surface(&h2).await.expect("get").expect("stored"),
        reauthored
    );
    assert_eq!(
        store
            .surface(&surface("applicant", &v1).content_hash())
            .await
            .expect("none"),
        None
    );
}

#[tokio::test]
async fn an_uncommitted_transaction_leaves_nothing() {
    let Some(mut db) = test_db().await else {
        return;
    };
    let line = lineage("rollback");
    let v1 = schema(&["a"]);
    {
        let mut tx = db.transaction().await.unwrap();
        let shared = Mutex::new(&mut tx as &mut dyn Executor);
        let store = PlatformStore::new(&shared);
        store
            .append_publication(&line, 0, &publication(&v1, &[]), &v1)
            .await
            .expect("append");
        // Dropped without commit: automatic rollback.
    }
    let mut tx = db.transaction().await.unwrap();
    let shared = Mutex::new(&mut tx as &mut dyn Executor);
    let store = PlatformStore::new(&shared);
    let dag = load_dag(&store, &line).await.expect("load");
    assert_eq!(dag.publications().len(), 0);
}
