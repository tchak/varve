//! Publication laws over generated schemas: the DAG is content-addressed
//! (§2.13 — one object per schema, however often it is published), the
//! publication log is history (§2.1 — every event counts, `latest` is the
//! last one), and nomenclature versions are append-only (§2.11).

use proptest::prelude::*;
use varve_core::{ColumnId, NomenclatureId, OptionId};
use varve_revision::{NomenclatureRegistry, PublishNomenclatureError, RevisionDag};
use varve_schema::{Arity, Column, Element, OptionRow, ScalarType, Schema, revision_id};

fn column(id: &str, ty: ScalarType) -> Element {
    Element::Column(Column { id: ColumnId::new(id), label: id.into(), ty, arity: Arity::One })
}

/// A handful of distinguishable schemas — small enough that repeats
/// (reverts) come up constantly.
fn schema() -> impl Strategy<Value = Schema> {
    (
        proptest::sample::subsequence(vec!["a", "b", "c"], 0..=3),
        prop_oneof![Just(ScalarType::Text), Just(ScalarType::Boolean), Just(ScalarType::Integer(None))],
    )
        .prop_map(|(ids, ty)| Schema {
            root: ids.into_iter().map(|id| column(id, ty.clone())).collect(),
            resolvers: vec![],
        })
}

fn row(id: &str, label: &str) -> OptionRow {
    OptionRow { id: OptionId::new(id), label: label.into(), fields: vec![] }
}

proptest! {
    /// Publishing a sequence of schemas: ids are the content hashes; the
    /// object store holds one object per distinct schema; the log holds
    /// one event per publication; `latest` is the last event; every
    /// event's parents were known when it was published.
    #[test]
    fn publication_is_content_addressed_and_the_log_is_history(
        schemas in proptest::collection::vec(schema(), 1..8),
    ) {
        let mut dag = RevisionDag::new();
        let mut distinct = std::collections::BTreeSet::new();
        let mut previous = None;
        for (i, s) in schemas.iter().enumerate() {
            let parents: Vec<_> = previous.iter().cloned().collect();
            let id = dag.publish(s.clone(), parents.clone()).unwrap();
            prop_assert_eq!(&id, &revision_id(s));
            distinct.insert(id.clone());
            // The object: same schema, parents from its *first*
            // publication.
            prop_assert_eq!(&dag.get(&id).unwrap().schema, s);
            // The event: exactly as published.
            let events = dag.publications();
            prop_assert_eq!(events.len(), i + 1);
            prop_assert_eq!(&events[i].revision, &id);
            prop_assert_eq!(&events[i].parents, &parents);
            prop_assert_eq!(dag.latest(), Some(&id));
            previous = Some(id);
        }
        // One object per distinct schema; one history entry per event,
        // in order.
        let history: Vec<_> = dag.history().map(|(id, _)| id.clone()).collect();
        prop_assert_eq!(history.len(), schemas.len());
        prop_assert_eq!(
            history.iter().collect::<std::collections::BTreeSet<_>>().len(),
            distinct.len()
        );
        for (id, s) in dag.history() {
            prop_assert!(distinct.contains(id));
            prop_assert_eq!(revision_id(s), id.clone());
        }
    }

    /// Republishing the current schema is idempotent on objects and on
    /// `latest`, and is still an event.
    #[test]
    fn republishing_the_same_schema_adds_no_object(s in schema(), times in 1usize..4) {
        let mut dag = RevisionDag::new();
        let first = dag.publish(s.clone(), vec![]).unwrap();
        for _ in 0..times {
            let again = dag.publish(s.clone(), vec![first.clone()]).unwrap();
            prop_assert_eq!(&again, &first);
            prop_assert_eq!(dag.latest(), Some(&first));
        }
        prop_assert_eq!(dag.publications().len(), times + 1);
        // The object keeps its first parents (none), not the revert's.
        prop_assert!(dag.get(&first).unwrap().parents.is_empty());
        // The aggregate over the lineage is over the one object.
        let (agg, _) = dag.aggregate(&Default::default()).unwrap();
        prop_assert_eq!(agg.columns.len(), s.root.len());
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
        Err(PublishNomenclatureError::DuplicateId { id: id.clone(), option: OptionId::new("o1") })
    );
    assert!(registry.rows(&id, 1).is_none());
    assert!(registry.table().is_empty());
}
