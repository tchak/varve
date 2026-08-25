//! Publication laws over generated schemas: the DAG is content-addressed
//! (§2.13 — one object per schema, however often it is published), the
//! publication log is history (§2.1 — every event counts, `latest` is the
//! last one), and nomenclature versions are append-only (§2.11).

use std::collections::BTreeMap;

use proptest::prelude::*;
use varve_core::{ColumnId, NomenclatureId, OptionId};
use varve_revision::{NomenclatureRegistry, PublishError, PublishNomenclatureError, RevisionDag};
use varve_schema::{Arity, Column, Element, OptionRow, ScalarType, Schema, revision_id};

fn column(id: &str, ty: ScalarType) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.into(),
        ty,
        arity: Arity::One,
    })
}

/// A handful of distinguishable schemas — small enough that repeats
/// (reverts) come up constantly.
fn schema() -> impl Strategy<Value = Schema> {
    (
        proptest::sample::subsequence(vec!["a", "b", "c"], 0..=3),
        prop_oneof![
            Just(ScalarType::Text),
            Just(ScalarType::Boolean),
            Just(ScalarType::Integer(None))
        ],
    )
        .prop_map(|(ids, ty)| Schema {
            root: ids.into_iter().map(|id| column(id, ty.clone())).collect(),
            resolvers: vec![],
        })
}

fn row(id: &str, label: &str) -> OptionRow {
    OptionRow {
        id: OptionId::new(id),
        label: label.into(),
        fields: vec![],
    }
}

proptest! {
    /// Publishing a sequence of schemas: revisions are content hashes,
    /// publications are content-addressed events (§2.13 decision 9);
    /// the object store holds one object per distinct schema; the log
    /// holds one event per accepted publication; `latest` is the last
    /// event; a publication identical to the head (same revision, same
    /// — here empty — surface set) refuses; publication ids never
    /// collide (their content includes their parents).
    #[test]
    fn publication_is_content_addressed_and_the_log_is_history(
        schemas in proptest::collection::vec(schema(), 1..8),
    ) {
        let mut dag = RevisionDag::new();
        let mut objects = std::collections::BTreeSet::new();
        let mut event_ids = std::collections::BTreeSet::new();
        let mut previous: Option<varve_core::PublicationId> = None;
        let mut accepted = 0usize;
        for s in schemas.iter() {
            let parents: Vec<_> = previous.iter().cloned().collect();
            let revision = revision_id(s);
            let head_revision = dag.head().map(|(_, p)| p.revision.clone());
            let result = dag.publish(s.clone(), BTreeMap::new(), parents.clone());
            if head_revision.as_ref() == Some(&revision) {
                // §2.13 decision 9: identical to the head is a no-op.
                prop_assert_eq!(result, Err(PublishError::IdenticalToHead));
                continue;
            }
            let id = result.unwrap();
            accepted += 1;
            objects.insert(revision.clone());
            prop_assert!(event_ids.insert(id.clone()));
            // The object: same schema.
            prop_assert_eq!(&dag.get(&revision).unwrap().schema, s);
            // The event: exactly as published.
            let (event_id, event) = dag.publications().last().unwrap();
            prop_assert_eq!(dag.publications().len(), accepted);
            prop_assert_eq!(event_id, &id);
            prop_assert_eq!(&event.revision, &revision);
            prop_assert_eq!(&event.parents, &parents);
            prop_assert_eq!(dag.latest(), Some(&id));
            previous = Some(id);
        }
        // One object per distinct accepted schema; one history entry
        // per event, in order, each naming its object.
        let history: Vec<_> = dag.history().map(|(id, _)| id.clone()).collect();
        prop_assert_eq!(history.len(), accepted);
        for (id, s) in dag.history() {
            prop_assert!(objects.contains(id));
            prop_assert_eq!(revision_id(s), id.clone());
        }
    }

    /// An identical republish refuses (§2.13 decision 9); a revert —
    /// the same schema again *after* something else — is a new event
    /// converging on the old object, under a fresh publication id.
    #[test]
    fn identical_refuses_and_a_revert_converges(
        (s, t) in (schema(), schema())
            .prop_filter("distinct schemas", |(s, t)| revision_id(s) != revision_id(t)),
    ) {
        let mut dag = RevisionDag::new();
        let p1 = dag.publish(s.clone(), BTreeMap::new(), vec![]).unwrap();
        prop_assert_eq!(
            dag.publish(s.clone(), BTreeMap::new(), vec![p1.clone()]),
            Err(PublishError::IdenticalToHead)
        );
        let p2 = dag.publish(t.clone(), BTreeMap::new(), vec![p1.clone()]).unwrap();
        let p3 = dag.publish(s.clone(), BTreeMap::new(), vec![p2]).unwrap();
        // A new event, not the first one again — its parents differ.
        prop_assert_ne!(&p3, &p1);
        prop_assert_eq!(dag.latest(), Some(&p3));
        prop_assert_eq!(dag.publications().len(), 3);
        // No new object, and the object keeps its first parents.
        prop_assert_eq!(&dag.publication(&p3).unwrap().revision, &revision_id(&s));
        prop_assert!(dag.get(&revision_id(&s)).unwrap().parents.is_empty());
        // The aggregate over the lineage is over the two objects.
        let (agg, _) = dag.aggregate(&Default::default()).unwrap();
        prop_assert!(agg.columns.len() <= s.root.len() + t.root.len());
    }

    /// Nomenclature versions are append-only (§2.11): a next version is
    /// accepted iff it keeps every id of the previous one; relabels and
    /// additions are free; version numbers are dense; every version
    /// stays readable at its own number.
    #[test]
    fn nomenclature_versions_are_append_only(
        versions in proptest::collection::vec(
            proptest::collection::btree_map(
                prop_oneof![Just("o1"), Just("o2"), Just("o3")],
                prop_oneof![Just("A"), Just("B")],
                0..=3,
            ),
            1..6,
        ),
    ) {
        let id = NomenclatureId::new("n");
        let mut registry = NomenclatureRegistry::new();
        let mut published: Vec<Vec<OptionRow>> = Vec::new();
        for rows in versions {
            let rows: Vec<OptionRow> = rows.into_iter().map(|(i, l)| row(i, l)).collect();
            let removed: Vec<OptionId> = published
                .last()
                .map(|prev| {
                    prev.iter()
                        .map(|r| r.id.clone())
                        .filter(|o| !rows.iter().any(|r| &r.id == o))
                        .collect()
                })
                .unwrap_or_default();
            let result = registry.publish(id.clone(), rows.clone());
            if removed.is_empty() {
                prop_assert_eq!(result, Ok(published.len() as u32 + 1));
                published.push(rows);
            } else {
                prop_assert_eq!(
                    result,
                    Err(PublishNomenclatureError::RemovesIds {
                        id: id.clone(),
                        version: published.len() as u32 + 1,
                        removed,
                    })
                );
            }
            // Every accepted version stays readable, verbatim, at its
            // number; nothing beyond.
            for (i, rows) in published.iter().enumerate() {
                prop_assert_eq!(registry.rows(&id, i as u32 + 1), Some(rows.as_slice()));
            }
            prop_assert_eq!(registry.rows(&id, published.len() as u32 + 1), None);
            prop_assert_eq!(registry.rows(&id, 0), None);
        }
        let table = registry.table();
        prop_assert_eq!(table.versions(&id).count(), published.len());
    }
}

/// A duplicate id inside one version is refused before the append-only
/// check, and leaves the registry untouched.
#[test]
fn duplicate_ids_are_refused() {
    let id = NomenclatureId::new("n");
    let mut registry = NomenclatureRegistry::new();
    assert_eq!(
        registry.publish(id.clone(), vec![row("o1", "A"), row("o1", "B")]),
        Err(PublishNomenclatureError::DuplicateId {
            id: id.clone(),
            option: OptionId::new("o1")
        })
    );
    assert!(registry.rows(&id, 1).is_none());
    assert!(registry.table().is_empty());
}
