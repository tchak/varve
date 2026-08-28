//! The cell-append operation against the `MemoryStore` reference
//! implementation: the surface gate, sibling-anchor translation with
//! minted item ids, conformance, and the one-batch-one-entry shape
//! (§2.9, platform P.4 *Case-file record log*, graphql.md G.14).

use varve_core::canonical::Salt;
use varve_core::primitives::Instant;
use varve_core::{ColumnId, GroupId, ItemId, RecordId, RowPath};
use varve_record::{Actor, ActorKind, EntrySalts};
use varve_schema::{Arity, Cardinality, Column, Element, Group, ScalarType, Schema, revision_id};
use varve_service::{AppendCells, AppendCellsError, CellWrite, append_cells};
use varve_store::load::load_log;
use varve_store::{MemoryStore, RecordLogStore};
use varve_surface::{ColumnNode, GroupNode, Node, Surface, WritePolicy};
use varve_value::{CellState, CellValue, Scalar};

fn column(id: &str, ty: ScalarType) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.to_string(),
        ty,
        arity: Arity::One,
    })
}

/// `name: text`, `age: integer`, `children` a `many` group of one
/// text column `first`.
fn schema() -> Schema {
    Schema {
        root: vec![
            column("name", ScalarType::Text),
            column("age", ScalarType::Integer(None)),
            Element::Group(Group {
                id: GroupId::new("children"),
                label: "children".into(),
                cardinality: Cardinality::Many,
                children: vec![column("first", ScalarType::Text)],
                included_from: None,
            }),
        ],
        resolvers: vec![],
    }
}

fn column_node(id: &str, writable: bool) -> Node {
    Node::Column(ColumnNode {
        column: ColumnId::new(id),
        prompt: None,
        help: None,
        visibility: None,
        required: None,
        write: WritePolicy {
            writable,
            override_derived: false,
        },
        format: None,
    })
}

/// A surface over [`schema`]; `age` writability is the parameter so
/// the gate has something to refuse.
fn surface(schema: &Schema, age_writable: bool) -> Surface {
    Surface {
        id: varve_core::SurfaceId::new("applicant"),
        revision: revision_id(schema),
        nodes: vec![
            column_node("name", true),
            column_node("age", age_writable),
            Node::Group(GroupNode {
                group: GroupId::new("children"),
                prompt: None,
                visibility: None,
                children: vec![column_node("first", true)],
            }),
        ],
        ineligibility: None,
    }
}

fn salts(n: usize) -> EntrySalts {
    EntrySalts {
        meta: Salt([7; 32]),
        ops: (0..n).map(|i| Salt([i as u8 + 1; 32])).collect(),
    }
}

fn request<'a>(
    schema: &'a Schema,
    surface: &'a Surface,
    writes: Vec<CellWrite>,
) -> AppendCells<'a> {
    AppendCells {
        record: RecordId::new("r1"),
        surface,
        schema,
        revision: revision_id(schema),
        writes,
        actor: Actor {
            id: "a1".into(),
            kind: ActorKind::Human,
        },
        timestamp: Instant::parse("2026-08-28T10:00:00Z").unwrap(),
    }
}

fn set(column: &str, value: &str) -> CellWrite {
    CellWrite::Set {
        column: ColumnId::new(column),
        path: RowPath::root(),
        state: CellState::Value(CellValue::One(Scalar::Text(value.into()))),
    }
}

fn minted(counter: &mut u32) -> impl FnMut() -> ItemId + '_ {
    move || {
        *counter += 1;
        ItemId::new(format!("item-{counter}"))
    }
}

#[tokio::test]
async fn a_batch_is_one_entry_and_the_fold_reads_back() {
    let store = MemoryStore::default();
    let schema = schema();
    let surface = surface(&schema, true);
    let mut n = 0;

    let outcome = append_cells(
        &store,
        request(&schema, &surface, vec![set("name", "Ada")]),
        salts,
        minted(&mut n),
    )
    .await
    .unwrap();
    assert_eq!(outcome.version, 1);

    // A second batch: add an item (appended, minted id) and set a
    // root cell — one entry again, anchors resolved on the fold.
    let outcome = append_cells(
        &store,
        request(
            &schema,
            &surface,
            vec![
                CellWrite::AddItem {
                    group: GroupId::new("children"),
                    parent: RowPath::root(),
                    before: None,
                },
                set("age", "x"), // wrong kind — checked below, separately
            ],
        ),
        salts,
        minted(&mut n),
    )
    .await;
    assert!(matches!(outcome, Err(AppendCellsError::Conformance(_))));

    let outcome = append_cells(
        &store,
        request(
            &schema,
            &surface,
            vec![CellWrite::AddItem {
                group: GroupId::new("children"),
                parent: RowPath::root(),
                before: None,
            }],
        ),
        salts,
        minted(&mut n),
    )
    .await
    .unwrap();
    assert_eq!(outcome.version, 2);
    let record = RecordId::new("r1");
    let log = load_log(&store, &record).await.unwrap();
    assert_eq!(log.entries().len(), 2);
    assert_eq!(log.entries()[0].content.ops.len(), 1);
    let fold = log.fold().unwrap();
    assert_eq!(fold.values, outcome.values);
    // The refused batch stored nothing: the minted id in the log is
    // the second mint, not the first.
    let items: Vec<_> = fold.values.items.values().flatten().collect();
    assert_eq!(items, vec![&ItemId::new("item-2")]);
    assert_eq!(store.version(&record).await.unwrap(), 2);
}

#[tokio::test]
async fn the_surface_gates_columns_and_groups() {
    let store = MemoryStore::default();
    let schema = schema();
    let read_only_age = surface(&schema, false);
    let mut n = 0;

    // A cell op on a non-writable column names the column; nothing
    // is stored — the batch dies whole.
    let err = append_cells(
        &store,
        request(
            &schema,
            &read_only_age,
            vec![set("name", "Ada"), set("age", "41")],
        ),
        salts,
        minted(&mut n),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppendCellsError::NotWritable(c) if c.as_str() == "age"));
    assert_eq!(store.version(&RecordId::new("r1")).await.unwrap(), 0);

    // An item op on a group the surface does not carry.
    let mut bare = read_only_age.clone();
    bare.nodes.truncate(2); // drop the group node
    let err = append_cells(
        &store,
        request(
            &schema,
            &bare,
            vec![CellWrite::AddItem {
                group: GroupId::new("children"),
                parent: RowPath::root(),
                before: None,
            }],
        ),
        salts,
        minted(&mut n),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, AppendCellsError::GroupNotWritable(g) if g.as_str() == "children"));
}

#[tokio::test]
async fn anchors_resolve_and_a_bad_anchor_refuses() {
    let store = MemoryStore::default();
    let schema = schema();
    let surface = surface(&schema, true);
    let mut n = 0;
    let add = |before: Option<&str>| CellWrite::AddItem {
        group: GroupId::new("children"),
        parent: RowPath::root(),
        before: before.map(ItemId::new),
    };

    append_cells(
        &store,
        request(&schema, &surface, vec![add(None), add(None)]),
        salts,
        minted(&mut n),
    )
    .await
    .unwrap();
    // Anchored before the first item.
    let outcome = append_cells(
        &store,
        request(&schema, &surface, vec![add(Some("item-1"))]),
        salts,
        minted(&mut n),
    )
    .await
    .unwrap();
    let items: Vec<_> = outcome.values.items.values().flatten().cloned().collect();
    assert_eq!(
        items,
        ["item-3", "item-1", "item-2"].map(ItemId::new).to_vec()
    );

    let err = append_cells(
        &store,
        request(&schema, &surface, vec![add(Some("ghost"))]),
        salts,
        minted(&mut n),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(err, AppendCellsError::UnknownAnchor { item, .. } if item.as_str() == "ghost")
    );
}

#[tokio::test]
async fn admissibility_never_gates_an_append() {
    // A batch that leaves the record inadmissible (nothing set on a
    // surface that would require plenty) appends fine: findings are
    // the output of a read, never a gate (G.14).
    let store = MemoryStore::default();
    let schema = schema();
    let surface = surface(&schema, true);
    let mut n = 0;
    append_cells(
        &store,
        request(
            &schema,
            &surface,
            vec![CellWrite::Set {
                column: ColumnId::new("name"),
                path: RowPath::root(),
                state: CellState::Empty,
            }],
        ),
        salts,
        minted(&mut n),
    )
    .await
    .unwrap();
    assert_eq!(store.version(&RecordId::new("r1")).await.unwrap(), 1);
}

#[tokio::test]
async fn a_checkpoint_entry_lands_and_reads_back() {
    use varve_record::Checkpoint;
    use varve_service::{AppendCheckpoint, append_checkpoint};

    let store = MemoryStore::default();
    let schema = schema();
    let surface = surface(&schema, true);
    let mut n = 0;
    append_cells(
        &store,
        request(&schema, &surface, vec![set("name", "Ada")]),
        salts,
        minted(&mut n),
    )
    .await
    .unwrap();

    // The dépôt shape (G.15): nothing frozen, nothing expected.
    let version = append_checkpoint(
        &store,
        AppendCheckpoint {
            record: RecordId::new("r1"),
            checkpoint: Checkpoint {
                name: "submitted".into(),
                reading_revision: revision_id(&schema),
                expected: vec![],
                frozen_columns: Default::default(),
                frozen_groups: Default::default(),
            },
            revision: revision_id(&schema),
            actor: Actor {
                id: "a1".into(),
                kind: ActorKind::Human,
            },
            timestamp: Instant::parse("2026-08-28T11:00:00Z").unwrap(),
        },
        salts,
    )
    .await
    .unwrap();
    assert_eq!(version, 2);

    // The checkpoint reads back positioned, and cell writes after it
    // still append — dépôt froze nothing (§2.9, Q12).
    let record = RecordId::new("r1");
    let log = load_log(&store, &record).await.unwrap();
    let checkpoints = log.checkpoints();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].seq, 1);
    assert_eq!(checkpoints[0].checkpoint.name, "submitted");
    append_cells(
        &store,
        request(&schema, &surface, vec![set("name", "Grace")]),
        salts,
        minted(&mut n),
    )
    .await
    .unwrap();
    let log = load_log(&store, &record).await.unwrap();
    assert_eq!(
        varve_record::validate_after_checkpoint(&log, 1),
        vec![],
        "nothing frozen, nothing violated"
    );
}
