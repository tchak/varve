use std::collections::{BTreeMap, BTreeSet};

use varve_core::primitives::Decimal;
use varve_core::{ColumnId, GroupId, ItemId, OptionId, PathSeg, ResolverId, RowPath};
use varve_logic::{
    Atom, ColumnRef, Const, EvalContext, Expr, Operand, TypeError, check_acyclic,
    eval, sources, typecheck,
};
use varve_schema::{
    Arity, Cardinality, Column, Element, Group, NomenclatureRef, OptionRow,
    ScalarType, Schema, SchemaIndex, Unit,
};
use varve_value::{CellAddr, CellState, CellValue, RecordValues, Scalar};

fn column(id: &str, ty: ScalarType, arity: Arity) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.to_string(),
        ty,
        arity,
    })
}

fn communes() -> NomenclatureRef {
    NomenclatureRef::Inline(vec![
        OptionRow {
            id: OptionId::new("01053"),
            label: "Bourg-en-Bresse".into(),
            fields: vec![("departement".into(), "01".into())],
        },
        OptionRow {
            id: OptionId::new("75056"),
            label: "Paris".into(),
            fields: vec![("departement".into(), "75".into())],
        },
    ])
}

fn yes_no() -> NomenclatureRef {
    NomenclatureRef::Inline(vec![
        OptionRow { id: OptionId::new("oui"), label: "Oui".into(), fields: vec![] },
        OptionRow { id: OptionId::new("non"), label: "Non".into(), fields: vec![] },
    ])
}

/// situation: enum; montant: decimal €?— no: duration in months;
/// commune: enum with departement field; tags: enum many; note: text;
/// contacts (many) { role: enum }.
fn schema() -> Schema {
    Schema {
        root: vec![
            column("situation", ScalarType::Enum(yes_no()), Arity::One),
            column(
                "duree",
                ScalarType::Integer(Some(Unit::Month)),
                Arity::One,
            ),
            column("commune", ScalarType::Enum(communes()), Arity::One),
            column("tags", ScalarType::Enum(yes_no()), Arity::Many),
            column("note", ScalarType::Text, Arity::One),
            Element::Group(Group {
                id: GroupId::new("contacts"),
                label: "contacts".into(),
                cardinality: Cardinality::Many,
                children: vec![column("role", ScalarType::Enum(yes_no()), Arity::One)],
            }),
        ],
        resolvers: vec![],
    }
}

fn source(id: &str) -> ColumnRef {
    ColumnRef { column: ColumnId::new(id), field: None }
}

fn eq(id: &str, c: Const) -> Expr {
    Expr::Atom(Atom::Eq { source: source(id), right: Operand::Const(c) })
}

fn months(n: &str) -> Const {
    Const::Number { value: Decimal::parse(n).unwrap(), unit: Some(Unit::Month) }
}

fn years(n: &str) -> Const {
    Const::Number { value: Decimal::parse(n).unwrap(), unit: Some(Unit::Year) }
}

struct Fixture {
    index: SchemaIndex,
    noms: varve_schema::NomenclatureTable,
    values: RecordValues,
}

impl Fixture {
    fn new() -> Self {
        Self {
            index: SchemaIndex::build(&schema()),
            noms: Default::default(),
            values: RecordValues::new(),
        }
    }

    fn set(&mut self, id: &str, scalar: Scalar) -> &mut Self {
        self.values.cells.insert(
            CellAddr { column: ColumnId::new(id), path: RowPath::root() },
            CellState::Value(CellValue::One(scalar)),
        );
        self
    }

    fn ctx(&self) -> EvalContext<'_> {
        EvalContext {
            index: &self.index,
            nomenclatures: &self.noms,
            values: &self.values,
            item: RowPath::root(),
            hidden: BTreeSet::new(),
            pending: BTreeSet::new(),
        }
    }
}

#[test]
fn absence_always_loses_even_for_negatives() {
    let f = Fixture::new();
    let ctx = f.ctx();
    // Unanswered: both Eq and NotEq are false — NotEq is not Not(Eq).
    assert!(!eval(&eq("situation", Const::Option(OptionId::new("oui"))), &ctx));
    assert!(!eval(
        &Expr::Atom(Atom::NotEq {
            source: source("situation"),
            right: Operand::Const(Const::Option(OptionId::new("oui"))),
        }),
        &ctx
    ));
    // is_empty is the one atom absence satisfies.
    assert!(eval(&Expr::Atom(Atom::IsEmpty { source: source("situation") }), &ctx));
    // The "visible unless explicitly no" idiom from §4.1.
    let unless_no = Expr::Or(vec![
        Expr::Atom(Atom::IsEmpty { source: source("situation") }),
        Expr::Atom(Atom::NotEq {
            source: source("situation"),
            right: Operand::Const(Const::Option(OptionId::new("non"))),
        }),
    ]);
    assert!(eval(&unless_no, &f.ctx()));
}

#[test]
fn hidden_sources_read_as_absent() {
    let mut f = Fixture::new();
    f.set("situation", Scalar::Enum(OptionId::new("oui")));
    let expr = eq("situation", Const::Option(OptionId::new("oui")));
    assert!(eval(&expr, &f.ctx()));
    let mut ctx = f.ctx();
    ctx.hidden.insert(ColumnId::new("situation"));
    // A stale value in a hidden column must not drive visibility.
    assert!(!eval(&expr, &ctx));
}

#[test]
fn unit_aware_comparison_is_exact() {
    let mut f = Fixture::new();
    f.set("duree", Scalar::Integer(18));
    // 18 months vs 1.5 years: equal, exactly, across units.
    assert!(eval(&eq("duree", years("1.5")), &f.ctx()));
    assert!(eval(
        &Expr::Atom(Atom::Gt {
            source: source("duree"),
            right: Operand::Const(years("1")),
        }),
        &f.ctx()
    ));
    assert!(!eval(&eq("duree", months("17")), &f.ctx()));
}

#[test]
fn field_projection_dissolves_geo_operators() {
    let mut f = Fixture::new();
    f.set("commune", Scalar::Enum(OptionId::new("01053")));
    // InDepartement(commune, "01") = eq(column(commune, departement), "01")
    let in_dept = |code: &str| {
        Expr::Atom(Atom::Eq {
            source: ColumnRef {
                column: ColumnId::new("commune"),
                field: Some("departement".into()),
            },
            right: Operand::Const(Const::Text(code.into())),
        })
    };
    assert!(eval(&in_dept("01"), &f.ctx()));
    assert!(!eval(&in_dept("75"), &f.ctx()));
}

#[test]
fn item_scope_reads_own_item_and_record() {
    let mut f = Fixture::new();
    f.set("situation", Scalar::Enum(OptionId::new("oui")));
    let item = RowPath::root().child(PathSeg {
        group: GroupId::new("contacts"),
        item: ItemId::new("i1"),
    });
    f.values.cells.insert(
        CellAddr { column: ColumnId::new("role"), path: item.clone() },
        CellState::Value(CellValue::One(Scalar::Enum(OptionId::new("non")))),
    );
    let both = Expr::And(vec![
        eq("situation", Const::Option(OptionId::new("oui"))),
        eq("role", Const::Option(OptionId::new("non"))),
    ]);
    let mut ctx = f.ctx();
    ctx.item = item;
    assert!(eval(&both, &ctx));
    // From the record scope, the item column reads absent.
    assert!(!eval(&eq("role", Const::Option(OptionId::new("non"))), &f.ctx()));
}

#[test]
fn typechecker_enforces_the_matrix_and_scopes() {
    let s = schema();
    let noms = Default::default();
    let record = &[];
    let contacts = &[GroupId::new("contacts")];

    // Raw text comparisons: out of v1.
    let text_eq = eq("note", Const::Text("x".into()));
    assert!(matches!(
        typecheck(&text_eq, &s, &noms, record).as_slice(),
        [TypeError::AtomNotAllowed(_)]
    ));
    // Text presence: allowed.
    let present = Expr::Atom(Atom::IsFilled { source: source("note") });
    assert_eq!(typecheck(&present, &s, &noms, record), vec![]);

    // Record rule reading an item column: scope violation.
    let bad = eq("role", Const::Option(OptionId::new("oui")));
    assert!(matches!(
        typecheck(&bad, &s, &noms, record).as_slice(),
        [TypeError::ScopeViolation(_)]
    ));
    // The same rule at item scope: fine — and it may read record
    // columns too.
    assert_eq!(typecheck(&bad, &s, &noms, contacts), vec![]);
    let record_from_item = eq("situation", Const::Option(OptionId::new("oui")));
    assert_eq!(typecheck(&record_from_item, &s, &noms, contacts), vec![]);

    // Unknown option: statically caught (§2.12).
    let bad_option = eq("situation", Const::Option(OptionId::new("peut-etre")));
    assert!(matches!(
        typecheck(&bad_option, &s, &noms, record).as_slice(),
        [TypeError::UnknownOption(..)]
    ));

    // Unit dimension mismatch: months column vs metre constant.
    let bad_unit = eq(
        "duree",
        Const::Number { value: Decimal::parse("1").unwrap(), unit: Some(Unit::Metre) },
    );
    assert!(matches!(
        typecheck(&bad_unit, &s, &noms, record).as_slice(),
        [TypeError::UnitMismatch(_)]
    ));

    // Column-to-column: representable, rejected by policy.
    let col_col = Expr::Atom(Atom::Eq {
        source: source("duree"),
        right: Operand::Column(source("duree")),
    });
    assert!(
        typecheck(&col_col, &s, &noms, record)
            .contains(&TypeError::ColumnComparisonNotEnabled)
    );

    // Unknown resolver in pending().
    let pending = Expr::Atom(Atom::Pending { resolver: ResolverId::new("insee") });
    assert!(matches!(
        typecheck(&pending, &s, &noms, record).as_slice(),
        [TypeError::UnknownResolver(_)]
    ));

    // Unknown projected field.
    let bad_field = Expr::Atom(Atom::Eq {
        source: ColumnRef {
            column: ColumnId::new("commune"),
            field: Some("region".into()),
        },
        right: Operand::Const(Const::Text("84".into())),
    });
    assert!(matches!(
        typecheck(&bad_field, &s, &noms, record).as_slice(),
        [TypeError::UnknownField(..)]
    ));
}

#[test]
fn contains_and_pending() {
    let mut f = Fixture::new();
    f.values.cells.insert(
        CellAddr { column: ColumnId::new("tags"), path: RowPath::root() },
        CellState::Value(CellValue::Many(vec![Scalar::Enum(OptionId::new("oui"))])),
    );
    let contains = |o: &str| {
        Expr::Atom(Atom::Contains { source: source("tags"), option: OptionId::new(o) })
    };
    assert!(eval(&contains("oui"), &f.ctx()));
    assert!(!eval(&contains("non"), &f.ctx()));
    let excludes = Expr::Atom(Atom::Excludes {
        source: source("tags"),
        option: OptionId::new("non"),
    });
    assert!(eval(&excludes, &f.ctx()));

    let mut ctx = f.ctx();
    ctx.pending.insert(ResolverId::new("insee-sirene"));
    let pending = Expr::Atom(Atom::Pending { resolver: ResolverId::new("insee-sirene") });
    assert!(eval(&pending, &ctx));
    assert!(!eval(&pending, &f.ctx()));
}

#[test]
fn acyclicity_check_orders_and_detects() {
    let rule = |on: &str| eq(on, Const::Option(OptionId::new("oui")));
    // b depends on a, c on b: fine, topological order respects it.
    let mut rules = BTreeMap::new();
    rules.insert(ColumnId::new("b"), rule("a"));
    rules.insert(ColumnId::new("c"), rule("b"));
    let order = check_acyclic(&rules).unwrap();
    let position = |id: &str| order.iter().position(|c| c.as_str() == id).unwrap();
    assert!(position("b") < position("c"));

    // a → b → a: cycle, named.
    rules.insert(ColumnId::new("a"), rule("b"));
    let cycle = check_acyclic(&rules).unwrap_err();
    assert!(cycle.cycle.len() >= 3);

    // sources() feeds the graph.
    assert_eq!(
        sources(&rule("a")),
        BTreeSet::from([ColumnId::new("a")])
    );
}
