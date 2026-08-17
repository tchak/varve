//! Admissibility (§2.6): a record is never globally invalid — it is
//! non-admissible **with respect to a surface**. Findings: required
//! cells missing (requiredness is a rule, so "required unless pending"
//! is expressible — §2.8 rule 3), format violations on filled text,
//! and ineligibility.

use std::collections::BTreeSet;

use varve_core::{ColumnId, RowPath};
// `eval` is varve-logic's pure predicate-AST evaluator (no code
// execution — see its definition).
use varve_logic::{EvalContext, PendingSet, RuleCycle, eval};
use varve_schema::{NomenclatureTable, Schema, SchemaIndex};
use varve_value::{CellAddr, CellState, CellValue, RecordValues, Scalar};

use crate::reach::paths_for_scope;
use crate::{Format, Surface, column_entries, reachability};

#[derive(Debug, Clone, PartialEq)]
pub enum Finding {
    /// Reachable, required by its rule, and not filled.
    MissingRequired { column: ColumnId, path: RowPath },
    /// Filled text cell violating the surface's format constraint
    /// (§2.6): non-admissible, never ill-typed.
    FormatViolation {
        column: ColumnId,
        path: RowPath,
        format: Format,
    },
    /// The submission surface's record-scoped predicate holds (§4.1).
    Ineligible { message: String },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdmissibilityReport {
    pub findings: Vec<Finding>,
}

impl AdmissibilityReport {
    pub fn is_admissible(&self) -> bool {
        self.findings.is_empty()
    }
}

pub fn admissibility(
    surface: &Surface,
    schema: &Schema,
    nomenclatures: &NomenclatureTable,
    values: &RecordValues,
    pending: &PendingSet,
) -> Result<AdmissibilityReport, RuleCycle> {
    let index = SchemaIndex::build(schema);
    let reach = reachability(surface, schema, nomenclatures, values, pending)?;
    let mut findings = Vec::new();

    for entry in column_entries(surface) {
        let column = &entry.node.column;
        let Some(info) = index.columns.get(column) else {
            continue;
        };
        for path in paths_for_scope(&info.scope, values) {
            if !reach.is_visible(column, &path) {
                continue;
            }
            let hidden_here: BTreeSet<ColumnId> = reach
                .hidden
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
            let cell = values.cells.get(&CellAddr {
                column: column.clone(),
                path: path.clone(),
            });
            let filled = match cell {
                Some(CellState::Value(CellValue::One(_))) => true,
                Some(CellState::Value(CellValue::Many(items))) => !items.is_empty(),
                _ => false,
            };
            if let Some(required) = &entry.node.required
                && eval(required, &ctx)
                && !filled
            {
                findings.push(Finding::MissingRequired {
                    column: column.clone(),
                    path: path.clone(),
                });
            }
            if let Some(format) = &entry.node.format
                && let Some(CellState::Value(CellValue::One(Scalar::Text(text)))) = cell
                && !text.is_empty()
                && !format.check(text)
            {
                findings.push(Finding::FormatViolation {
                    column: column.clone(),
                    path: path.clone(),
                    format: format.clone(),
                });
            }
        }
    }

    if let Some(ineligibility) = &surface.ineligibility {
        let ctx = EvalContext {
            index: &index,
            nomenclatures,
            values,
            item: RowPath::root(),
            hidden: reach
                .hidden
                .iter()
                .filter(|(_, hp)| hp.is_root())
                .map(|(c, _)| c.clone())
                .collect(),
            pending: pending.clone(),
        };
        if eval(&ineligibility.rule, &ctx) {
            findings.push(Finding::Ineligible {
                message: ineligibility.message.clone(),
            });
        }
    }

    Ok(AdmissibilityReport { findings })
}
