//! Typed conformance: do a record's values fit a schema?
//!
//! This is *type-level* checking only (§2.6): representability. Nothing
//! here knows about `required`, visibility, or admissibility — those are
//! surface concerns.

use std::collections::HashSet;

use varve_core::{ColumnId, GroupId, NomenclatureId, OptionId};
use varve_schema::{
    Arity, Cardinality, NomenclatureRef, NomenclatureTable, OptionRow,
    ScalarType, Schema, SchemaIndex,
};

use crate::{CellState, CellValue, ItemsAddr, RecordValues, Scalar};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConformanceError {
    #[error("unknown column '{0}'")]
    UnknownColumn(ColumnId),
    /// The cell's row path does not match the column's scope — the chain
    /// of `many` groups from the root (§2.2).
    #[error("cell for column '{0}' is addressed outside the column's scope")]
    ScopeMismatch(ColumnId),
    /// A path segment names an item absent from its group's item list.
    #[error("cell for column '{0}' names an item absent from group '{1}'")]
    UnknownItem(ColumnId, GroupId),
    #[error("column '{0}': value arity does not match the column arity")]
    ArityMismatch(ColumnId),
    #[error("column '{0}': value does not match the column type")]
    TypeMismatch(ColumnId),
    /// An enum cell holds an id absent from its nomenclature (§2.11).
    #[error("column '{0}': option '{1}' is not in the nomenclature")]
    UnknownOption(ColumnId, OptionId),
    /// A published nomenclature is not in the provided table.
    #[error("column '{0}': published nomenclature '{1}' not provided")]
    UnknownNomenclature(ColumnId, NomenclatureId),
    /// Duplicate element identity inside one `many` cell (§2.4).
    #[error("column '{0}': duplicate element identity in a `many` cell")]
    DuplicateElement(ColumnId),
    #[error("unknown group '{0}'")]
    UnknownGroup(GroupId),
    /// An item list for a group that is not cardinality `many`, or whose
    /// parent path does not match the group's scope.
    #[error("item list for group '{0}' is misplaced (not `many`, or wrong scope)")]
    MisplacedItems(GroupId),
    #[error("duplicate item in group '{0}'")]
    DuplicateItem(GroupId),
    /// §2.15: the cell's claimed content type violates the column's
    /// accept set.
    #[error("column '{0}': content type '{1}' is not accepted")]
    AttachmentTypeNotAccepted(ColumnId, String),
    #[error("column '{0}': file exceeds the size limit")]
    AttachmentTooLarge(ColumnId),
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
        | (Scalar::Integer(_), ScalarType::Integer(_))
        | (Scalar::Decimal(_), ScalarType::Decimal(_))
        | (Scalar::Date(_), ScalarType::Date)
        | (Scalar::Datetime(_), ScalarType::Datetime)
        | (Scalar::Geometry(_), ScalarType::Geometry) => {}
        // §2.15: check the claims (type, size) against the schema
        // constraints — zero IO; the Tier 5 store verifies claims
        // against bytes at ingest.
        (Scalar::Attachment(a), ScalarType::Attachment(constraints)) => {
            if !constraints.accepts(&a.content_type) {
                errors.push(ConformanceError::AttachmentTypeNotAccepted(
                    column.clone(),
                    a.content_type.clone(),
                ));
            }
            if !constraints.admits_size(a.byte_size) {
                errors.push(ConformanceError::AttachmentTooLarge(column.clone()));
            }
        }
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
    let index = SchemaIndex::build(schema);

    for (addr, list) in &values.items {
        match index.groups.get(&addr.group) {
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
        let Some(info) = index.columns.get(&addr.column) else {
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
            let parent = varve_core::RowPath::root();
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
                scalar_conforms(scalar, &info.ty, &addr.column, nomenclatures, &mut errors);
            }
            (CellValue::Many(scalars), Arity::Many) => {
                let mut seen = HashSet::new();
                for scalar in scalars {
                    scalar_conforms(
                        scalar,
                        &info.ty,
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
