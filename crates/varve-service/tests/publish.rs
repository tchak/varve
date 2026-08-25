//! The publish operation against the `MemoryStore` reference
//! implementation (§13.2's oracle): the gate, the fork-point check,
//! the surface writes, and the one-code-path first publication.

use varve_core::{ColumnId, PublicationId, RevisionId, SurfaceId};
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
    base: Option<PublicationId>,
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
    let PublishOutcome::Published {
        publication,
        revision,
        report,
        surface_report,
    } = outcome
    else {
        panic!("first publication must not require confirmation");
    };
    // One code path (§3.1 like §3): against the empty set, the first
    // publication's surface report is the initial surface list.
    assert_eq!(surface_report.worst(), ChangeClass::Safe);
    assert_eq!(surface_report.changes.len(), 2);
    assert!(
        surface_report
            .changes
            .iter()
            .all(|c| matches!(c.kind, varve_surface::SurfaceChangeKind::SurfaceAdded))
    );
    assert_eq!(revision, revision_id(&v1));
    assert_eq!(report.worst(), ChangeClass::Safe);

    // The DAG loads back verified; the publication commits to both
    // surfaces (§2.13 decision 9) and each hash resolves in the store.
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.latest(), Some(&publication));
    let event = dag.publication(&publication).expect("event");
    assert_eq!(event.revision, revision);
    assert_eq!(event.surfaces.len(), 2);
    for (id, hash) in &event.surfaces {
        let stored = store.surface(hash).await.expect("get").expect("stored");
        assert_eq!(&stored.id, id);
        assert_eq!(stored.revision, revision);
    }
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
        PublishOutcome::Published { publication, .. } => publication,
        other => panic!("unexpected: {other:?}"),
    };

    // Without confirm: the report comes back, nothing is written.
    let outcome = publish_revision(
        &store,
        request(&lineage, Some(base.clone()), v2.clone(), false),
    )
    .await
    .expect("gate");
    let PublishOutcome::RequiresConfirmation { report, .. } = outcome else {
        panic!("a retype with no cast must gate");
    };
    assert_eq!(report.worst(), ChangeClass::Breaking);
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.publications().len(), 1);

    // With confirm: published.
    let outcome = publish_revision(&store, request(&lineage, Some(base), v2.clone(), true))
        .await
        .expect("confirmed");
    let PublishOutcome::Published {
        publication,
        revision,
        report,
        ..
    } = outcome
    else {
        panic!("confirmed publication must publish");
    };
    assert_eq!(report.worst(), ChangeClass::Breaking);
    assert_eq!(revision, revision_id(&v2));
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.publications().len(), 2);
    assert_eq!(dag.latest(), Some(&publication));
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
        PublishOutcome::Published { publication, .. } => publication,
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
        PublishOutcome::Published { publication, .. } => publication,
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

    let p1 = match publish_revision(&store, request(&lineage, None, v1.clone(), false))
        .await
        .expect("v1")
    {
        PublishOutcome::Published { publication, .. } => publication,
        other => panic!("unexpected: {other:?}"),
    };
    let p2 = match publish_revision(&store, request(&lineage, Some(p1.clone()), v2, false))
        .await
        .expect("v2")
    {
        PublishOutcome::Published { publication, .. } => publication,
        other => panic!("unexpected: {other:?}"),
    };

    // Publishing v1's schema again is a revert: a third event, no new
    // object, the revision converging (§2.13) under a fresh
    // publication id (decision 9 — its parents differ).
    let outcome = publish_revision(&store, request(&lineage, Some(p2), v1.clone(), true))
        .await
        .expect("revert");
    let PublishOutcome::Published {
        publication,
        revision,
        ..
    } = outcome
    else {
        panic!("revert publishes");
    };
    assert_ne!(publication, p1);
    assert_eq!(revision, revision_id(&v1));
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.publications().len(), 3);
    assert_eq!(dag.latest(), Some(&publication));
}

/// §2.13 decision 9: same revision, same surface set as the head —
/// refused before the gate, as "nothing to publish", not "safe".
#[tokio::test]
async fn an_identical_publication_is_a_refused_noop() {
    let store = MemoryStore::default();
    let lineage = LineageId::new("proc-6");
    let v1 = schema(vec![column("a", ScalarType::Text)]);

    let head = match publish_revision(&store, request(&lineage, None, v1.clone(), false))
        .await
        .expect("v1")
    {
        PublishOutcome::Published { publication, .. } => publication,
        other => panic!("unexpected: {other:?}"),
    };
    let err = publish_revision(&store, request(&lineage, Some(head), v1, false))
        .await
        .expect_err("noop");
    assert!(matches!(err, PublishRevisionError::NothingToPublish));
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.publications().len(), 1);
}

/// A surface-only change — the schema untouched — is a real
/// publication (§2.13 decision 9): the empty-diff bug this resolves
/// was a section rename vanishing from history.
#[tokio::test]
async fn a_surface_only_change_publishes() {
    let store = MemoryStore::default();
    let lineage = LineageId::new("proc-7");
    let v1 = schema(vec![column("a", ScalarType::Text)]);

    let p1 = match publish_revision(&store, request(&lineage, None, v1.clone(), false))
        .await
        .expect("v1")
    {
        PublishOutcome::Published { publication, .. } => publication,
        other => panic!("unexpected: {other:?}"),
    };
    let mut renamed = request(&lineage, Some(p1.clone()), v1.clone(), false);
    let Node::Column(node) = &mut renamed.surfaces[0].nodes[0] else {
        panic!("column node");
    };
    node.prompt = Some("Votre nom".into());
    let outcome = publish_revision(&store, renamed).await.expect("publish");
    let PublishOutcome::Published {
        publication,
        revision,
        report,
        surface_report,
    } = outcome
    else {
        panic!("a surface-only change publishes");
    };
    // §3.1: the schema half is empty, the surface half carries the
    // prompt change — safe, so no confirmation was demanded.
    assert_eq!(report.worst(), ChangeClass::Safe);
    assert_eq!(surface_report.worst(), ChangeClass::Safe);
    assert!(
        surface_report.changes.iter().any(|c| matches!(
            &c.kind,
            varve_surface::SurfaceChangeKind::PromptChanged { column } if column.as_str() == "a"
        )),
        "{surface_report:?}"
    );
    assert_ne!(publication, p1);
    assert_eq!(revision, revision_id(&v1));
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.publications().len(), 2);
    // Both surface sets survive — nothing was overwritten.
    let (first, second) = (
        dag.publication(&p1).expect("p1").surfaces.clone(),
        dag.publication(&publication).expect("p2").surfaces.clone(),
    );
    assert_ne!(first, second);
    for hash in first.values().chain(second.values()) {
        assert!(store.surface(hash).await.expect("get").is_some());
    }
}

/// Two surfaces sharing an id is a host bug (the surface set is a
/// map), refused before anything is written.
#[tokio::test]
async fn duplicate_surface_ids_are_refused() {
    let store = MemoryStore::default();
    let lineage = LineageId::new("proc-8");
    let v1 = schema(vec![column("a", ScalarType::Text)]);
    let mut req = request(&lineage, None, v1, false);
    let clone = req.surfaces[0].clone();
    req.surfaces.push(clone);
    let err = publish_revision(&store, req).await.expect_err("refused");
    assert!(matches!(
        err,
        PublishRevisionError::DuplicateSurface(id) if id == SurfaceId::new("applicant")
    ));
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

/// §3.1: the gate takes the worst class of the report pair — a
/// requiredness tightening (schema untouched, all-`Identical`)
/// demands the same confirmation a lossy cast does.
#[tokio::test]
async fn a_requiredness_tightening_gates_on_confirmation() {
    let store = MemoryStore::default();
    let lineage = LineageId::new("proc-9");
    let v1 = schema(vec![column("a", ScalarType::Text)]);

    let p1 = match publish_revision(&store, request(&lineage, None, v1.clone(), false))
        .await
        .expect("v1")
    {
        PublishOutcome::Published { publication, .. } => publication,
        other => panic!("unexpected: {other:?}"),
    };

    let tighten = |base: Option<PublicationId>, confirm: bool| {
        let mut req = request(&lineage, base, v1.clone(), confirm);
        for surface in &mut req.surfaces {
            let Node::Column(node) = &mut surface.nodes[0] else {
                panic!("column node");
            };
            node.required = Some(varve_logic::Expr::And(vec![]));
        }
        req
    };

    // Without confirm: the pair comes back, nothing is written.
    let outcome = publish_revision(&store, tighten(Some(p1.clone()), false))
        .await
        .expect("gate");
    let PublishOutcome::RequiresConfirmation {
        report,
        surface_report,
    } = outcome
    else {
        panic!("a tightening must gate");
    };
    assert_eq!(report.worst(), ChangeClass::Safe);
    assert_eq!(surface_report.worst(), ChangeClass::Checked);
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.publications().len(), 1);

    // With confirm: published.
    let outcome = publish_revision(&store, tighten(Some(p1), true))
        .await
        .expect("confirmed");
    assert!(matches!(outcome, PublishOutcome::Published { .. }));
    let dag = load_dag(&store, &lineage).await.expect("load");
    assert_eq!(dag.publications().len(), 2);
}
