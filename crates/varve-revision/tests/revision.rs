use varve_core::{ColumnId, GroupId, NomenclatureId, OptionId, RevisionId};
use varve_revision::{
    AggregatePolicy, MergeConflict, NomenclatureRegistry, PublishNomenclatureError,
    RevisionDag, aggregate, merge,
};
use varve_schema::{
    Arity, Cardinality, Column, Element, Group, OptionRow, ScalarType, Schema, Unit,
    revision_id,
};

fn column(id: &str, ty: ScalarType) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.to_string(),
        ty,
        arity: Arity::One,
    })
}

fn schema(elements: Vec<Element>) -> Schema {
    Schema { root: elements, resolvers: vec![] }
}

fn row(id: &str, label: &str) -> OptionRow {
    OptionRow { id: OptionId::new(id), label: label.into(), fields: vec![] }
}

#[test]
fn revision_ids_converge_and_diverge() {
    let a = schema(vec![column("name", ScalarType::Text)]);
    let b = schema(vec![column("name", ScalarType::Text)]);
    // Identical schemas → identical ids, on any instance (§2.13).
    assert_eq!(revision_id(&a), revision_id(&b));
    // A relabel is a new revision (§2.11).
    let mut c = schema(vec![column("name", ScalarType::Text)]);
    if let Element::Column(col) = &mut c.root[0] {
        col.label = "Nom".into();
    }
    assert_ne!(revision_id(&a), revision_id(&c));
    // A unit is identity-bearing (§2.14).
    let d = schema(vec![column("name", ScalarType::Integer(Some(Unit::Day)))]);
    let e = schema(vec![column("name", ScalarType::Integer(None))]);
    assert_ne!(revision_id(&d), revision_id(&e));
}

#[test]
fn dag_publishes_and_deduplicates() {
    let mut dag = RevisionDag::new();
    let r1 = dag.publish(schema(vec![column("a", ScalarType::Text)]), vec![]).unwrap();
    let r2 = dag
        .publish(
            schema(vec![column("a", ScalarType::Text), column("b", ScalarType::Boolean)]),
            vec![r1.clone()],
        )
        .unwrap();
    assert_eq!(dag.get(&r2).unwrap().parents, vec![r1.clone()]);
    assert_eq!(dag.latest(), Some(&r2));
    // Republishing r1's schema after r2 is a *revert*: the same object
    // (content-addressed — no new revision), but a new publication
    // event: it is `latest` again, following r2.
    let again = dag.publish(schema(vec![column("a", ScalarType::Text)]), vec![r2.clone()]).unwrap();
    assert_eq!(again, r1);
    assert_eq!(dag.latest(), Some(&r1));
    assert_eq!(dag.history().count(), 3);
    assert_eq!(dag.publications().last().unwrap().parents, vec![r2.clone()]);
    // The object's own parents are those of its first publication.
    assert_eq!(dag.get(&r1).unwrap().parents, vec![]);
    // Unknown parents are rejected.
    assert!(dag
        .publish(schema(vec![]), vec![RevisionId::new("nope")])
        .is_err());
}

#[test]
fn nomenclature_publication_is_append_only() {
    let mut registry = NomenclatureRegistry::new();
    let id = NomenclatureId::new("statut");
    let v1 = registry
        .publish(id.clone(), vec![row("o1", "En cours"), row("o2", "Clos")])
        .unwrap();
    assert_eq!(v1, 1);
    // Relabels and additions are fine — renames are the point (§2.11).
    let v2 = registry
        .publish(
            id.clone(),
            vec![
                row("o1", "En cours d'instruction"),
                row("o2", "Clos"),
                row("o3", "Suspendu"),
            ],
        )
        .unwrap();
    assert_eq!(v2, 2);
    // Removal is rejected: the §5.5 join leans on this invariant.
    let removal = registry.publish(id.clone(), vec![row("o1", "En cours")]);
    assert!(matches!(
        removal,
        Err(PublishNomenclatureError::RemovesIds { removed, .. })
            if removed == vec![OptionId::new("o2"), OptionId::new("o3")]
    ));
    assert_eq!(registry.rows(&id, 1).unwrap().len(), 2);
    // The table carries every version: a column bound to v1 resolves
    // against v1's rows, not the latest (§2.12).
    let table = registry.table();
    assert_eq!(table.get(&id, 1).unwrap().len(), 2);
    assert_eq!(table.get(&id, 2).unwrap().len(), 3);
    assert!(table.get(&id, 3).is_none());
    assert_eq!(table.versions(&id).map(|(v, _)| v).collect::<Vec<_>>(), vec![1, 2]);
}

#[test]
fn block_publication_numbers_versions_and_validates() {
    use varve_core::BlockId;
    use varve_revision::{BlockRegistry, PublishBlockError};
    use varve_schema::{Block, DepthPolicy};
    let shell = |cols: Vec<Element>| Group {
        id: GroupId::new("rib"),
        label: "RIB".into(),
        cardinality: Cardinality::One,
        children: cols,
        included_from: None,
    };
    let block = |version: u32| Block {
        id: BlockId::new("rib"),
        version,
        group: shell(vec![column("iban", ScalarType::Text)]),
        resolvers: vec![],
    };
    let mut registry = BlockRegistry::new();
    assert_eq!(registry.publish(block(1), DepthPolicy::default()), Ok(1));
    // Versions are numbered by the registry: a stale author fails loudly.
    assert!(matches!(
        registry.publish(block(1), DepthPolicy::default()),
        Err(PublishBlockError::VersionMismatch { expected: 1, next: 2, .. })
    ));
    assert_eq!(registry.publish(block(2), DepthPolicy::default()), Ok(2));
    assert_eq!(registry.get(&BlockId::new("rib"), 1).unwrap().version, 1);
    assert_eq!(registry.latest(&BlockId::new("rib")).unwrap().version, 2);
    // The shell's group id is the block's inclusion identity: it stays.
    let mut renamed = block(3);
    renamed.group.id = GroupId::new("bank");
    assert!(matches!(
        registry.publish(renamed, DepthPolicy::default()),
        Err(PublishBlockError::ShellIdChanged { .. })
    ));
    // An invalid shell (duplicate column ids) is refused.
    let mut invalid = block(3);
    invalid.group.children.push(column("iban", ScalarType::Text));
    assert!(matches!(
        registry.publish(invalid, DepthPolicy::default()),
        Err(PublishBlockError::Invalid { .. })
    ));
    assert_eq!(registry.all().count(), 2);
}

#[test]
fn three_way_merge_combines_disjoint_edits() {
    let base = schema(vec![
        column("a", ScalarType::Text),
        column("b", ScalarType::Integer(None)),
    ]);
    // Left retypes b; right adds c and removes nothing.
    let left = schema(vec![
        column("a", ScalarType::Text),
        column("b", ScalarType::Decimal(None)),
    ]);
    let right = schema(vec![
        column("a", ScalarType::Text),
        column("b", ScalarType::Integer(None)),
        column("c", ScalarType::Boolean),
    ]);
    let merged = merge(&base, &left, &right).unwrap();
    let ids: Vec<&str> = merged
        .root
        .iter()
        .map(|e| match e {
            Element::Column(c) => c.id.as_str(),
            Element::Group(g) => g.id.as_str(),
        })
        .collect();
    assert_eq!(ids, vec!["a", "b", "c"]);
    let Element::Column(b) = &merged.root[1] else { panic!() };
    assert_eq!(b.ty, ScalarType::Decimal(None));

    // Both modify b differently: conflict, named.
    let right2 = schema(vec![
        column("a", ScalarType::Text),
        column("b", ScalarType::Text),
    ]);
    let conflicts = merge(&base, &left, &right2).unwrap_err();
    assert_eq!(conflicts, vec![MergeConflict::Column(ColumnId::new("b"))]);

    // Remove vs modify: also a conflict.
    let removed = schema(vec![column("a", ScalarType::Text)]);
    assert!(merge(&base, &left, &removed).is_err());
}

#[test]
fn merge_recurses_into_groups() {
    let group = |children: Vec<Element>| {
        Element::Group(Group {
            included_from: None,
            id: GroupId::new("g"),
            label: "g".into(),
            cardinality: Cardinality::Many,
            children,
        })
    };
    let base = schema(vec![group(vec![column("x", ScalarType::Text)])]);
    let left = schema(vec![group(vec![
        column("x", ScalarType::Text),
        column("y", ScalarType::Boolean),
    ])]);
    let right = schema(vec![group(vec![column("x", ScalarType::Decimal(None))])]);
    let merged = merge(&base, &left, &right).unwrap();
    let Element::Group(g) = &merged.root[0] else { panic!() };
    assert_eq!(g.children.len(), 2);
    let Element::Column(x) = &g.children[0] else { panic!() };
    assert_eq!(x.ty, ScalarType::Decimal(None));

    // One side deletes the group, the other adds a column inside it:
    // the addition would survive with nowhere to live — a conflict,
    // never a silent drop.
    let deleted = schema(vec![]);
    let added_inside = schema(vec![group(vec![
        column("x", ScalarType::Text),
        column("y", ScalarType::Boolean),
    ])]);
    assert_eq!(
        merge(&base, &deleted, &added_inside),
        Err(vec![MergeConflict::OrphanedByDeletedGroup {
            group: GroupId::new("g"),
            element: "y".into(),
        }])
    );
    // Symmetric.
    assert!(merge(&base, &added_inside, &deleted).is_err());
    // Deleting the group on one side while the other leaves it untouched
    // deletes it, children included — no orphan, no conflict.
    assert_eq!(merge(&base, &deleted, &base).unwrap().root, vec![]);
}

#[test]
fn aggregate_joins_history_and_reports() {
    let noms = Default::default();
    let r = |n: u32| RevisionId::new(format!("r{n}"));
    // History: b is integer then decimal (joins); dropped disappears
    // after r1 (deprecated); clash is boolean then date (ViaText);
    // broken is attachment then integer (omitted).
    let rev1 = schema(vec![
        column("b", ScalarType::Integer(None)),
        column("dropped", ScalarType::Text),
        column("clash", ScalarType::Boolean),
        column("broken", ScalarType::Attachment(Default::default())),
    ]);
    let rev2 = schema(vec![
        column("clash", ScalarType::Date),
        column("b", ScalarType::Decimal(None)),
        column("broken", ScalarType::Integer(None)),
    ]);
    let history = [(r(1), &rev1), (r(2), &rev2)];
    let (agg, report) = aggregate(&history, &noms).unwrap();

    // Order: latest revision's order (clash, b), then deprecated.
    let ids: Vec<&str> = agg.columns.iter().map(|c| c.column.as_str()).collect();
    assert_eq!(ids, vec!["clash", "b", "dropped"]);

    let by_id = |id: &str| agg.columns.iter().find(|c| c.column.as_str() == id).unwrap();
    assert_eq!(by_id("b").ty, ScalarType::Decimal(None));
    assert_eq!(by_id("b").deprecated_since, None);
    assert_eq!(by_id("clash").ty, ScalarType::Text);
    assert_eq!(by_id("dropped").deprecated_since, Some(r(2)));

    assert!(report.entries.contains(&(ColumnId::new("clash"), AggregatePolicy::ViaText)));
    assert!(report.entries.contains(&(ColumnId::new("broken"), AggregatePolicy::Omitted)));
    assert!(!agg.columns.iter().any(|c| c.column.as_str() == "broken"));

    // Removed then re-added with another type, same id (§5.5 row 2):
    // joined across the gap, and reported.
    let rev3 = schema(vec![column("b", ScalarType::Decimal(None))]);
    let rev4 = schema(vec![column("b", ScalarType::Decimal(None)), column("dropped", ScalarType::Integer(None))]);
    let history = [(r(1), &rev1), (r(2), &rev2), (r(3), &rev3), (r(4), &rev4)];
    let (_, report) = aggregate(&history, &noms).unwrap();
    assert!(report.entries.contains(&(ColumnId::new("dropped"), AggregatePolicy::ReAddedRetyped)));

    // The DAG builds its own aggregate over distinct revisions, in
    // first-publication order — a revert repeats no revision.
    let mut dag = RevisionDag::new();
    let id1 = dag.publish(rev1.clone(), vec![]).unwrap();
    let id2 = dag.publish(rev2.clone(), vec![id1.clone()]).unwrap();
    dag.publish(rev1.clone(), vec![id2]).unwrap(); // revert
    let (from_dag, _) = dag.aggregate(&noms).unwrap();
    let (from_slice, _) = aggregate(&[(id1, &rev1), (r(2), &rev2)], &noms).unwrap();
    assert_eq!(from_dag.columns.len(), from_slice.columns.len());
}
