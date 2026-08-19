//! Structural diff and patch: the five ops of §5/§2.9, one apply
//! function. The log entry, the export, the migration stream and the
//! diff share this representation.

use varve_core::{ColumnId, GroupId, ItemId, PathSeg, RowPath};

use crate::{CellAddr, CellState, CellValue, ItemsAddr, RecordValues};

/// The op set (§5). A snapshot export never uses more than `Set` and
/// `AddItem`.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
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
        item: ItemId,
        at: usize,
    },
    /// Removes the item and everything under it: its cells and any nested
    /// item lists.
    RemoveItem {
        group: GroupId,
        parent: RowPath,
        item: ItemId,
    },
    /// `order` must be a permutation of the current list.
    Reorder {
        group: GroupId,
        parent: RowPath,
        order: Vec<ItemId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error("item '{1}' already exists in group '{0}'")]
    ItemExists(GroupId, ItemId),
    #[error("item '{1}' not found in group '{0}'")]
    UnknownItem(GroupId, ItemId),
    #[error("index {1} out of bounds in group '{0}'")]
    BadIndex(GroupId, usize),
    /// Reorder is not a permutation of the existing list.
    #[error("reorder of group '{0}' is not a permutation of its items")]
    BadReorder(GroupId),
    /// A `many` cell with zero elements is `Empty` — one state, one
    /// encoding (§2.4); a zero-length list is never stored.
    #[error("column '{0}': a zero-length list is not a value — use `Empty`")]
    EmptyList(ColumnId),
}

/// Apply one op. Errors leave `values` unchanged.
pub fn apply(values: &mut RecordValues, op: &Op) -> Result<(), ApplyError> {
    match op {
        Op::Set {
            column,
            path,
            state,
        } => {
            if matches!(state, CellState::Value(CellValue::Many(list)) if list.is_empty()) {
                return Err(ApplyError::EmptyList(column.clone()));
            }
            values.cells.insert(
                CellAddr {
                    column: column.clone(),
                    path: path.clone(),
                },
                state.clone(),
            );
            Ok(())
        }
        Op::Unset { column, path } => {
            values.cells.remove(&CellAddr {
                column: column.clone(),
                path: path.clone(),
            });
            Ok(())
        }
        Op::AddItem {
            group,
            parent,
            item,
            at,
        } => {
            let addr = ItemsAddr {
                group: group.clone(),
                parent: parent.clone(),
            };
            // Validate before touching the map: an error must not leave
            // an empty item list behind (no empty lists are ever stored).
            let len = values.items.get(&addr).map_or(0, Vec::len);
            if values.items.get(&addr).is_some_and(|l| l.contains(item)) {
                return Err(ApplyError::ItemExists(group.clone(), item.clone()));
            }
            if *at > len {
                return Err(ApplyError::BadIndex(group.clone(), *at));
            }
            values
                .items
                .entry(addr)
                .or_default()
                .insert(*at, item.clone());
            Ok(())
        }
        Op::RemoveItem {
            group,
            parent,
            item,
        } => {
            let addr = ItemsAddr {
                group: group.clone(),
                parent: parent.clone(),
            };
            let Some(list) = values.items.get_mut(&addr) else {
                return Err(ApplyError::UnknownItem(group.clone(), item.clone()));
            };
            let Some(pos) = list.iter().position(|i| i == item) else {
                return Err(ApplyError::UnknownItem(group.clone(), item.clone()));
            };
            list.remove(pos);
            if list.is_empty() {
                values.items.remove(&addr);
            }
            let prefix = parent.child(PathSeg {
                group: group.clone(),
                item: item.clone(),
            });
            values
                .cells
                .retain(|addr, _| !addr.path.starts_with(&prefix));
            values
                .items
                .retain(|addr, _| !addr.parent.starts_with(&prefix));
            Ok(())
        }
        Op::Reorder {
            group,
            parent,
            order,
        } => {
            let addr = ItemsAddr {
                group: group.clone(),
                parent: parent.clone(),
            };
            let Some(list) = values.items.get_mut(&addr) else {
                return Err(ApplyError::BadReorder(group.clone()));
            };
            // A permutation: same length, same elements, no repeats.
            let mut a = list.clone();
            let mut b = order.clone();
            a.sort();
            b.sort();
            let repeats = b.windows(2).any(|w| w[0] == w[1]);
            if a != b || repeats {
                return Err(ApplyError::BadReorder(group.clone()));
            }
            *list = order.clone();
            Ok(())
        }
    }
}

/// Compute ops such that applying them to `from` yields `to` exactly.
///
/// Op ordering: item removals (deepest first, cascading their cells),
/// cell unsets, item adds (shallowest first), reorders, cell sets.
pub fn diff(from: &RecordValues, to: &RecordValues) -> Vec<Op> {
    let mut removes = Vec::new();
    let mut adds = Vec::new();
    let mut reorders = Vec::new();

    // Items present in `from`, gone or changed in `to`.
    for (addr, from_list) in &from.items {
        let to_list = to.items.get(addr);
        for item in from_list {
            if !to_list.is_some_and(|l| l.contains(item)) {
                removes.push(Op::RemoveItem {
                    group: addr.group.clone(),
                    parent: addr.parent.clone(),
                    item: item.clone(),
                });
            }
        }
    }
    for (addr, to_list) in &to.items {
        let from_list = from.items.get(addr);
        let mut simulated: Vec<ItemId> = from_list
            .map(|l| l.iter().filter(|i| to_list.contains(i)).cloned().collect())
            .unwrap_or_default();
        for (index, item) in to_list.iter().enumerate() {
            if !simulated.contains(item) {
                let at = index.min(simulated.len());
                simulated.insert(at, item.clone());
                adds.push(Op::AddItem {
                    group: addr.group.clone(),
                    parent: addr.parent.clone(),
                    item: item.clone(),
                    at,
                });
            }
        }
        if simulated != *to_list {
            reorders.push(Op::Reorder {
                group: addr.group.clone(),
                parent: addr.parent.clone(),
                order: to_list.clone(),
            });
        }
    }

    // Deepest removals first so cascades never mask each other; shallow
    // adds first so parents exist before children.
    removes.sort_by_key(|op| match op {
        Op::RemoveItem { parent, .. } => usize::MAX - parent.depth(),
        _ => 0,
    });
    adds.sort_by_key(|op| match op {
        Op::AddItem { parent, .. } => parent.depth(),
        _ => 0,
    });

    let removed_prefixes: Vec<RowPath> = removes
        .iter()
        .filter_map(|op| match op {
            Op::RemoveItem {
                group,
                parent,
                item,
            } => Some(parent.child(PathSeg {
                group: group.clone(),
                item: item.clone(),
            })),
            _ => None,
        })
        .collect();

    let mut unsets = Vec::new();
    for addr in from.cells.keys() {
        if to.cells.contains_key(addr) {
            continue;
        }
        // Cells under a removed item go with the removal.
        if removed_prefixes.iter().any(|p| addr.path.starts_with(p)) {
            continue;
        }
        unsets.push(Op::Unset {
            column: addr.column.clone(),
            path: addr.path.clone(),
        });
    }

    let mut sets = Vec::new();
    for (addr, state) in &to.cells {
        if from.cells.get(addr) != Some(state) {
            sets.push(Op::Set {
                column: addr.column.clone(),
                path: addr.path.clone(),
                state: state.clone(),
            });
        }
    }

    let mut ops = removes;
    ops.extend(unsets);
    ops.extend(adds);
    ops.extend(reorders);
    ops.extend(sets);
    ops
}
