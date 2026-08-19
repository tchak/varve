//! The acyclicity check (§4.1): the kernel invariant behind DN's
//! "previous in the tree" editor rule. Checked at publication like the
//! depth policy — an error with a message, never a type. Evaluation
//! order is the returned topological order; a hidden source reads as
//! absent, so cascades are deterministic.

use std::collections::BTreeMap;

use varve_core::ColumnId;

use crate::{Expr, sources};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("visibility rules form a cycle: {}", cycle.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(" → "))]
pub struct RuleCycle {
    pub cycle: Vec<ColumnId>,
}

/// Check the dependency graph (column → its rule's sources) is acyclic;
/// return a topological evaluation order over the ruled columns.
pub fn check_acyclic(rules: &BTreeMap<ColumnId, Expr>) -> Result<Vec<ColumnId>, RuleCycle> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Unvisited,
        InProgress,
        Done,
    }
    let mut marks: BTreeMap<&ColumnId, Mark> = rules.keys().map(|c| (c, Mark::Unvisited)).collect();
    let mut order = Vec::new();

    fn visit<'a>(
        column: &'a ColumnId,
        rules: &'a BTreeMap<ColumnId, Expr>,
        marks: &mut BTreeMap<&'a ColumnId, Mark>,
        order: &mut Vec<ColumnId>,
        stack: &mut Vec<ColumnId>,
    ) -> Result<(), RuleCycle> {
        match marks.get(column).copied() {
            None | Some(Mark::Done) => return Ok(()),
            Some(Mark::InProgress) => {
                let start = stack.iter().position(|c| c == column).unwrap_or(0);
                let mut cycle = stack[start..].to_vec();
                cycle.push(column.clone());
                return Err(RuleCycle { cycle });
            }
            Some(Mark::Unvisited) => {}
        }
        marks.insert(column, Mark::InProgress);
        stack.push(column.clone());
        if let Some(expr) = rules.get(column) {
            for source in sources(expr) {
                if let Some((key, _)) = rules.get_key_value(&source) {
                    visit(key, rules, marks, order, stack)?;
                }
            }
        }
        stack.pop();
        marks.insert(column, Mark::Done);
        order.push(column.clone());
        Ok(())
    }

    let keys: Vec<&ColumnId> = rules.keys().collect();
    let mut stack = Vec::new();
    for column in keys {
        visit(column, rules, &mut marks, &mut order, &mut stack)?;
    }
    Ok(order)
}
