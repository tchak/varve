//! Static checking against a revision (§4.1): the conditionability
//! matrix, scope rules, enum membership, unit dimensions. Errors mirror
//! DN's rule-error taxonomy and feed `varve-impact`'s broken-rule
//! analysis.

use varve_core::{ColumnId, GroupId, OptionId, ResolverId};
use varve_schema::{
    Arity, ColumnInfo, NomenclatureTable, ScalarType, Schema, SchemaIndex, Unit,
    nomenclature_rows,
};

use crate::{Atom, ColumnRef, Const, Expr, Operand};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum TypeError {
    #[error("unknown column '{0}'")]
    UnknownColumn(ColumnId),
    /// §4.1 scope rule: an item-scoped rule reads its own item plus
    /// record columns; a record-scoped rule reads record columns only.
    #[error("column '{0}' is out of scope for this rule")]
    ScopeViolation(ColumnId),
    /// The conditionability matrix (§4.1): e.g. comparisons on raw text.
    #[error("column '{0}': this atom is not allowed for its type")]
    AtomNotAllowed(ColumnId),
    #[error("column '{0}': constant type does not match the column")]
    TypeMismatch(ColumnId),
    #[error("column '{0}': option '{1}' is not in the nomenclature")]
    UnknownOption(ColumnId, OptionId),
    #[error("column '{0}': no row of its nomenclature carries field '{1}'")]
    UnknownField(ColumnId, String),
    #[error("column '{0}': unit dimensions do not match")]
    UnitMismatch(ColumnId),
    #[error("unknown resolver '{0}'")]
    UnknownResolver(ResolverId),
    /// Representable, not yet enabled — publication-time policy (§4.3).
    #[error("column-to-column comparisons are not enabled")]
    ColumnComparisonNotEnabled,
    #[error("column '{0}': published nomenclature not provided")]
    UnknownNomenclature(ColumnId),
}

/// Check `expr` attached at `scope` (empty = record scope) against a
/// schema.
pub fn typecheck(
    expr: &Expr,
    schema: &Schema,
    nomenclatures: &NomenclatureTable,
    scope: &[GroupId],
) -> Vec<TypeError> {
    let index = SchemaIndex::build(schema);
    let mut errors = Vec::new();
    walk(expr, schema, &index, nomenclatures, scope, &mut errors);
    errors
}

fn walk(
    expr: &Expr,
    schema: &Schema,
    index: &SchemaIndex,
    nomenclatures: &NomenclatureTable,
    scope: &[GroupId],
    errors: &mut Vec<TypeError>,
) {
    match expr {
        Expr::And(operands) | Expr::Or(operands) => {
            for operand in operands {
                walk(operand, schema, index, nomenclatures, scope, errors);
            }
        }
        Expr::Atom(atom) => check_atom(atom, schema, index, nomenclatures, scope, errors),
    }
}

fn check_atom(
    atom: &Atom,
    schema: &Schema,
    index: &SchemaIndex,
    nomenclatures: &NomenclatureTable,
    scope: &[GroupId],
    errors: &mut Vec<TypeError>,
) {
    if let Atom::Pending { resolver } | Atom::NotPending { resolver } = atom {
        if !schema.resolvers.iter().any(|r| r.id == *resolver) {
            errors.push(TypeError::UnknownResolver(resolver.clone()));
        }
        return;
    }
    let source = atom.source().expect("non-pending atoms have a source");
    let Some(info) = index.columns.get(&source.column) else {
        errors.push(TypeError::UnknownColumn(source.column.clone()));
        return;
    };
    // Scope: the source's scope must be a prefix of the rule's scope.
    if !scope.starts_with(&info.scope) {
        errors.push(TypeError::ScopeViolation(source.column.clone()));
    }
    if atom.right_column().is_some() {
        errors.push(TypeError::ColumnComparisonNotEnabled);
        return;
    }

    match atom {
        Atom::IsEmpty { .. } | Atom::IsFilled { .. } => {}
        Atom::Contains { option, .. } | Atom::Excludes { option, .. } => {
            match (&info.ty, info.arity) {
                (ScalarType::Enum(nref), Arity::Many) => {
                    check_option(&source.column, nref, option, nomenclatures, errors);
                }
                _ => errors.push(TypeError::AtomNotAllowed(source.column.clone())),
            }
        }
        Atom::Eq { right, .. } | Atom::NotEq { right, .. } => {
            check_comparison(source, info, right, nomenclatures, false, errors);
        }
        Atom::Lt { right, .. }
        | Atom::Le { right, .. }
        | Atom::Gt { right, .. }
        | Atom::Ge { right, .. } => {
            check_comparison(source, info, right, nomenclatures, true, errors);
        }
        Atom::Pending { .. } | Atom::NotPending { .. } => unreachable!(),
    }
}

fn check_comparison(
    source: &ColumnRef,
    info: &ColumnInfo,
    right: &Operand,
    nomenclatures: &NomenclatureTable,
    ordering: bool,
    errors: &mut Vec<TypeError>,
) {
    let Operand::Const(constant) = right else {
        return; // already reported as ColumnComparisonNotEnabled
    };
    let column = &source.column;

    // Field projection: enum column, field must exist on some row,
    // compares eq/not_eq against text (§4.1 — the dissolved geo
    // operators).
    if let Some(field) = &source.field {
        let ScalarType::Enum(nref) = &info.ty else {
            errors.push(TypeError::AtomNotAllowed(column.clone()));
            return;
        };
        if ordering || !matches!(constant, Const::Text(_)) {
            errors.push(TypeError::AtomNotAllowed(column.clone()));
        }
        match nomenclature_rows(nref, nomenclatures) {
            Err(_) => errors.push(TypeError::UnknownNomenclature(column.clone())),
            Ok(rows) => {
                if !rows.iter().any(|r| r.fields.iter().any(|(k, _)| k == field)) {
                    errors.push(TypeError::UnknownField(column.clone(), field.clone()));
                }
            }
        }
        return;
    }

    match (&info.ty, constant) {
        // The §4.1 conditionability matrix.
        (ScalarType::Boolean, Const::Boolean(_)) if !ordering => {}
        (ScalarType::Boolean, _) => errors.push(TypeError::AtomNotAllowed(column.clone())),
        (ScalarType::Integer(u) | ScalarType::Decimal(u), Const::Number { unit, .. }) => {
            if !units_compatible(*u, *unit) {
                errors.push(TypeError::UnitMismatch(column.clone()));
            }
        }
        (ScalarType::Date, Const::Date(_)) => {}
        (ScalarType::Datetime, Const::Datetime(_)) => {}
        (ScalarType::Enum(nref), Const::Option(option)) if !ordering => {
            check_option(column, nref, option, nomenclatures, errors);
        }
        (
            ScalarType::Text
            | ScalarType::Attachment
            | ScalarType::Geometry
            | ScalarType::Enum(_),
            _,
        ) => errors.push(TypeError::AtomNotAllowed(column.clone())),
        _ => errors.push(TypeError::TypeMismatch(column.clone())),
    }
}

fn units_compatible(column: Option<Unit>, constant: Option<Unit>) -> bool {
    match (column, constant) {
        (None, None) => true,
        (Some(a), Some(b)) => a.dimension() == b.dimension(),
        _ => false,
    }
}

fn check_option(
    column: &ColumnId,
    nref: &varve_schema::NomenclatureRef,
    option: &OptionId,
    nomenclatures: &NomenclatureTable,
    errors: &mut Vec<TypeError>,
) {
    match nomenclature_rows(nref, nomenclatures) {
        Err(_) => errors.push(TypeError::UnknownNomenclature(column.clone())),
        Ok(rows) => {
            if !rows.iter().any(|r| r.id == *option) {
                errors.push(TypeError::UnknownOption(column.clone(), option.clone()));
            }
        }
    }
}
