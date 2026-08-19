//! Totality and the absence-loses law (§4.1) over arbitrary expressions
//! × arbitrary (possibly non-conforming) records: `eval` and
//! `typecheck` never panic, negatives are independent atoms (not
//! negations) that lose on absence, and the paired atoms partition
//! exactly the comparable cases.

use std::collections::BTreeSet;

use proptest::prelude::*;
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, GroupId, ItemId, NomenclatureId, OptionId, PathSeg, RowPath};
use varve_logic::{
    Atom, ColumnRef, Const, EvalContext, Expr, Operand, PendingSet, eval, sources, typecheck,
};
use varve_schema::{
    Arity, Cardinality, Column, Element, Group, NomenclatureRef, NomenclatureTable, OptionRow,
    ScalarType, Schema, SchemaIndex, Unit,
};
use varve_value::{CellAddr, CellState, CellValue, ItemsAddr, RecordValues, Scalar};

fn column(id: &str, ty: ScalarType, arity: Arity) -> Element {
    Element::Column(Column {
        id: ColumnId::new(id),
        label: id.to_string(),
        ty,
        arity,
    })
}

/// `(id, label, fields)`.
type RowSpec<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

fn rows(pairs: &[RowSpec]) -> Vec<OptionRow> {
    pairs
        .iter()
        .map(|(id, label, fields)| OptionRow {
            id: OptionId::new(*id),
            label: (*label).into(),
            fields: fields
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        })
        .collect()
}

fn yes_no() -> NomenclatureRef {
    NomenclatureRef::Inline(rows(&[("oui", "Oui", &[]), ("non", "Non", &[])]))
}

fn communes() -> NomenclatureRef {
    NomenclatureRef::Inline(rows(&[
        ("01053", "Bourg-en-Bresse", &[("departement", "01")]),
        ("75056", "Paris", &[("departement", "75")]),
    ]))
}

const CONTACTS: &str = "contacts";

/// Every scalar type once, one `many` enum, one published enum, and a
/// `many` group with an item column.
fn schema() -> Schema {
    Schema {
        root: vec![
            column("situation", ScalarType::Enum(yes_no()), Arity::One),
            column("duree", ScalarType::Integer(Some(Unit::Month)), Arity::One),
            column("commune", ScalarType::Enum(communes()), Arity::One),
            column("tags", ScalarType::Enum(yes_no()), Arity::Many),
            column("note", ScalarType::Text, Arity::One),
            column("montant", ScalarType::Decimal(None), Arity::One),
            column("jour", ScalarType::Date, Arity::One),
            column("horodatage", ScalarType::Datetime, Arity::One),
            column("ok", ScalarType::Boolean, Arity::One),
            column(
                "pays",
                ScalarType::Enum(NomenclatureRef::Published {
                    id: NomenclatureId::new("pays"),
                    version: 1,
                }),
                Arity::One,
            ),
            Element::Group(Group {
                included_from: None,
                id: GroupId::new(CONTACTS),
                label: CONTACTS.into(),
                cardinality: Cardinality::Many,
                children: vec![column("role", ScalarType::Enum(yes_no()), Arity::One)],
            }),
        ],
        resolvers: vec![],
    }
}

/// The schema's columns plus ids it does not have.
const COLUMNS: &[&str] = &[
    "situation",
    "duree",
    "commune",
    "tags",
    "note",
    "montant",
    "jour",
    "horodatage",
    "ok",
    "pays",
    "role",
    "ghost",
    "unknown",
];
const OPTIONS: &[&str] = &["oui", "non", "01053", "75056", "FR", "zz"];
const RESOLVERS: &[&str] = &["insee", "ban", "ghost-r"];
const FIELDS: &[&str] = &["departement", "region"];

fn column_ref() -> impl Strategy<Value = ColumnRef> {
    (
        proptest::sample::select(COLUMNS),
        proptest::option::weighted(0.25, proptest::sample::select(FIELDS)),
    )
        .prop_map(|(c, f)| ColumnRef {
            column: ColumnId::new(c),
            field: f.map(String::from),
        })
}

fn date() -> impl Strategy<Value = Date> {
    (1900i32..=2100, 1u8..=12, 1u8..=28)
        .prop_map(|(y, m, d)| Date::parse(&format!("{y:04}-{m:02}-{d:02}")).unwrap())
}

fn datetime() -> impl Strategy<Value = Instant> {
    (
        1900i32..=2100,
        1u8..=12,
        1u8..=28,
        0u8..24,
        0u8..60,
        0u8..60,
    )
        .prop_map(|(y, mo, d, h, mi, s)| {
            Instant::parse(&format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")).unwrap()
        })
}

fn decimal() -> impl Strategy<Value = Decimal> {
    "-?[0-9]{1,6}(\\.[0-9]{1,3})?".prop_map(|s| Decimal::parse(&s).unwrap())
}

fn unit() -> impl Strategy<Value = Option<Unit>> {
    prop_oneof![
        Just(None),
        Just(Some(Unit::Month)),
        Just(Some(Unit::Year)),
        Just(Some(Unit::Day)),
        Just(Some(Unit::Metre)),
    ]
}

fn constant() -> impl Strategy<Value = Const> {
    prop_oneof![
        any::<bool>().prop_map(Const::Boolean),
        (decimal(), unit()).prop_map(|(value, unit)| Const::Number { value, unit }),
        date().prop_map(Const::Date),
        datetime().prop_map(Const::Datetime),
        proptest::sample::select(OPTIONS).prop_map(|o| Const::Option(OptionId::new(o))),
        prop_oneof![Just("01"), Just("75"), Just("x")].prop_map(|t| Const::Text(t.into())),
    ]
}

fn operand() -> impl Strategy<Value = Operand> {
    prop_oneof![
        4 => constant().prop_map(Operand::Const),
        1 => column_ref().prop_map(Operand::Column),
    ]
}

fn resolver() -> impl Strategy<Value = GroupId> {
    proptest::sample::select(RESOLVERS).prop_map(GroupId::new)
}

fn option() -> impl Strategy<Value = OptionId> {
    proptest::sample::select(OPTIONS).prop_map(OptionId::new)
}

/// All twelve atom kinds.
fn atom() -> impl Strategy<Value = Atom> {
    let cmp = |ctor: fn(ColumnRef, Operand) -> Atom| {
        (column_ref(), operand()).prop_map(move |(source, right)| ctor(source, right))
    };
    prop_oneof![
        cmp(|source, right| Atom::Eq { source, right }),
        cmp(|source, right| Atom::NotEq { source, right }),
        cmp(|source, right| Atom::Lt { source, right }),
        cmp(|source, right| Atom::Le { source, right }),
        cmp(|source, right| Atom::Gt { source, right }),
        cmp(|source, right| Atom::Ge { source, right }),
        column_ref().prop_map(|source| Atom::IsEmpty { source }),
        column_ref().prop_map(|source| Atom::IsFilled { source }),
        (column_ref(), option()).prop_map(|(source, option)| Atom::Contains { source, option }),
        (column_ref(), option()).prop_map(|(source, option)| Atom::Excludes { source, option }),
        resolver().prop_map(|group| Atom::Pending { group }),
        resolver().prop_map(|group| Atom::NotPending { group }),
    ]
}

fn expr() -> impl Strategy<Value = Expr> {
    atom()
        .prop_map(Expr::Atom)
        .prop_recursive(4, 24, 4, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..4).prop_map(Expr::And),
                proptest::collection::vec(inner, 0..4).prop_map(Expr::Or),
            ]
        })
}

/// Any scalar of any kind — deliberately not matched to the column it
/// lands in: eval must be total over non-conforming stored values.
fn scalar() -> impl Strategy<Value = Scalar> {
    prop_oneof![
        prop_oneof![Just("01"), Just("x"), Just("")].prop_map(|t| Scalar::Text(t.into())),
        any::<bool>().prop_map(Scalar::Boolean),
        (-40i64..40).prop_map(Scalar::Integer),
        decimal().prop_map(Scalar::Decimal),
        date().prop_map(Scalar::Date),
        datetime().prop_map(Scalar::Datetime),
        option().prop_map(Scalar::Enum),
    ]
}

fn state() -> impl Strategy<Value = CellState> {
    prop_oneof![
        Just(CellState::Empty),
        scalar().prop_map(|s| CellState::Value(CellValue::One(s))),
        // Includes the zero-length list conformance forbids: eval must
        // still be total (and read it as absent).
        proptest::collection::vec(scalar(), 0..3)
            .prop_map(|v| CellState::Value(CellValue::Many(v))),
    ]
}

fn item(group: &str, id: &str) -> RowPath {
    RowPath::root().child(PathSeg {
        group: GroupId::new(group),
        item: ItemId::new(id),
    })
}

/// Root, an item of `contacts`, or an item of a group the schema does
/// not have.
fn path() -> impl Strategy<Value = RowPath> {
    prop_oneof![
        Just(RowPath::root()),
        Just(item(CONTACTS, "i1")),
        Just(item(CONTACTS, "i2")),
        Just(item("ghosts", "i1")),
    ]
}

fn record_values() -> impl Strategy<Value = RecordValues> {
    (
        proptest::collection::btree_map((proptest::sample::select(COLUMNS), path()), state(), 0..8),
        proptest::sample::subsequence(vec!["i1", "i2"], 0..=2),
    )
        .prop_map(|(cells, items)| {
            let mut v = RecordValues::new();
            for ((column, path), state) in cells {
                v.cells.insert(
                    CellAddr {
                        column: ColumnId::new(column),
                        path,
                    },
                    state,
                );
            }
            if !items.is_empty() {
                v.items.insert(
                    ItemsAddr {
                        group: GroupId::new(CONTACTS),
                        parent: RowPath::root(),
                    },
                    items.into_iter().map(ItemId::new).collect(),
                );
            }
            v
        })
}

fn hidden() -> impl Strategy<Value = BTreeSet<ColumnId>> {
    proptest::collection::btree_set(
        proptest::sample::select(COLUMNS).prop_map(ColumnId::new),
        0..4,
    )
}

fn pending() -> impl Strategy<Value = PendingSet> {
    proptest::collection::btree_set((path(), resolver()), 0..4)
}

/// One evaluation situation: record, item, hidden set, pending set.
#[derive(Debug, Clone)]
struct Situation {
    values: RecordValues,
    item: RowPath,
    hidden: BTreeSet<ColumnId>,
    pending: PendingSet,
}

fn situation() -> impl Strategy<Value = Situation> {
    (record_values(), path(), hidden(), pending()).prop_map(|(values, item, hidden, pending)| {
        Situation {
            values,
            item,
            hidden,
            pending,
        }
    })
}

fn published_table() -> NomenclatureTable {
    let mut t = NomenclatureTable::new();
    t.insert(
        NomenclatureId::new("pays"),
        1,
        rows(&[("FR", "France", &[])]),
    );
    t
}

struct Fixture {
    index: SchemaIndex,
    noms: NomenclatureTable,
}

impl Fixture {
    fn new() -> Self {
        Self {
            index: SchemaIndex::build(&schema()),
            noms: published_table(),
        }
    }

    fn ctx<'a>(&'a self, s: &'a Situation) -> EvalContext<'a> {
        EvalContext {
            index: &self.index,
            nomenclatures: &self.noms,
            values: &s.values,
            item: s.item.clone(),
            hidden: s.hidden.clone(),
            pending: s.pending.clone(),
        }
    }

    /// The §4.1 read rule restated: hidden, unknown, out-of-scope,
    /// absent, empty and zero-length all read as absent.
    fn read<'a>(&self, source: &ColumnRef, s: &'a Situation) -> Option<&'a CellValue> {
        if s.hidden.contains(&source.column) {
            return None;
        }
        let info = self.index.columns.get(&source.column)?;
        let segments = s.item.segments();
        if segments.len() < info.scope.len() {
            return None;
        }
        let prefix = &segments[..info.scope.len()];
        if prefix.iter().map(|seg| &seg.group).ne(info.scope.iter()) {
            return None;
        }
        let path = prefix
            .iter()
            .cloned()
            .fold(RowPath::root(), |p, seg| p.child(seg));
        match s.values.cells.get(&CellAddr {
            column: source.column.clone(),
            path,
        })? {
            CellState::Empty => None,
            CellState::Value(CellValue::Many(items)) if items.is_empty() => None,
            CellState::Value(v) => Some(v),
        }
    }
}

fn atom_expr(a: Atom) -> Expr {
    Expr::Atom(a)
}

proptest! {
    /// (a) `eval` is total over anything: unknown columns, values of the
    /// wrong type, out-of-scope items, field projections on non-enums.
    #[test]
    fn eval_never_panics(e in expr(), s in situation()) {
        let f = Fixture::new();
        let _ = eval(&e, &f.ctx(&s));
    }

    /// (b) `typecheck` is total over arbitrary expressions, scopes and
    /// nomenclature tables.
    #[test]
    fn typecheck_never_panics(e in expr(), item_scope in any::<bool>()) {
        let s = schema();
        let scope: Vec<GroupId> = if item_scope { vec![GroupId::new(CONTACTS)] } else { vec![] };
        let _ = typecheck(&e, &s, &NomenclatureTable::new(), &scope);
        let _ = typecheck(&e, &s, &published_table(), &scope);
    }

    /// (c) Absence always loses: a hidden, absent or empty source makes
    /// every comparison and membership atom false, `is_empty` true and
    /// `is_filled` false — whether the atom is positive or negative.
    #[test]
    fn absence_loses(a in atom(), s in situation(), how in 0u8..3) {
        let Some(source) = source_of(&a) else { return Ok(()) };
        let f = Fixture::new();
        // Make the source absent one of three ways.
        let mut s = s;
        match how {
            0 => { s.hidden.insert(source.column.clone()); }
            1 => s.values.cells.retain(|addr, _| addr.column != source.column),
            _ => {
                for state in s.values.cells.iter_mut().filter(|(addr, _)| addr.column == source.column).map(|(_, st)| st) {
                    *state = CellState::Empty;
                }
            }
        }
        let ctx = f.ctx(&s);
        prop_assert!(f.read(&source, &s).is_none());
        prop_assert!(!eval(&atom_expr(a), &ctx), "an atom on an absent source held");
        let is_empty = atom_expr(Atom::IsEmpty { source: source.clone() });
        let is_filled = atom_expr(Atom::IsFilled { source });
        prop_assert!(eval(&is_empty, &ctx), "is_empty on an absent source did not hold");
        prop_assert!(!eval(&is_filled, &ctx), "is_filled on an absent source held");
    }

    /// (d) Combinators are exactly all/any of their operands; the empty
    /// `and` is true and the empty `or` false.
    #[test]
    fn combinators_are_all_and_any(items in proptest::collection::vec(expr(), 0..5), s in situation()) {
        let f = Fixture::new();
        let ctx = f.ctx(&s);
        let each: Vec<bool> = items.iter().map(|e| eval(e, &ctx)).collect();
        prop_assert_eq!(eval(&Expr::And(items.clone()), &ctx), each.iter().all(|b| *b));
        prop_assert_eq!(eval(&Expr::Or(items), &ctx), each.iter().any(|b| *b));
        prop_assert!(eval(&Expr::And(vec![]), &ctx));
        prop_assert!(!eval(&Expr::Or(vec![]), &ctx));
    }

    /// (e) The comparison pairs partition the comparable cases: never
    /// both, and comparability is one property shared by all three
    /// pairs — so `le = lt ∨ eq`, `ge = gt ∨ eq`, `not_eq = lt ∨ gt`.
    #[test]
    fn comparison_pairs_partition_comparable_cases(source in column_ref(), right in operand(), s in situation()) {
        let f = Fixture::new();
        let ctx = f.ctx(&s);
        let run = |ctor: fn(ColumnRef, Operand) -> Atom| {
            eval(&atom_expr(ctor(source.clone(), right.clone())), &ctx)
        };
        let eq = run(|source, right| Atom::Eq { source, right });
        let ne = run(|source, right| Atom::NotEq { source, right });
        let lt = run(|source, right| Atom::Lt { source, right });
        let le = run(|source, right| Atom::Le { source, right });
        let gt = run(|source, right| Atom::Gt { source, right });
        let ge = run(|source, right| Atom::Ge { source, right });
        prop_assert!(!(eq && ne));
        prop_assert!(!(lt && ge));
        prop_assert!(!(gt && le));
        let comparable = eq || ne;
        prop_assert_eq!(lt || ge, comparable);
        prop_assert_eq!(gt || le, comparable);
        prop_assert_eq!(le, lt || eq);
        prop_assert_eq!(ge, gt || eq);
        prop_assert_eq!(ne, lt || gt);
        // Comparable implies filled; a filled source is not always
        // comparable (type drift, unit mismatch, many-valued).
        if comparable {
            prop_assert!(f.read(&source, &s).is_some());
            let is_filled = atom_expr(Atom::IsFilled { source: source.clone() });
            prop_assert!(eval(&is_filled, &ctx), "comparable but not filled");
        }
    }

    /// (f) `pending` / `not_pending` are complementary — always, at
    /// every item and for every pending set.
    #[test]
    fn pending_pairs_are_complementary(r in resolver(), s in situation()) {
        let f = Fixture::new();
        let ctx = f.ctx(&s);
        let p = eval(&atom_expr(Atom::Pending { group: r.clone() }), &ctx);
        let np = eval(&atom_expr(Atom::NotPending { group: r }), &ctx);
        prop_assert_ne!(p, np);
    }

    /// (g) `contains` / `excludes`: exactly one holds when the source
    /// reads as a non-empty list, both lose otherwise (absence, `one`
    /// values, zero-length lists).
    #[test]
    fn membership_pairs_partition_lists(source in column_ref(), o in option(), s in situation()) {
        let f = Fixture::new();
        let ctx = f.ctx(&s);
        let contains = eval(&atom_expr(Atom::Contains { source: source.clone(), option: o.clone() }), &ctx);
        let excludes = eval(&atom_expr(Atom::Excludes { source: source.clone(), option: o }), &ctx);
        match f.read(&source, &s) {
            Some(CellValue::Many(items)) if !items.is_empty() => prop_assert_ne!(contains, excludes),
            _ => prop_assert!(!contains && !excludes),
        }
    }

    /// The presence atoms are exactly the read rule: `is_empty` iff the
    /// source reads as absent, `is_filled` iff it reads a value.
    #[test]
    fn presence_atoms_are_the_read_rule(source in column_ref(), s in situation()) {
        let f = Fixture::new();
        let ctx = f.ctx(&s);
        let filled = f.read(&source, &s).is_some();
        prop_assert_eq!(eval(&atom_expr(Atom::IsFilled { source: source.clone() }), &ctx), filled);
        prop_assert_eq!(eval(&atom_expr(Atom::IsEmpty { source }), &ctx), !filled);
    }

    /// (h) A well-typed expression reads only columns the schema has,
    /// only in scope, and every resolver it names is declared.
    #[test]
    fn well_typed_reads_known_columns_in_scope(e in expr(), item_scope in any::<bool>()) {
        let s = schema();
        let index = SchemaIndex::build(&s);
        let scope: Vec<GroupId> = if item_scope { vec![GroupId::new(CONTACTS)] } else { vec![] };
        if typecheck(&e, &s, &published_table(), &scope).is_empty() {
            for c in sources(&e) {
                let info = index.columns.get(&c);
                prop_assert!(info.is_some(), "well-typed expression reads unknown column {c}");
                prop_assert!(scope.starts_with(&info.unwrap().scope), "column {c} out of scope");
            }
            // The schema declares no resolvers: a well-typed expression
            // has no pending atoms at all.
            prop_assert!(varve_logic::pending_sources(&e).is_empty());
        }
    }
}

fn source_of(a: &Atom) -> Option<ColumnRef> {
    match a {
        Atom::Eq { source, .. }
        | Atom::NotEq { source, .. }
        | Atom::Lt { source, .. }
        | Atom::Le { source, .. }
        | Atom::Gt { source, .. }
        | Atom::Ge { source, .. }
        | Atom::Contains { source, .. }
        | Atom::Excludes { source, .. } => Some(source.clone()),
        // Presence atoms are the law's conclusion, not its subject.
        Atom::IsEmpty { .. }
        | Atom::IsFilled { .. }
        | Atom::Pending { .. }
        | Atom::NotPending { .. } => None,
    }
}
