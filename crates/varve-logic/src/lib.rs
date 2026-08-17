//! Tier 2 (§7): the logic language — §4 of DESIGN.md.
//!
//! Pure, total, no recursion in evaluation beyond the expression tree.
//! Predicates only in v1; computed values (§4.2) and the satisfiability
//! solver (§4.3) follow. Atoms are two-valued: **absence always loses**
//! (§4.1) — a source that is absent, empty, or hidden makes every
//! comparison atom false and `is_empty` true. There is no general `Not`
//! combinator, by decision, not omission.

#![forbid(unsafe_code)]

mod eval;
mod graph;
mod typecheck;

pub use eval::{EvalContext, eval};
pub use graph::{RuleCycle, check_acyclic};
pub use typecheck::{TypeError, typecheck};

use std::collections::BTreeSet;

use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, OptionId, ResolverId};
use varve_schema::Unit;

/// Arbitrarily nestable (§4.1 — settled from institutional memory; DN's
/// one-level and/or imports as the degenerate case).
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Atom(Atom),
}

/// A column reference, optionally projected through a nomenclature
/// extra field (§2.12) — DN's geo operators, dissolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnRef {
    pub column: ColumnId,
    pub field: Option<String>,
}

/// Typed literal. Number constants carry units (§2.14).
#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Boolean(bool),
    Number { value: Decimal, unit: Option<Unit> },
    Date(Date),
    Datetime(Instant),
    Option(OptionId),
    /// Only against nomenclature field projections — raw text columns
    /// are presence-only (§4.1 conditionability matrix).
    Text(String),
}

/// The right side of a comparison. `Column` is representable but
/// rejected by the v1 typechecker — a publication-time policy with a
/// documented relaxation path (§4.3), not a grammar restriction.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Const(Const),
    Column(ColumnRef),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Atom {
    Eq { source: ColumnRef, right: Operand },
    NotEq { source: ColumnRef, right: Operand },
    Lt { source: ColumnRef, right: Operand },
    Le { source: ColumnRef, right: Operand },
    Gt { source: ColumnRef, right: Operand },
    Ge { source: ColumnRef, right: Operand },
    IsEmpty { source: ColumnRef },
    IsFilled { source: ColumnRef },
    /// Arity-`many` enum membership.
    Contains { source: ColumnRef, option: OptionId },
    Excludes { source: ColumnRef, option: OptionId },
    /// §2.8 rule 3: resolution status is readable.
    Pending { resolver: ResolverId },
}

impl Atom {
    fn source(&self) -> Option<&ColumnRef> {
        match self {
            Atom::Eq { source, .. }
            | Atom::NotEq { source, .. }
            | Atom::Lt { source, .. }
            | Atom::Le { source, .. }
            | Atom::Gt { source, .. }
            | Atom::Ge { source, .. }
            | Atom::IsEmpty { source }
            | Atom::IsFilled { source }
            | Atom::Contains { source, .. }
            | Atom::Excludes { source, .. } => Some(source),
            Atom::Pending { .. } => None,
        }
    }

    fn right_column(&self) -> Option<&ColumnRef> {
        match self {
            Atom::Eq { right, .. }
            | Atom::NotEq { right, .. }
            | Atom::Lt { right, .. }
            | Atom::Le { right, .. }
            | Atom::Gt { right, .. }
            | Atom::Ge { right, .. } => match right {
                Operand::Column(c) => Some(c),
                Operand::Const(_) => None,
            },
            _ => None,
        }
    }
}

/// Every column an expression reads — the input to the acyclicity check
/// and to `varve-impact`'s broken-rule analysis.
pub fn sources(expr: &Expr) -> BTreeSet<ColumnId> {
    let mut out = BTreeSet::new();
    collect_sources(expr, &mut out);
    out
}

fn collect_sources(expr: &Expr, out: &mut BTreeSet<ColumnId>) {
    match expr {
        Expr::And(operands) | Expr::Or(operands) => {
            for operand in operands {
                collect_sources(operand, out);
            }
        }
        Expr::Atom(atom) => {
            if let Some(source) = atom.source() {
                out.insert(source.column.clone());
            }
            if let Some(right) = atom.right_column() {
                out.insert(right.column.clone());
            }
        }
    }
}

/// The resolvers an expression's `pending` atoms read.
pub fn resolver_sources(expr: &Expr) -> BTreeSet<ResolverId> {
    let mut out = BTreeSet::new();
    fn walk(expr: &Expr, out: &mut BTreeSet<ResolverId>) {
        match expr {
            Expr::And(operands) | Expr::Or(operands) => {
                for operand in operands {
                    walk(operand, out);
                }
            }
            Expr::Atom(Atom::Pending { resolver }) => {
                out.insert(resolver.clone());
            }
            Expr::Atom(_) => {}
        }
    }
    walk(expr, &mut out);
    out
}
