//! The total evaluator (§4.1). Two-valued; absence always loses: a
//! source that is absent, empty, or hidden makes every comparison atom
//! **false** and `is_empty` **true**. Negative atoms are independent
//! atoms, not negations — `NotEq(absent, x)` is false, matching DN.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use varve_core::primitives::Decimal;
use varve_core::{ColumnId, ResolverId, RowPath};
use varve_schema::{
    NomenclatureTable, ScalarType, SchemaIndex, Unit, nomenclature_rows,
};
use varve_value::{CellAddr, CellState, CellValue, RecordValues, Scalar};

use crate::{Atom, ColumnRef, Const, Expr, Operand};

pub struct EvalContext<'a> {
    pub index: &'a SchemaIndex,
    pub nomenclatures: &'a NomenclatureTable,
    pub values: &'a RecordValues,
    /// The item being evaluated; root for record scope.
    pub item: RowPath,
    /// Columns currently hidden by the visibility cascade: read as
    /// absent (§4.1 — hidden never contributes; stale values in hidden
    /// columns must not drive visibility).
    pub hidden: BTreeSet<ColumnId>,
    /// Pending resolutions as `(scope, resolver)` — per group instance
    /// (§2.8 rule 3). Supplied by the caller — resolution state lives
    /// beside the record, above this crate's tier. `pending(r)` holds
    /// at `item` iff some pending `(scope, r)` has `scope` a prefix of
    /// `item`: an item sees its own instance's and the record's pending
    /// resolutions, never a sibling item's (the §4.1 scope rule).
    pub pending: PendingSet,
}

/// Pending resolutions keyed by group instance (§2.8): what
/// `varve_record::pending_set` produces.
pub type PendingSet = BTreeSet<(RowPath, ResolverId)>;

fn is_pending(ctx: &EvalContext, resolver: &ResolverId) -> bool {
    ctx.pending
        .iter()
        .any(|(scope, r)| r == resolver && ctx.item.starts_with(scope))
}

/// Total: every expression evaluates to a boolean on every record.
///
/// (Not `eval` in the code-execution sense: this walks the closed
/// predicate AST above — no parsing, no code, no IO; a pure function
/// of the expression and the record.)
pub fn eval(expr: &Expr, ctx: &EvalContext) -> bool {
    match expr {
        Expr::And(operands) => operands.iter().all(|e| eval(e, ctx)),
        Expr::Or(operands) => operands.iter().any(|e| eval(e, ctx)),
        Expr::Atom(atom) => eval_atom(atom, ctx),
    }
}

fn eval_atom(atom: &Atom, ctx: &EvalContext) -> bool {
    match atom {
        Atom::Pending { resolver } => is_pending(ctx, resolver),
        Atom::NotPending { resolver } => !is_pending(ctx, resolver),
        Atom::IsEmpty { source } => read(source, ctx).is_none(),
        Atom::IsFilled { source } => read(source, ctx).is_some(),
        Atom::Contains { source, option } => match read(source, ctx) {
            Some(CellValue::Many(items)) => {
                items.iter().any(|s| matches!(s, Scalar::Enum(id) if id == option))
            }
            _ => false,
        },
        Atom::Excludes { source, option } => match read(source, ctx) {
            Some(CellValue::Many(items)) => {
                !items.iter().any(|s| matches!(s, Scalar::Enum(id) if id == option))
            }
            // Absence loses — an absent list "excludes" nothing.
            _ => false,
        },
        Atom::Eq { source, right } => compare(source, right, ctx)
            .is_some_and(|o| o == Ordering::Equal),
        Atom::NotEq { source, right } => compare(source, right, ctx)
            .is_some_and(|o| o != Ordering::Equal),
        Atom::Lt { source, right } => compare(source, right, ctx)
            .is_some_and(|o| o == Ordering::Less),
        Atom::Le { source, right } => compare(source, right, ctx)
            .is_some_and(|o| o != Ordering::Greater),
        Atom::Gt { source, right } => compare(source, right, ctx)
            .is_some_and(|o| o == Ordering::Greater),
        Atom::Ge { source, right } => compare(source, right, ctx)
            .is_some_and(|o| o != Ordering::Less),
    }
}

/// The stored value a column reference resolves to in this context —
/// `None` for absent, empty, hidden, out-of-scope, or an empty list
/// (§2.4 stored state × §4.1 absence rule).
fn read<'a>(source: &ColumnRef, ctx: &'a EvalContext) -> Option<&'a CellValue> {
    if ctx.hidden.contains(&source.column) {
        return None;
    }
    let info = ctx.index.columns.get(&source.column)?;
    // The cell's path: the rule scope's item path truncated to the
    // column's own scope depth, with matching groups.
    let segments = ctx.item.segments().get(..info.scope.len())?;
    if !segments.iter().map(|s| &s.group).eq(info.scope.iter()) {
        return None;
    }
    let path = segments
        .iter()
        .fold(RowPath::root(), |p, seg| p.child(seg.clone()));
    let state = ctx.values.cells.get(&CellAddr {
        column: source.column.clone(),
        path,
    })?;
    match state {
        CellState::Empty => None,
        CellState::Value(CellValue::Many(items)) if items.is_empty() => None,
        CellState::Value(value) => Some(value),
    }
}

/// Compare a source against its operand: `None` = incomparable in this
/// context (absent source, type drift, unconvertible unit) — the atom
/// then loses.
fn compare(source: &ColumnRef, right: &Operand, ctx: &EvalContext) -> Option<Ordering> {
    // v1 policy: constant side only (typechecker enforces; evaluation
    // stays total regardless).
    let Operand::Const(constant) = right else {
        return None;
    };
    let value = read(source, ctx)?;
    let CellValue::One(scalar) = value else {
        return None;
    };

    if let Some(field) = &source.field {
        // Nomenclature field projection (§2.12): the row's field value
        // compares as text.
        let Scalar::Enum(id) = scalar else {
            return None;
        };
        let ScalarType::Enum(nref) = &ctx.index.columns.get(&source.column)?.ty
        else {
            return None;
        };
        let rows = nomenclature_rows(nref, ctx.nomenclatures).ok()?;
        let row = rows.iter().find(|r| r.id == *id)?;
        let (_, projected) = row.fields.iter().find(|(k, _)| k == field)?;
        let Const::Text(expected) = constant else {
            return None;
        };
        return Some(projected.as_str().cmp(expected.as_str()));
    }

    match (scalar, constant) {
        (Scalar::Boolean(a), Const::Boolean(b)) => Some(a.cmp(b)),
        (Scalar::Integer(a), Const::Number { value, unit }) => {
            let column_unit = number_unit(ctx, &source.column)?;
            compare_numbers(Decimal::from_i64(*a), column_unit, value, *unit)
        }
        (Scalar::Decimal(a), Const::Number { value, unit }) => {
            let column_unit = number_unit(ctx, &source.column)?;
            compare_numbers(a.clone(), column_unit, value, *unit)
        }
        (Scalar::Date(a), Const::Date(b)) => Some(a.cmp(b)),
        (Scalar::Datetime(a), Const::Datetime(b)) => Some(a.cmp(b)),
        (Scalar::Enum(a), Const::Option(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

fn number_unit(ctx: &EvalContext, column: &ColumnId) -> Option<Option<Unit>> {
    match ctx.index.columns.get(column)?.ty {
        ScalarType::Integer(u) | ScalarType::Decimal(u) => Some(u),
        _ => None,
    }
}

/// §2.14: comparison across compatible units happens on exact rationals
/// — total and exact even where a storage cast would fail. Both sides
/// scale to the dimension's base unit (integer factors, exact).
fn compare_numbers(
    a: Decimal,
    a_unit: Option<Unit>,
    b: &Decimal,
    b_unit: Option<Unit>,
) -> Option<Ordering> {
    match (a_unit, b_unit) {
        (None, None) => Some(a.cmp(b)),
        (Some(ua), Some(ub)) if ua.dimension() == ub.dimension() => {
            let a = a.mul_div_exact(ua.factor(), 1)?;
            let b = b.mul_div_exact(ub.factor(), 1)?;
            Some(a.cmp(&b))
        }
        _ => None,
    }
}
