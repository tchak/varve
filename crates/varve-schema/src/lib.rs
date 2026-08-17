//! Tier 1 (§7): types, arity, groups, cardinality, blocks, nomenclatures,
//! resolver declarations, structural constraints, depth policy.
//!
//! M0 scope: enough to express every DN procedure. The cast table and the
//! type join (§5.5) come with M1.

#![forbid(unsafe_code)]

mod canon;
mod cast;
mod units;

pub use canon::{revision_id, schema_canonical};
pub use units::{Dimension, Unit, conversion};

pub use cast::{
    Cast, CastClass, CastError, JoinConflict, JoinPath, NomenclatureTable,
    arity_cast, arity_join, column_cast, column_join, nomenclature_rows,
    scalar_cast, scalar_join,
};

use std::collections::HashSet;

use varve_core::{ColumnId, GroupId, NomenclatureId, OptionId, ResolverId};

/// Column arity (§2.2): a `many` column holds a list *value* and
/// contributes nothing to the row path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arity {
    One,
    Many,
}

/// Group cardinality (§2.2): a `many` group introduces a *scope* and
/// contributes a row-path segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    One,
    Many,
}

/// The nine scalars settled by the M0 residue
/// (`corpus/M0-type-frequency.md`). Format constraints (email, phone,
/// IBAN, regex) are surface admissibility over `Text`, never types (§2.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScalarType {
    Text,
    Boolean,
    /// Optional unit (§2.14): plain numbers stay plain.
    Integer(Option<Unit>),
    Decimal(Option<Unit>),
    Date,
    Datetime,
    /// Every enum is nomenclature-backed (§2.12).
    Enum(NomenclatureRef),
    /// One file; multi-file is arity `many` (§2.2).
    Attachment,
    /// One GeoJSON Feature; feature sets are arity `many`. A
    /// FeatureCollection is a render shape, not a kernel value.
    Geometry,
}

impl ScalarType {
    /// Type-constructor equality, ignoring which nomenclature backs an
    /// enum. M0 mapping checks are exact on the constructor; the real
    /// compatibility relation is the cast table (M1).
    pub fn same_constructor(&self, other: &ScalarType) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

/// One row of a nomenclature: `(id, label, …fields)` (§2.11, §2.12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionRow {
    pub id: OptionId,
    pub label: String,
    /// Extra fields, if any. Rows carrying more than `(id, label)` are
    /// what activate the resolver aspect of a nomenclature (§2.12).
    pub fields: Vec<(String, String)>,
}

/// How an enum column is backed (§2.12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NomenclatureRef {
    /// Inline: no identity, no ceremony — versions with the containing
    /// revision, ids synthesized by the authoring tool.
    Inline(Vec<OptionRow>),
    /// Published standalone with its own identity and version; travels in
    /// the wire stream like a block.
    Published { id: NomenclatureId, version: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub id: ColumnId,
    pub label: String,
    pub ty: ScalarType,
    pub arity: Arity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: GroupId,
    pub label: String,
    pub cardinality: Cardinality,
    pub children: Vec<Element>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Element {
    Column(Column),
    Group(Group),
}

/// A named field of a resolver's declared result type (§2.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultField {
    pub name: String,
    pub ty: ScalarType,
}

/// Mapping is projection (§2.7): one result field lands in one column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    pub result_field: String,
    pub target: ColumnId,
}

/// Schema-side resolver declaration (§2.7): a versioned schema *object*,
/// not a type. The implementation (endpoint, credentials) is
/// instance-local and never part of the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverDeclaration {
    pub id: ResolverId,
    pub version: u32,
    /// Input signature: which columns feed it, with types.
    pub input: Vec<(ColumnId, ScalarType)>,
    /// Declaring the result type in the schema is what makes the whole
    /// thing analysable (§2.7).
    pub result_type: Vec<ResultField>,
    pub mapping: Vec<Mapping>,
}

/// A schema: the root is an implicit `one` group (§2.5).
#[derive(Debug, Clone, Default)]
pub struct Schema {
    pub root: Vec<Element>,
    pub resolvers: Vec<ResolverDeclaration>,
}

/// Everything addressing needs to know about one column: its type,
/// arity, and **scope** — the chain of `many` groups from the root
/// (`one` groups contribute nothing, §2.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub ty: ScalarType,
    pub arity: Arity,
    pub scope: Vec<GroupId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    pub cardinality: Cardinality,
    /// Scope *outside* this group.
    pub parent_scope: Vec<GroupId>,
}

/// Flat lookup over a schema's tree — what conformance and projection
/// both need.
#[derive(Debug, Clone, Default)]
pub struct SchemaIndex {
    pub columns: std::collections::BTreeMap<varve_core::ColumnId, ColumnInfo>,
    pub groups: std::collections::BTreeMap<GroupId, GroupInfo>,
}

impl SchemaIndex {
    pub fn build(schema: &Schema) -> Self {
        fn walk(
            elements: &[Element],
            scope: &mut Vec<GroupId>,
            index: &mut SchemaIndex,
        ) {
            for el in elements {
                match el {
                    Element::Column(c) => {
                        index.columns.insert(
                            c.id.clone(),
                            ColumnInfo {
                                ty: c.ty.clone(),
                                arity: c.arity,
                                scope: scope.clone(),
                            },
                        );
                    }
                    Element::Group(g) => {
                        index.groups.insert(
                            g.id.clone(),
                            GroupInfo {
                                cardinality: g.cardinality,
                                parent_scope: scope.clone(),
                            },
                        );
                        match g.cardinality {
                            Cardinality::Many => {
                                scope.push(g.id.clone());
                                walk(&g.children, scope, index);
                                scope.pop();
                            }
                            Cardinality::One => walk(&g.children, scope, index),
                        }
                    }
                }
            }
        }
        let mut index = SchemaIndex::default();
        walk(&schema.root, &mut Vec::new(), &mut index);
        index
    }
}

/// Depth-1 is a policy, not a type (§2.3).
#[derive(Debug, Clone, Copy)]
pub struct DepthPolicy {
    /// Maximum nesting of `many` groups. `one` groups contribute nothing
    /// to the row path and are not counted.
    pub max_many_depth: usize,
}

impl Default for DepthPolicy {
    fn default() -> Self {
        Self { max_many_depth: 1 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SchemaError {
    /// §2.3: a validation rule with an error message, never a type.
    #[error("group '{group}' nests `many` groups to depth {depth}, policy allows {max}")]
    DepthExceeded {
        group: GroupId,
        depth: usize,
        max: usize,
    },
    #[error("duplicate column id '{0}'")]
    DuplicateColumnId(ColumnId),
    #[error("duplicate group id '{0}'")]
    DuplicateGroupId(GroupId),
    /// Mapping typecheck failures (§2.7): these are schema-publication
    /// errors, not runtime surprises.
    #[error("resolver '{resolver}' maps into unknown column '{target}'")]
    UnknownMappingTarget {
        resolver: ResolverId,
        target: ColumnId,
    },
    #[error("resolver '{resolver}' maps result field '{field}' absent from its declared result type")]
    UnknownMappingField {
        resolver: ResolverId,
        field: String,
    },
    #[error("resolver '{resolver}': result field '{field}' does not typecheck against column '{target}'")]
    MappingTypeMismatch {
        resolver: ResolverId,
        field: String,
        target: ColumnId,
    },
    #[error("resolver '{resolver}' reads unknown column '{column}'")]
    UnknownInputColumn {
        resolver: ResolverId,
        column: ColumnId,
    },
}

/// Validate a schema against structural rules and the depth policy.
pub fn validate(schema: &Schema, policy: DepthPolicy) -> Vec<SchemaError> {
    let mut errors = Vec::new();
    let mut columns: Vec<(ColumnId, ScalarType)> = Vec::new();
    let mut column_ids = HashSet::new();
    let mut group_ids = HashSet::new();

    fn walk(
        elements: &[Element],
        many_depth: usize,
        policy: DepthPolicy,
        errors: &mut Vec<SchemaError>,
        columns: &mut Vec<(ColumnId, ScalarType)>,
        column_ids: &mut HashSet<ColumnId>,
        group_ids: &mut HashSet<GroupId>,
    ) {
        for el in elements {
            match el {
                Element::Column(c) => {
                    if !column_ids.insert(c.id.clone()) {
                        errors.push(SchemaError::DuplicateColumnId(c.id.clone()));
                    }
                    columns.push((c.id.clone(), c.ty.clone()));
                }
                Element::Group(g) => {
                    if !group_ids.insert(g.id.clone()) {
                        errors.push(SchemaError::DuplicateGroupId(g.id.clone()));
                    }
                    let depth = match g.cardinality {
                        Cardinality::Many => many_depth + 1,
                        Cardinality::One => many_depth,
                    };
                    if depth > policy.max_many_depth {
                        errors.push(SchemaError::DepthExceeded {
                            group: g.id.clone(),
                            depth,
                            max: policy.max_many_depth,
                        });
                    }
                    walk(
                        &g.children,
                        depth,
                        policy,
                        errors,
                        columns,
                        column_ids,
                        group_ids,
                    );
                }
            }
        }
    }

    walk(
        &schema.root,
        0,
        policy,
        &mut errors,
        &mut columns,
        &mut column_ids,
        &mut group_ids,
    );

    for r in &schema.resolvers {
        for (input, _) in &r.input {
            if !column_ids.contains(input) {
                errors.push(SchemaError::UnknownInputColumn {
                    resolver: r.id.clone(),
                    column: input.clone(),
                });
            }
        }
        for m in &r.mapping {
            let field = r.result_type.iter().find(|f| f.name == m.result_field);
            let target = columns.iter().find(|(id, _)| *id == m.target);
            match (field, target) {
                (None, _) => errors.push(SchemaError::UnknownMappingField {
                    resolver: r.id.clone(),
                    field: m.result_field.clone(),
                }),
                (_, None) => errors.push(SchemaError::UnknownMappingTarget {
                    resolver: r.id.clone(),
                    target: m.target.clone(),
                }),
                (Some(f), Some((_, ty))) => {
                    if !f.ty.same_constructor(ty) {
                        errors.push(SchemaError::MappingTypeMismatch {
                            resolver: r.id.clone(),
                            field: m.result_field.clone(),
                            target: m.target.clone(),
                        });
                    }
                }
            }
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(id: &str, ty: ScalarType) -> Element {
        Element::Column(Column {
            id: ColumnId::new(id),
            label: id.to_string(),
            ty,
            arity: Arity::One,
        })
    }

    fn many_group(id: &str, children: Vec<Element>) -> Element {
        Element::Group(Group {
            id: GroupId::new(id),
            label: id.to_string(),
            cardinality: Cardinality::Many,
            children,
        })
    }

    #[test]
    fn depth_one_is_fine() {
        let schema = Schema {
            root: vec![many_group("g1", vec![col("c1", ScalarType::Text)])],
            resolvers: vec![],
        };
        assert!(validate(&schema, DepthPolicy::default()).is_empty());
    }

    #[test]
    fn nested_many_violates_depth_policy() {
        let schema = Schema {
            root: vec![many_group(
                "g1",
                vec![many_group("g2", vec![col("c1", ScalarType::Text)])],
            )],
            resolvers: vec![],
        };
        let errors = validate(&schema, DepthPolicy::default());
        assert!(matches!(
            errors.as_slice(),
            [SchemaError::DepthExceeded { depth: 2, max: 1, .. }]
        ));
    }

    #[test]
    fn one_groups_do_not_count_toward_depth() {
        let one_group = Element::Group(Group {
            id: GroupId::new("wrapper"),
            label: "wrapper".into(),
            cardinality: Cardinality::One,
            children: vec![many_group("g1", vec![col("c1", ScalarType::Text)])],
        });
        let schema = Schema {
            root: vec![one_group],
            resolvers: vec![],
        };
        assert!(validate(&schema, DepthPolicy::default()).is_empty());
    }

    #[test]
    fn mapping_typecheck() {
        let schema = Schema {
            root: vec![col("siret", ScalarType::Text), col("name", ScalarType::Text)],
            resolvers: vec![ResolverDeclaration {
                id: ResolverId::new("insee"),
                version: 1,
                input: vec![(ColumnId::new("siret"), ScalarType::Text)],
                result_type: vec![ResultField {
                    name: "raison_sociale".into(),
                    ty: ScalarType::Date, // wrong on purpose
                }],
                mapping: vec![Mapping {
                    result_field: "raison_sociale".into(),
                    target: ColumnId::new("name"),
                }],
            }],
        };
        let errors = validate(&schema, DepthPolicy::default());
        assert!(matches!(
            errors.as_slice(),
            [SchemaError::MappingTypeMismatch { .. }]
        ));
    }
}
