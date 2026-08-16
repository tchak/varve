//! Typed conformance: do a record's values fit a schema?
//!
//! This is *type-level* checking only (§2.6): representability. Nothing
//! here knows about `required`, visibility, or admissibility — those are
//! surface concerns.

use std::collections::{HashMap, HashSet};

use strata_core::{ColumnId, GroupId, NomenclatureId, OptionId};
use strata_schema::{
    Arity, Cardinality, Element, NomenclatureRef, NomenclatureTable, OptionRow,
    ScalarType, Schema,
};

use crate::{CellState, CellValue, ItemsAddr, RecordValues, Scalar};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConformanceError {
    UnknownColumn(ColumnId),
    /// The cell's row path does not match the column's scope — the chain
    /// of `many` groups from the root (§2.2).
    ScopeMismatch(ColumnId),
    /// A path segment names an item absent from its group's item list.
    UnknownItem(ColumnId, GroupId),
    ArityMismatch(ColumnId),
    TypeMismatch(ColumnId),
    /// An enum cell holds an id absent from its nomenclature (§2.11).
    UnknownOption(ColumnId, OptionId),
    /// A published nomenclature is not in the provided table.
    UnknownNomenclature(ColumnId, NomenclatureId),
    /// Duplicate element identity inside one `many` cell (§2.4).
    DuplicateElement(ColumnId),
    UnknownGroup(GroupId),
    /// An item list for a group that is not cardinality `many`, or whose
    /// parent path does not match the group's scope.
    MisplacedItems(GroupId),
    DuplicateItem(GroupId),
}

struct ColumnInfo<'a> {
    ty: &'a ScalarType,
    arity: Arity,
    /// Enclosing `many` groups, root-outward. `one` groups contribute
    /// nothing (§2.5).
    scope: Vec<GroupId>,
}

struct GroupInfo {
    cardinality: Cardinality,
    /// Scope *outside* this group.
    parent_scope: Vec<GroupId>,
}

fn index<'a>(
    elements: &'a [Element],
    scope: &mut Vec<GroupId>,
    columns: &mut HashMap<ColumnId, ColumnInfo<'a>>,
    groups: &mut HashMap<GroupId, GroupInfo>,
) {
    for el in elements {
        match el {
            Element::Column(c) => {
                columns.insert(
                    c.id.clone(),
                    ColumnInfo {
                        ty: &c.ty,
                        arity: c.arity,
                        scope: scope.clone(),
                    },
                );
            }
            Element::Group(g) => {
                groups.insert(
                    g.id.clone(),
                    GroupInfo {
                        cardinality: g.cardinality,
                        parent_scope: scope.clone(),
                    },
                );
                match g.cardinality {
                    Cardinality::Many => {
                        scope.push(g.id.clone());
                        index(&g.children, scope, columns, groups);
                        scope.pop();
                    }
                    Cardinality::One => {
                        index(&g.children, scope, columns, groups);
                    }
                }
            }
        }
    }
}

fn scalar_conforms(
    scalar: &Scalar,
    ty: &ScalarType,
    column: &ColumnId,
    nomenclatures: &NomenclatureTable,
    errors: &mut Vec<ConformanceError>,
) {
    match (scalar, ty) {
        (Scalar::Text(_), ScalarType::Text)
        | (Scalar::Boolean(_), ScalarType::Boolean)
        | (Scalar::Integer(_), ScalarType::Integer)
        | (Scalar::Decimal(_), ScalarType::Decimal)
        | (Scalar::Date(_), ScalarType::Date)
        | (Scalar::Datetime(_), ScalarType::Datetime)
        | (Scalar::Attachment(_), ScalarType::Attachment)
        | (Scalar::Geometry(_), ScalarType::Geometry) => {}
        (Scalar::Enum(option), ScalarType::Enum(nref)) => {
            let rows: Option<&[OptionRow]> = match nref {
                NomenclatureRef::Inline(rows) => Some(rows),
                NomenclatureRef::Published { id, .. } => {
                    match nomenclatures.get(id) {
                        Some(rows) => Some(rows),
                        None => {
                            errors.push(ConformanceError::UnknownNomenclature(
                                column.clone(),
                                id.clone(),
                            ));
                            None
                        }
                    }
                }
            };
            if let Some(rows) = rows
                && !rows.iter().any(|r| r.id == *option)
            {
                errors.push(ConformanceError::UnknownOption(
                    column.clone(),
                    option.clone(),
                ));
            }
        }
        _ => errors.push(ConformanceError::TypeMismatch(column.clone())),
    }
}

/// Check every cell and item list of `values` against `schema`.
pub fn check(
    values: &RecordValues,
    schema: &Schema,
    nomenclatures: &NomenclatureTable,
) -> Vec<ConformanceError> {
    let mut errors = Vec::new();
    let mut columns = HashMap::new();
    let mut groups = HashMap::new();
    index(&schema.root, &mut Vec::new(), &mut columns, &mut groups);

    for (addr, list) in &values.items {
        match groups.get(&addr.group) {
            None => errors.push(ConformanceError::UnknownGroup(addr.group.clone())),
            Some(info) => {
                let parent_groups: Vec<&GroupId> =
                    addr.parent.segments().iter().map(|s| &s.group).collect();
                let scope_ok = info.parent_scope.iter().eq(parent_groups);
                if info.cardinality != Cardinality::Many || !scope_ok {
                    errors.push(ConformanceError::MisplacedItems(addr.group.clone()));
                }
            }
        }
        let mut seen = HashSet::new();
        for item in list {
            if !seen.insert(item) {
                errors.push(ConformanceError::DuplicateItem(addr.group.clone()));
            }
        }
    }

    for (addr, state) in &values.cells {
        let Some(info) = columns.get(&addr.column) else {
            errors.push(ConformanceError::UnknownColumn(addr.column.clone()));
            continue;
        };

        let segments = addr.path.segments();
        let path_groups: Vec<&GroupId> = segments.iter().map(|s| &s.group).collect();
        if !info.scope.iter().eq(path_groups) {
            errors.push(ConformanceError::ScopeMismatch(addr.column.clone()));
            continue;
        }
        for (depth, seg) in segments.iter().enumerate() {
            let parent = strata_core::RowPath::root();
            let parent = segments[..depth]
                .iter()
                .fold(parent, |p, s| p.child(s.clone()));
            let items_addr = ItemsAddr {
                group: seg.group.clone(),
                parent,
            };
            let exists = values
                .items
                .get(&items_addr)
                .is_some_and(|list| list.contains(&seg.item));
            if !exists {
                errors.push(ConformanceError::UnknownItem(
                    addr.column.clone(),
                    seg.group.clone(),
                ));
            }
        }

        let CellState::Value(value) = state else {
            // Empty conforms to every column: blank is not a type error,
            // and required-ness is not a schema concept (§2.6).
            continue;
        };
        match (value, info.arity) {
            (CellValue::One(scalar), Arity::One) => {
                scalar_conforms(scalar, info.ty, &addr.column, nomenclatures, &mut errors);
            }
            (CellValue::Many(scalars), Arity::Many) => {
                let mut seen = HashSet::new();
                for scalar in scalars {
                    scalar_conforms(
                        scalar,
                        info.ty,
                        &addr.column,
                        nomenclatures,
                        &mut errors,
                    );
                    if let Some(id) = scalar.element_id()
                        && !seen.insert(id.to_string())
                    {
                        errors.push(ConformanceError::DuplicateElement(
                            addr.column.clone(),
                        ));
                    }
                }
            }
            _ => errors.push(ConformanceError::ArityMismatch(addr.column.clone())),
        }
    }

    errors
}
