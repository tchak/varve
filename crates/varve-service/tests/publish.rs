//! The publish operation against the `MemoryStore` reference
//! implementation (§13.2's oracle): the gate, the fork-point check,
//! the surface writes, and the one-code-path first publication.

use varve_core::{ColumnId, RevisionId, SurfaceId};
use varve_impact::ChangeClass;
use varve_schema::{Arity, Column, Element, ScalarType, Schema, revision_id};
use varve_service::{PublishOutcome, PublishRevision, PublishRevisionError, publish_revision};
use varve_store::load::load_dag;
use varve_store::{LineageId, MemoryStore, SurfaceStore};
use varve_surface::{ColumnNode, Node, Surface, WritePolicy};

fn column(id: &str, ty: ScalarType) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.to_string(),
        ty,
        arity: Arity::One,
    })
}

fn schema(columns: Vec<Element>) -> Schema {
    Schema {
        root: columns,
        resolvers: vec![],
    }
}

/// The two fixed surfaces of platform P.4, minimal: every column a
/// plain writable node.
fn surfaces(schema: &Schema) -> Vec<Surface> {
    let revision = revision_id(schema);
    ["applicant", "reviewer"]
        .into_iter()
        .map(|id| Surface {
            id: SurfaceId::new(id),
            revision: revision.clone(),
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
        })
        .collect()
}

fn request(
    lineage: &LineageId,
    base: Option<RevisionId>,
    schema: Schema,
    confirm: bool,
) -> PublishRevision {
    let surfaces = surfaces(&schema);
    PublishRevision {
        lineage: lineage.clone(),
        base,
        schema,
        surfaces,
        confirm,
    }
}

#[tokio::test]
async fn first_publication_is_free_and_stores_the_pair() {
    let store = MemoryStore::default();
    let lineage = LineageId::new("proc-1");
    let v1 = schema(vec![column("name", ScalarType::Text)]);

    let outcome = publish_revision(&store, request(&lineage, None, v1.clone(), false))
        .await
        .expect("publish");
    let PublishOutcome::Published { revision, report } = outcome else {
        panic!("first publication must not require confirmation");
    };
    assert_eq!(revision, revision_id(&v1));
    assert_eq!(report.worst(), ChangeClass::Safe);

    // The DAG loads back verified, and both surfaces are stored
    // against the revision.
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.latest(), Some(&revision));
    let stored = store.surfaces(&revision).await.expect("surfaces");
    assert_eq!(stored.len(), 2);
}

#[tokio::test]
async fn a_breaking_change_gates_on_confirmation() {
    let store = MemoryStore::default();
    let lineage = LineageId::new("proc-2");
    let v1 = schema(vec![column("a", ScalarType::Text)]);
    let v2 = schema(vec![column("a", ScalarType::Geometry)]);

    let base = match publish_revision(&store, request(&lineage, None, v1, false))
        .await
        .expect("v1")
    {
        PublishOutcome::Published { revision, .. } => revision,
        other => panic!("unexpected: {other:?}"),
    };

    // Without confirm: the report comes back, nothing is written.
    let outcome = publish_revision(
        &store,
        request(&lineage, Some(base.clone()), v2.clone(), false),
    )
    .await
    .expect("gate");
    let PublishOutcome::RequiresConfirmation { report } = outcome else {
        panic!("a retype with no cast must gate");
    };
    assert_eq!(report.worst(), ChangeClass::Breaking);
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.publications().len(), 1);

    // With confirm: published.
    let outcome = publish_revision(&store, request(&lineage, Some(base), v2.clone(), true))
        .await
        .expect("confirmed");
    let PublishOutcome::Published { revision, report } = outcome else {
        panic!("confirmed publication must publish");
    };
    assert_eq!(report.worst(), ChangeClass::Breaking);
    assert_eq!(revision, revision_id(&v2));
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.publications().len(), 2);
    assert_eq!(dag.latest(), Some(&revision));
}

#[tokio::test]
async fn a_stale_base_is_refused() {
    let store = MemoryStore::default();
    let lineage = LineageId::new("proc-3");
    let v1 = schema(vec![column("a", ScalarType::Text)]);

    let head = match publish_revision(&store, request(&lineage, None, v1.clone(), false))
        .await
        .expect("v1")
    {
        PublishOutcome::Published { revision, .. } => revision,
        other => panic!("unexpected: {other:?}"),
    };

    // Claiming an empty lineage when it is not.
    let v2 = schema(vec![
        column("a", ScalarType::Text),
        column("b", ScalarType::Text),
    ]);
    let err = publish_revision(&store, request(&lineage, None, v2.clone(), false))
        .await
        .expect_err("stale");
    match err {
        PublishRevisionError::StaleBase { base, head: h } => {
            assert_eq!(base, None);
            assert_eq!(h, Some(head.clone()));
        }
        other => panic!("unexpected: {other}"),
    }

    // Claiming a base that is no longer the head.
    let head2 = match publish_revision(&store, request(&lineage, Some(head.clone()), v2, false))
        .await
        .expect("v2")
    {
        PublishOutcome::Published { revision, .. } => revision,
        other => panic!("unexpected: {other:?}"),
    };
    let v3 = schema(vec![column("c", ScalarType::Text)]);
    let err = publish_revision(&store, request(&lineage, Some(head), v3, true))
        .await
        .expect_err("stale");
    assert!(matches!(
        err,
        PublishRevisionError::StaleBase { head: Some(h), .. } if h == head2
    ));
}

#[tokio::test]
async fn republishing_an_identical_schema_converges_on_the_object() {
    let store = MemoryStore::default();
    let lineage = LineageId::new("proc-4");
    let v1 = schema(vec![column("a", ScalarType::Text)]);
    let v2 = schema(vec![
        column("a", ScalarType::Text),
        column("b", ScalarType::Text),
    ]);

    let r1 = match publish_revision(&store, request(&lineage, None, v1.clone(), false))
        .await
        .expect("v1")
    {
        PublishOutcome::Published { revision, .. } => revision,
        other => panic!("unexpected: {other:?}"),
    };
    let r2 = match publish_revision(&store, request(&lineage, Some(r1.clone()), v2, false))
        .await
        .expect("v2")
    {
        PublishOutcome::Published { revision, .. } => revision,
        other => panic!("unexpected: {other:?}"),
    };

    // Publishing v1's schema again is a revert: a third event, no new
    // object, the id converging (§2.13).
    let outcome = publish_revision(&store, request(&lineage, Some(r2), v1, true))
        .await
        .expect("revert");
    let PublishOutcome::Published { revision, .. } = outcome else {
        panic!("revert publishes");
    };
    assert_eq!(revision, r1);
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.publications().len(), 3);
    assert_eq!(dag.latest(), Some(&r1));
}

#[tokio::test]
async fn a_surface_for_the_wrong_revision_is_a_host_bug() {
    let store = MemoryStore::default();
    let lineage = LineageId::new("proc-5");
    let v1 = schema(vec![column("a", ScalarType::Text)]);
    let mut request = request(&lineage, None, v1, false);
    request.surfaces[0].revision = RevisionId::new("not-this-schema");

    let err = publish_revision(&store, request)
        .await
        .expect_err("refused");
    assert!(matches!(err, PublishRevisionError::Surface(_)), "{err}");
}
