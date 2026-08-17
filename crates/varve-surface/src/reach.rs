//! Derived reachability (§2.4): surface-relative, computed at read
//! time, never stored. Effective visibility rules evaluate in
//! topological order; a hidden source reads as absent, so cascades are
//! deterministic (§4.1). Item-scoped columns evaluate per item.

use std::collections::{BTreeMap, BTreeSet};

use varve_core::{ColumnId, GroupId, ItemId, PathSeg, ResolverId, RowPath};
// `eval` is varve-logic's pure predicate-AST evaluator (no code
// execution — see its definition).
use varve_logic::{EvalContext, Expr, RuleCycle, check_acyclic, eval};
use varve_schema::{NomenclatureTable, Schema, SchemaIndex};
use varve_value::{ItemsAddr, RecordValues};

use crate::{Surface, column_entries};

/// Which `(column, path)` pairs this surface hides on this record.
/// Everything else the surface presents is reachable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reachability {
    pub hidden: BTreeSet<(ColumnId, RowPath)>,
}

impl Reachability {
    pub fn is_visible(&self, column: &ColumnId, path: &RowPath) -> bool {
        !self.hidden.contains(&(column.clone(), path.clone()))
    }
}

/// Every item path a scope chain expands to on this record (depth-N
/// ready; one hop at depth 1).
pub(crate) fn paths_for_scope(
    scope: &[GroupId],
    values: &RecordValues,
) -> Vec<RowPath> {
    let mut paths = vec![RowPath::root()];
    for group in scope {
        let mut next = Vec::new();
        for parent in paths {
            let addr = ItemsAddr { group: group.clone(), parent: parent.clone() };
            if let Some(items) = values.items.get(&addr) {
                for item in items {
                    next.push(parent.child(PathSeg {
                        group: group.clone(),
                        item: ItemId::new(item.as_str()),
                    }));
                }
            }
        }
        paths = next;
    }
    paths
}

pub fn reachability(
    surface: &Surface,
    schema: &Schema,
    nomenclatures: &NomenclatureTable,
    values: &RecordValues,
    pending: &BTreeSet<ResolverId>,
) -> Result<Reachability, RuleCycle> {
    let index = SchemaIndex::build(schema);
    let entries = column_entries(surface);
    let mut rules: BTreeMap<ColumnId, Expr> = BTreeMap::new();
    for entry in &entries {
        if let Some(rule) = entry.effective_visibility() {
            rules.insert(entry.node.column.clone(), rule);
        }
    }
    let order = check_acyclic(&rules)?;

    let mut hidden: BTreeSet<(ColumnId, RowPath)> = BTreeSet::new();
    for column in order {
        let Some(info) = index.columns.get(&column) else {
            continue; // structural errors are validate()'s business
        };
        let rule = &rules[&column];
        for path in paths_for_scope(&info.scope, values) {
            // The hidden set relevant at `path`: every (c, hp) whose hp
            // is the truncation read() would use — i.e. a prefix of
            // `path` (root included).
            let hidden_here: BTreeSet<ColumnId> = hidden
                .iter()
                .filter(|(_, hp)| path.starts_with(hp))
                .map(|(c, _)| c.clone())
                .collect();
            let ctx = EvalContext {
                index: &index,
                nomenclatures,
                values,
                item: path.clone(),
                hidden: hidden_here,
                pending: pending.clone(),
            };
            if !eval(rule, &ctx) {
                hidden.insert((column.clone(), path));
            }
        }
    }
    Ok(Reachability { hidden })
}
