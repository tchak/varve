//! The **preview value bag** (platform P.4 *The fillable preview*,
//! graphql.md G.12): scratch record values an administrator fills
//! the draft form with, to watch admissibility behave before
//! publishing. Stored as a plain [`RecordValues`] in its own column
//! beside the draft — never a `varve-record` log: no history, no
//! actor, no hash chain (§2.10's obligations are for case files, not
//! a bag publication discards). Filling never forks the virtual
//! draft; the bag is cleared by discard and by publication (a
//! draft-cycle scope).
//!
//! Writes mirror the kernel's five-op patch set ([`Op`]) with item
//! placement as a sibling anchor and server-minted item ids (the
//! G.7.3 argument). A batch applies in order, all-or-nothing, and
//! the result must *conform* to the draft's derived schema
//! ([`varve_value::check`] — type-level only): admissibility
//! (required, formats) never refuses a write, it is the read model's
//! output ([`preview_findings`]).
//!
//! Stale cells are inert (G.12): a tree edit can orphan stored
//! values (a removed column, a type change); reads prune in memory,
//! writes prune before applying — `tree_edit` stays ignorant of
//! preview state.

use varve_core::canonical::CanonicalValue;
use varve_core::{ColumnId, GroupId, ItemId, RowPath};
use varve_logic::PendingSet;
use varve_record::canon::{
    RecordDecodeError, path_canonical, path_from, state_canonical, state_from,
};
use varve_schema::{NomenclatureTable, Schema, revision_id};
use varve_surface::Finding;
use varve_value::{
    ApplyError, CellState, ConformanceError, ItemsAddr, Op, RecordValues, apply, check,
};

use crate::procedure::{Procedure, RevisionDraftError, WorkingTree, working_tree};
use crate::surfaces::{APPLICANT_SURFACE, REVIEWER_SURFACE, compile_surfaces};
use crate::tree::Tree;

/// A preview [`RecordValues`] as its stored JSON bytes (one `BYTEA`
/// column). Constructed from kernel values only, so the column never
/// holds anything [`PreviewBytes::decode`] would refuse — short of
/// corruption, which [`PreviewError::Corrupt`] surfaces.
#[derive(Debug, Clone, PartialEq, Eq, toasty::Embed)]
pub struct PreviewBytes(Vec<u8>);

impl PreviewBytes {
    pub fn encode(values: &RecordValues) -> Self {
        let cells: Vec<serde_json::Value> = values
            .cells
            .iter()
            .map(|(addr, state)| {
                serde_json::json!({
                    "column": addr.column.as_str(),
                    "path": canonical_to_json(&path_canonical(&addr.path)),
                    "state": canonical_to_json(&state_canonical(state)),
                })
            })
            .collect();
        let items: Vec<serde_json::Value> = values
            .items
            .iter()
            .map(|(addr, list)| {
                serde_json::json!({
                    "group": addr.group.as_str(),
                    "parent": canonical_to_json(&path_canonical(&addr.parent)),
                    "items": list.iter().map(ItemId::as_str).collect::<Vec<_>>(),
                })
            })
            .collect();
        let value = serde_json::json!({ "cells": cells, "items": items });
        Self(serde_json::to_vec(&value).expect("json! values serialize"))
    }

    /// Decodes stored bytes; refuses anything [`PreviewBytes::encode`]
    /// does not produce.
    pub fn decode(&self) -> Result<RecordValues, PreviewDecodeError> {
        let malformed = |m: &str| PreviewDecodeError(m.to_string());
        let value: serde_json::Value =
            serde_json::from_slice(&self.0).map_err(|e| PreviewDecodeError(e.to_string()))?;
        let object = value
            .as_object()
            .ok_or_else(|| malformed("not an object"))?;
        let mut values = RecordValues::default();
        for cell in as_array(object.get("cells"), "cells")? {
            let cell = cell
                .as_object()
                .ok_or_else(|| malformed("cell not an object"))?;
            let column = ColumnId::new(str_field(cell, "column")?);
            let path = path_from(&json_to_canonical(
                cell.get("path")
                    .ok_or_else(|| malformed("cell without path"))?,
            ))?;
            let state = state_from(&json_to_canonical(
                cell.get("state")
                    .ok_or_else(|| malformed("cell without state"))?,
            ))?;
            values
                .cells
                .insert(varve_value::CellAddr { column, path }, state);
        }
        for entry in as_array(object.get("items"), "items")? {
            let entry = entry
                .as_object()
                .ok_or_else(|| malformed("item list not an object"))?;
            let group = GroupId::new(str_field(entry, "group")?);
            let parent = path_from(&json_to_canonical(
                entry
                    .get("parent")
                    .ok_or_else(|| malformed("item list without parent"))?,
            ))?;
            let list = as_array(entry.get("items"), "items")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(ItemId::new)
                        .ok_or_else(|| malformed("item id not a string"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            values.items.insert(ItemsAddr { group, parent }, list);
        }
        Ok(values)
    }
}

/// The stored preview no longer decodes — never produced by this
/// crate's writes; a database-level corruption to surface, not
/// silently replace.
#[derive(Debug, thiserror::Error)]
#[error("stored preview is unreadable: {0}")]
pub struct PreviewDecodeError(String);

impl From<RecordDecodeError> for PreviewDecodeError {
    fn from(e: RecordDecodeError) -> Self {
        Self(e.0)
    }
}

fn as_array<'a>(
    v: Option<&'a serde_json::Value>,
    key: &str,
) -> Result<&'a Vec<serde_json::Value>, PreviewDecodeError> {
    v.and_then(serde_json::Value::as_array)
        .ok_or_else(|| PreviewDecodeError(format!("'{key}' is not an array")))
}

fn str_field(
    m: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<String, PreviewDecodeError> {
    m.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| PreviewDecodeError(format!("'{key}' is not a string")))
}

/// Kernel canonical → `serde_json`, total. The stored bytes are a
/// platform column, not a hash preimage — plain JSON is enough; the
/// *shapes* inside stay the kernel's canonical encodings
/// ([`state_canonical`], [`path_canonical`]) so no scalar encoding is
/// invented here.
fn canonical_to_json(v: &CanonicalValue) -> serde_json::Value {
    match v {
        CanonicalValue::Null => serde_json::Value::Null,
        CanonicalValue::Bool(b) => serde_json::Value::Bool(*b),
        CanonicalValue::Int(i) => serde_json::Value::from(*i),
        // Finite by construction upstream (geometry coordinates); a
        // NaN would only arise from in-process misuse.
        CanonicalValue::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        CanonicalValue::String(s) => serde_json::Value::String(s.clone()),
        CanonicalValue::Array(a) => {
            serde_json::Value::Array(a.iter().map(canonical_to_json).collect())
        }
        CanonicalValue::Object(o) => serde_json::Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), canonical_to_json(v)))
                .collect(),
        ),
    }
}

/// `serde_json` → kernel canonical: integral numbers are `Int`, the
/// rest `Float` — the same reading `varve-wire` gives JSON numbers.
fn json_to_canonical(v: &serde_json::Value) -> CanonicalValue {
    match v {
        serde_json::Value::Null => CanonicalValue::Null,
        serde_json::Value::Bool(b) => CanonicalValue::Bool(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => CanonicalValue::Int(i),
            None => CanonicalValue::Float(n.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(s) => CanonicalValue::String(s.clone()),
        serde_json::Value::Array(a) => {
            CanonicalValue::Array(a.iter().map(json_to_canonical).collect())
        }
        serde_json::Value::Object(o) => CanonicalValue::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), json_to_canonical(v)))
                .collect(),
        ),
    }
}

/// One preview write (G.12): the kernel op set with item placement as
/// a **sibling anchor**, never an index (`before: None` appends), and
/// item ids server-minted — both the G.7.3 argument.
#[derive(Debug, Clone)]
pub enum PreviewWrite {
    Set {
        column: ColumnId,
        path: RowPath,
        state: CellState,
    },
    /// Back to absent — distinct from `Set(Empty)` (§2.4).
    Unset { column: ColumnId, path: RowPath },
    AddItem {
        group: GroupId,
        parent: RowPath,
        before: Option<ItemId>,
    },
    RemoveItem {
        group: GroupId,
        parent: RowPath,
        item: ItemId,
    },
    Reorder {
        group: GroupId,
        parent: RowPath,
        order: Vec<ItemId>,
    },
}

/// A refused batch (G.12 `INVALID_WRITE`): a type-level mistake —
/// the client's, never the values' admissibility. Nothing is stored.
#[derive(Debug, thiserror::Error)]
pub enum PreviewWriteError {
    #[error(transparent)]
    Apply(#[from] ApplyError),
    /// `AddItem` anchored on an item absent from its group's list.
    #[error("anchor item '{item}' is not in group '{group}'")]
    UnknownAnchor { group: GroupId, item: ItemId },
    /// The written values do not fit the draft's derived schema.
    #[error("{}", format_conformance(.0))]
    Conformance(Vec<ConformanceError>),
}

fn format_conformance(errors: &[ConformanceError]) -> String {
    errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error(transparent)]
    Write(#[from] PreviewWriteError),
    /// The working tree failed to load (corrupt draft, or the
    /// database — including the optimistic-concurrency conflict).
    #[error(transparent)]
    Draft(#[from] RevisionDraftError),
    #[error(transparent)]
    Corrupt(#[from] PreviewDecodeError),
    #[error(transparent)]
    Db(#[from] toasty::Error),
}

/// Applies `writes` to the procedure's stored preview bag and stores
/// the result — all-or-nothing: a refused write ([`PreviewWriteError`])
/// stores nothing. The stored bag is pruned against the current
/// working tree first (stale cells drop on the next write, G.12), and
/// the written result must conform to the tree's derived schema. The
/// procedure must come from
/// [`crate::procedure::find_procedure_with_revision_draft`]; on
/// success it is updated in place and the stored bag returned.
///
/// Concurrency is the row's `#[version]` guard, shared with the tree
/// autosaves: a racing edit fails the store (`condition_failed`)
/// instead of overwriting.
pub async fn update_preview(
    db: &mut toasty::Db,
    procedure: &mut Procedure,
    writes: Vec<PreviewWrite>,
) -> Result<RecordValues, PreviewError> {
    let WorkingTree { tree, .. } = working_tree(procedure)?;
    let schema = tree.schema();
    let nomenclatures = NomenclatureTable::new();
    let mut values = stored_values(procedure)?;
    prune(&mut values, &schema, &nomenclatures);
    apply_writes(&mut values, writes)?;
    let errors = check(&values, &schema, &nomenclatures);
    if !errors.is_empty() {
        return Err(PreviewWriteError::Conformance(errors).into());
    }
    procedure
        .update()
        .preview(Some(PreviewBytes::encode(&values)))
        .exec(db)
        .await?;
    Ok(values)
}

/// The preview bag as reads fold it (G.12): the stored values pruned
/// against the given derived schema — orphaned cells (a removed
/// column, a changed type) are inert, never surfaced. The procedure
/// must come from
/// [`crate::procedure::find_procedure_with_revision_draft`].
pub fn preview_values(
    procedure: &Procedure,
    schema: &Schema,
) -> Result<RecordValues, PreviewDecodeError> {
    let mut values = stored_values(procedure)?;
    prune(&mut values, schema, &NomenclatureTable::new());
    Ok(values)
}

/// One admissibility finding of the preview, tagged with the compiled
/// surface it holds on (`applicant` / `reviewer` — P.4's fixed pair).
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewFinding {
    pub surface: &'static str,
    pub finding: Finding,
}

/// The preview's findings (G.12): admissibility of `values` evaluated
/// per compiled surface — [`compile_surfaces`] over the draft tree,
/// the pending set empty and eligibility unevaluated (no lifecycle in
/// a preview). `values` must already be pruned ([`preview_values`]).
pub fn preview_findings(
    tree: &Tree,
    values: &RecordValues,
) -> Result<Vec<PreviewFinding>, varve_surface::SurfaceError> {
    let schema = tree.schema();
    let revision = revision_id(&schema);
    let pair = compile_surfaces(tree, &revision);
    let nomenclatures = NomenclatureTable::new();
    let pending = PendingSet::default();
    let mut findings = Vec::new();
    for (name, surface) in [
        (APPLICANT_SURFACE, &pair.applicant),
        (REVIEWER_SURFACE, &pair.reviewer),
    ] {
        let report =
            varve_surface::admissibility(surface, &schema, &nomenclatures, values, &pending)?;
        findings.extend(report.findings.into_iter().map(|finding| PreviewFinding {
            surface: name,
            finding,
        }));
    }
    Ok(findings)
}

/// The stored bag, undecoded stale cells and all; `None` stored means
/// the empty bag.
///
/// # Panics
///
/// If the preview was not loaded (the catalog lookups defer it).
fn stored_values(procedure: &Procedure) -> Result<RecordValues, PreviewDecodeError> {
    match procedure.preview.get() {
        Some(bytes) => bytes.decode(),
        None => Ok(RecordValues::default()),
    }
}

fn apply_writes(
    values: &mut RecordValues,
    writes: Vec<PreviewWrite>,
) -> Result<(), PreviewWriteError> {
    for write in writes {
        let op = match write {
            PreviewWrite::Set {
                column,
                path,
                state,
            } => Op::Set {
                column,
                path,
                state,
            },
            PreviewWrite::Unset { column, path } => Op::Unset { column, path },
            PreviewWrite::AddItem {
                group,
                parent,
                before,
            } => {
                let list = values.items.get(&ItemsAddr {
                    group: group.clone(),
                    parent: parent.clone(),
                });
                let at = match &before {
                    Some(item) => list
                        .and_then(|l| l.iter().position(|i| i == item))
                        .ok_or_else(|| PreviewWriteError::UnknownAnchor {
                            group: group.clone(),
                            item: item.clone(),
                        })?,
                    None => list.map_or(0, Vec::len),
                };
                Op::AddItem {
                    group,
                    parent,
                    item: new_item_id(),
                    at,
                }
            }
            PreviewWrite::RemoveItem {
                group,
                parent,
                item,
            } => Op::RemoveItem {
                group,
                parent,
                item,
            },
            PreviewWrite::Reorder {
                group,
                parent,
                order,
            } => Op::Reorder {
                group,
                parent,
                order,
            },
        };
        apply(values, &op)?;
    }
    Ok(())
}

/// Preview item ids are minted here (v4 — no ordering to preserve),
/// like draft element ids ([`crate::tree_edit`]).
fn new_item_id() -> ItemId {
    ItemId::new(uuid::Uuid::new_v4().simple().to_string())
}

/// Drops everything [`check`] refuses until the bag conforms —
/// aggressive by design: the bag is scratch, and a stale cell kept
/// would block every future write. Terminates: every pass removes at
/// least one cell or item list, with a full reset as the backstop
/// should a refusal ever name nothing removable.
fn prune(values: &mut RecordValues, schema: &Schema, nomenclatures: &NomenclatureTable) {
    loop {
        let errors = check(values, schema, nomenclatures);
        if errors.is_empty() {
            return;
        }
        let before = (values.cells.len(), values.items.len());
        for error in &errors {
            cull(values, error);
        }
        if (values.cells.len(), values.items.len()) == before {
            *values = RecordValues::default();
            return;
        }
    }
}

fn cull(values: &mut RecordValues, error: &ConformanceError) {
    use ConformanceError as E;
    match error {
        E::UnknownColumn(c)
        | E::ScopeMismatch(c)
        | E::UnknownItem(c, _)
        | E::ArityMismatch(c)
        | E::TypeMismatch(c)
        | E::UnknownOption(c, _)
        | E::UnknownNomenclature(c, _, _)
        | E::DuplicateElement(c)
        | E::EmptyList(c)
        | E::AttachmentTypeNotAccepted(c, _)
        | E::AttachmentTooLarge(c)
        | E::AttachmentSizeUnrepresentable(c) => {
            values.cells.retain(|addr, _| addr.column != *c);
        }
        E::UnknownGroup(g)
        | E::MisplacedItems(g)
        | E::DuplicateItem(g)
        | E::OrphanItemList(g, _)
        | E::EmptyItemList(g) => {
            values
                .items
                .retain(|addr, _| addr.group != *g && !path_names(&addr.parent, g));
            values.cells.retain(|addr, _| !path_names(&addr.path, g));
        }
    }
}

fn path_names(path: &RowPath, group: &GroupId) -> bool {
    path.segments().iter().any(|seg| seg.group == *group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use varve_core::PathSeg;
    use varve_schema::{Arity, Cardinality, ScalarType};
    use varve_value::{CellAddr, CellValue, Scalar};

    use crate::tree::{Audience, TreeColumn, TreeElement, TreeGroup};

    fn column(id: &str, ty: ScalarType) -> TreeElement {
        TreeElement::Column(TreeColumn {
            id: ColumnId::new(id),
            label: id.to_owned(),
            ty,
            arity: Arity::One,
            required: false,
            format: None,
            audience: Audience::All,
        })
    }

    fn tree() -> Tree {
        Tree {
            elements: vec![
                column("name", ScalarType::Text),
                TreeElement::Group(TreeGroup {
                    id: GroupId::new("kids"),
                    label: "kids".into(),
                    cardinality: Cardinality::Many,
                    audience: Audience::All,
                    children: vec![column("kid_name", ScalarType::Text)],
                }),
            ],
        }
    }

    fn set(column: &str, path: RowPath, text: &str) -> PreviewWrite {
        PreviewWrite::Set {
            column: ColumnId::new(column),
            path,
            state: CellState::Value(CellValue::One(Scalar::Text(text.into()))),
        }
    }

    #[test]
    fn encode_decode_round_trips() {
        let mut values = RecordValues::default();
        apply_writes(
            &mut values,
            vec![
                set("name", RowPath::root(), "Ada"),
                PreviewWrite::AddItem {
                    group: GroupId::new("kids"),
                    parent: RowPath::root(),
                    before: None,
                },
            ],
        )
        .unwrap();
        let item = values.items.values().next().unwrap()[0].clone();
        let path = RowPath::root().child(PathSeg {
            group: GroupId::new("kids"),
            item,
        });
        apply_writes(&mut values, vec![set("kid_name", path, "Sam")]).unwrap();
        assert_eq!(PreviewBytes::encode(&values).decode().unwrap(), values);
    }

    #[test]
    fn anchor_resolves_and_refuses() {
        let mut values = RecordValues::default();
        let group = GroupId::new("kids");
        apply_writes(
            &mut values,
            vec![PreviewWrite::AddItem {
                group: group.clone(),
                parent: RowPath::root(),
                before: None,
            }],
        )
        .unwrap();
        let first = values.items.values().next().unwrap()[0].clone();
        // Anchored on the existing item: the new one lands in front.
        apply_writes(
            &mut values,
            vec![PreviewWrite::AddItem {
                group: group.clone(),
                parent: RowPath::root(),
                before: Some(first.clone()),
            }],
        )
        .unwrap();
        let list = values.items.values().next().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1], first);
        // A bogus anchor refuses the batch.
        let err = apply_writes(
            &mut values,
            vec![PreviewWrite::AddItem {
                group,
                parent: RowPath::root(),
                before: Some(ItemId::new("missing")),
            }],
        )
        .unwrap_err();
        assert!(matches!(err, PreviewWriteError::UnknownAnchor { .. }));
    }

    #[test]
    fn conformance_refuses_a_mistyped_write() {
        let schema = tree().schema();
        let mut values = RecordValues::default();
        apply_writes(
            &mut values,
            vec![PreviewWrite::Set {
                column: ColumnId::new("name"),
                path: RowPath::root(),
                state: CellState::Value(CellValue::One(Scalar::Boolean(true))),
            }],
        )
        .unwrap();
        let errors = check(&values, &schema, &NomenclatureTable::new());
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ConformanceError::TypeMismatch(_)))
        );
    }

    #[test]
    fn prune_drops_stale_cells_and_dependents() {
        let mut values = RecordValues::default();
        apply_writes(
            &mut values,
            vec![
                set("name", RowPath::root(), "Ada"),
                PreviewWrite::AddItem {
                    group: GroupId::new("kids"),
                    parent: RowPath::root(),
                    before: None,
                },
            ],
        )
        .unwrap();
        let item = values.items.values().next().unwrap()[0].clone();
        apply_writes(
            &mut values,
            vec![set(
                "kid_name",
                RowPath::root().child(PathSeg {
                    group: GroupId::new("kids"),
                    item,
                }),
                "Sam",
            )],
        )
        .unwrap();
        // The tree loses the group: its item list and the cells under
        // it go; the root cell survives.
        let smaller = Tree {
            elements: vec![column("name", ScalarType::Text)],
        };
        prune(&mut values, &smaller.schema(), &NomenclatureTable::new());
        assert!(values.items.is_empty());
        assert_eq!(values.cells.len(), 1);
        assert_eq!(
            values.cells.keys().next().unwrap(),
            &CellAddr {
                column: ColumnId::new("name"),
                path: RowPath::root(),
            }
        );
        // A type change orphans the cell too.
        let retyped = Tree {
            elements: vec![column("name", ScalarType::Boolean)],
        };
        prune(&mut values, &retyped.schema(), &NomenclatureTable::new());
        assert!(values.cells.is_empty());
    }

    #[test]
    fn findings_are_surface_tagged() {
        let tree = Tree {
            elements: vec![
                TreeElement::Column(TreeColumn {
                    id: ColumnId::new("public"),
                    label: "public".into(),
                    ty: ScalarType::Text,
                    arity: Arity::One,
                    required: true,
                    format: None,
                    audience: Audience::All,
                }),
                TreeElement::Column(TreeColumn {
                    id: ColumnId::new("private"),
                    label: "private".into(),
                    ty: ScalarType::Text,
                    arity: Arity::One,
                    required: true,
                    format: None,
                    audience: Audience::Reviewer,
                }),
            ],
        };
        let findings = preview_findings(&tree, &RecordValues::default()).unwrap();
        // The public column is missing on both surfaces; the
        // reviewer-only one on the reviewer surface alone.
        let on = |surface: &str, column: &str| {
            findings
                .iter()
                .filter(|f| {
                    f.surface == surface
                        && matches!(&f.finding, Finding::MissingRequired { column: c, .. }
                            if c.as_str() == column)
                })
                .count()
        };
        assert_eq!(on(APPLICANT_SURFACE, "public"), 1);
        assert_eq!(on(REVIEWER_SURFACE, "public"), 1);
        assert_eq!(on(APPLICANT_SURFACE, "private"), 0);
        assert_eq!(on(REVIEWER_SURFACE, "private"), 1);
    }
}
