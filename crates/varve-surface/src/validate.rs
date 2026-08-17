//! Structural validation of a surface against its revision's schema:
//! placement, rule typechecking at the right scope, format constraints
//! on text only, and acyclicity of the effective visibility graph.

use std::collections::{BTreeMap, BTreeSet};

use varve_core::{ColumnId, GroupId};
use varve_logic::{RuleCycle, TypeError, check_acyclic, typecheck};
use varve_schema::{Cardinality, NomenclatureTable, ScalarType, Schema, SchemaIndex};

use crate::{Node, Surface, column_entries};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SurfaceError {
    #[error("unknown column '{0}'")]
    UnknownColumn(ColumnId),
    #[error("unknown group '{0}'")]
    UnknownGroup(GroupId),
    #[error("column '{0}' appears more than once")]
    DuplicateColumn(ColumnId),
    /// A column node must sit inside the surface nodes of exactly the
    /// `many` groups that form its schema scope.
    #[error("column '{0}' is placed outside its scope")]
    MisplacedColumn(ColumnId),
    #[error("group '{0}' is placed outside its scope")]
    MisplacedGroup(GroupId),
    /// §2.6: format constraints apply to text columns only.
    #[error("column '{0}': format constraint on a non-text column")]
    FormatOnNonText(ColumnId),
    #[error("rule on column '{0}': {1}")]
    Rule(ColumnId, TypeError),
    #[error("rule on group '{0}': {1}")]
    GroupRule(GroupId, TypeError),
    #[error("section rule: {0}")]
    SectionRule(TypeError),
    #[error("ineligibility rule: {0}")]
    IneligibilityRule(TypeError),
    #[error(transparent)]
    Cycle(RuleCycle),
}

pub fn validate(
    surface: &Surface,
    schema: &Schema,
    nomenclatures: &NomenclatureTable,
) -> Vec<SurfaceError> {
    let index = SchemaIndex::build(schema);
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    walk(
        &surface.nodes,
        schema,
        &index,
        nomenclatures,
        &mut Vec::new(),
        &mut seen,
        &mut errors,
    );

    if let Some(ineligibility) = &surface.ineligibility {
        for error in typecheck(&ineligibility.rule, schema, nomenclatures, &[]) {
            errors.push(SurfaceError::IneligibilityRule(error));
        }
    }

    // Acyclicity of the effective visibility graph (§4.1): checked at
    // publication like the depth policy.
    let mut rules = BTreeMap::new();
    for entry in column_entries(surface) {
        if let Some(rule) = entry.effective_visibility() {
            rules.insert(entry.node.column.clone(), rule);
        }
    }
    if let Err(cycle) = check_acyclic(&rules) {
        errors.push(SurfaceError::Cycle(cycle));
    }

    errors
}

fn walk(
    nodes: &[Node],
    schema: &Schema,
    index: &SchemaIndex,
    nomenclatures: &NomenclatureTable,
    scope: &mut Vec<GroupId>,
    seen: &mut BTreeSet<ColumnId>,
    errors: &mut Vec<SurfaceError>,
) {
    for node in nodes {
        match node {
            Node::Note(_) => {}
            Node::Section(section) => {
                if let Some(rule) = &section.visibility {
                    for error in typecheck(rule, schema, nomenclatures, scope) {
                        errors.push(SurfaceError::SectionRule(error));
                    }
                }
                walk(&section.children, schema, index, nomenclatures, scope, seen, errors);
            }
            Node::Group(group_node) => {
                let Some(info) = index.groups.get(&group_node.group) else {
                    errors.push(SurfaceError::UnknownGroup(group_node.group.clone()));
                    continue;
                };
                if info.parent_scope != *scope {
                    errors.push(SurfaceError::MisplacedGroup(group_node.group.clone()));
                }
                // The group's own rule governs the whole block: it
                // typechecks *outside* the group.
                if let Some(rule) = &group_node.visibility {
                    for error in typecheck(rule, schema, nomenclatures, scope) {
                        errors.push(SurfaceError::GroupRule(
                            group_node.group.clone(),
                            error,
                        ));
                    }
                }
                let entered = info.cardinality == Cardinality::Many;
                if entered {
                    scope.push(group_node.group.clone());
                }
                walk(&group_node.children, schema, index, nomenclatures, scope, seen, errors);
                if entered {
                    scope.pop();
                }
            }
            Node::Column(column_node) => {
                let column = &column_node.column;
                if !seen.insert(column.clone()) {
                    errors.push(SurfaceError::DuplicateColumn(column.clone()));
                }
                let Some(info) = index.columns.get(column) else {
                    errors.push(SurfaceError::UnknownColumn(column.clone()));
                    continue;
                };
                if info.scope != *scope {
                    errors.push(SurfaceError::MisplacedColumn(column.clone()));
                }
                if column_node.format.is_some() && info.ty != ScalarType::Text {
                    errors.push(SurfaceError::FormatOnNonText(column.clone()));
                }
                for rule in [&column_node.visibility, &column_node.required]
                    .into_iter()
                    .flatten()
                {
                    for error in typecheck(rule, schema, nomenclatures, scope) {
                        errors.push(SurfaceError::Rule(column.clone(), error));
                    }
                }
            }
        }
    }
}
