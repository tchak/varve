//! The surface codec laws (§5, settled 2026-08-19): `from(to(x)) == x`
//! over generated surfaces, nodes, formats and block defaults — every
//! node kind, every format, rules on every slot, nested groups and
//! sections — and the decoder refuses alternative spellings. The wire
//! carries these bodies opaquely; this is where their strictness lives.

use proptest::prelude::*;
use varve_core::canonical::{CanonicalValue, hash_plain};
use varve_core::{BlockId, ColumnId, GroupId, OptionId, RevisionId, SurfaceId};
use varve_logic::{Atom, ColumnRef, Const, Expr, Operand};
use varve_schema::BlockRef;
use varve_surface::{
    BlockDefaults, ColumnNode, Format, GroupNode, Ineligibility, Node, Note, Section, Surface,
    WritePolicy, block_defaults_canonical, block_defaults_from, format_canonical, format_from,
    node_canonical, node_from, surface_canonical, surface_from,
};

// ------------------------------------------------------------ strategies

fn column_ref() -> impl Strategy<Value = ColumnRef> {
    ("[a-z][a-z0-9_]{0,6}", proptest::option::of("[a-z]{1,8}")).prop_map(|(column, field)| {
        ColumnRef {
            column: ColumnId::new(column),
            field,
        }
    })
}

/// A sample of atom kinds — the logic crate's own property suite covers
/// the full rule language; here rules only need to ride every slot.
fn atom() -> impl Strategy<Value = Atom> {
    prop_oneof![
        (column_ref(), any::<bool>()).prop_map(|(source, b)| Atom::Eq {
            source,
            right: Operand::Const(Const::Boolean(b)),
        }),
        (column_ref(), column_ref()).prop_map(|(source, c)| Atom::Lt {
            source,
            right: Operand::Column(c),
        }),
        column_ref().prop_map(|source| Atom::IsEmpty { source }),
        column_ref().prop_map(|source| Atom::IsFilled { source }),
        (column_ref(), "[a-z0-9]{1,6}").prop_map(|(source, o)| Atom::Contains {
            source,
            option: OptionId::new(o),
        }),
        "[a-z-]{1,10}".prop_map(|g| Atom::NotPending {
            group: GroupId::new(g)
        }),
    ]
}

fn expr() -> impl Strategy<Value = Expr> {
    atom()
        .prop_map(Expr::Atom)
        .prop_recursive(4, 16, 4, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..4).prop_map(Expr::And),
                proptest::collection::vec(inner, 0..4).prop_map(Expr::Or),
            ]
        })
}

fn rule() -> impl Strategy<Value = Option<Expr>> {
    proptest::option::of(expr())
}

fn text() -> impl Strategy<Value = Option<String>> {
    proptest::option::of("\\PC{0,12}")
}

fn format() -> impl Strategy<Value = Format> {
    prop_oneof![
        Just(Format::Email),
        Just(Format::Phone),
        Just(Format::Iban),
        "[a-z0-9\\[\\]+*?.-]{0,10}".prop_map(Format::Regex),
    ]
}

fn column_node() -> impl Strategy<Value = ColumnNode> {
    (
        "[a-z][a-z0-9_]{0,6}",
        text(),
        text(),
        rule(),
        rule(),
        any::<bool>(),
        any::<bool>(),
        proptest::option::of(format()),
    )
        .prop_map(
            |(column, prompt, help, visibility, required, writable, override_derived, format)| {
                ColumnNode {
                    column: ColumnId::new(column),
                    prompt,
                    help,
                    visibility,
                    required,
                    write: WritePolicy {
                        writable,
                        override_derived,
                    },
                    format,
                }
            },
        )
}

fn node() -> impl Strategy<Value = Node> {
    let leaf = prop_oneof![
        column_node().prop_map(Node::Column),
        (text(), "\\PC{0,16}").prop_map(|(title, body)| Node::Note(Note { title, body })),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            (
                "[a-z]{1,6}",
                text(),
                rule(),
                proptest::collection::vec(inner.clone(), 0..4)
            )
                .prop_map(|(group, prompt, visibility, children)| Node::Group(
                    GroupNode {
                        group: GroupId::new(group),
                        prompt,
                        visibility,
                        children,
                    }
                )),
            (
                "\\PC{0,12}",
                text(),
                rule(),
                proptest::collection::vec(inner, 0..4)
            )
                .prop_map(|(title, help, visibility, children)| Node::Section(
                    Section {
                        title,
                        help,
                        visibility,
                        children,
                    }
                )),
        ]
    })
}

fn surface() -> impl Strategy<Value = Surface> {
    (
        "[a-z][a-z0-9-]{0,8}",
        "[a-z0-9:]{1,12}",
        proptest::collection::vec(node(), 0..5),
        proptest::option::of((expr(), "\\PC{0,16}")),
    )
        .prop_map(|(id, revision, nodes, ineligibility)| Surface {
            id: SurfaceId::new(id),
            revision: RevisionId::new(revision),
            nodes,
            ineligibility: ineligibility.map(|(rule, message)| Ineligibility { rule, message }),
        })
}

fn block_defaults() -> impl Strategy<Value = BlockDefaults> {
    (
        "[a-z]{1,6}",
        any::<u32>(),
        "[a-z]{1,6}",
        text(),
        rule(),
        proptest::collection::vec(column_node().prop_map(Node::Column), 0..4),
    )
        .prop_map(
            |(block, version, group, prompt, visibility, children)| BlockDefaults {
                block: BlockRef {
                    id: BlockId::new(block),
                    version,
                },
                node: GroupNode {
                    group: GroupId::new(group),
                    prompt,
                    visibility,
                    children,
                },
            },
        )
}

// ---------------------------------------------------------------- laws

proptest! {
    #[test]
    fn format_round_trips(f in format()) {
        prop_assert_eq!(format_from(&format_canonical(&f)).unwrap(), f);
    }

    #[test]
    fn node_round_trips(n in node()) {
        let encoded = node_canonical(&n);
        let decoded = node_from(&encoded).unwrap();
        prop_assert_eq!(&decoded, &n);
        prop_assert_eq!(node_canonical(&decoded), encoded);
    }

    #[test]
    fn surface_round_trips(s in surface()) {
        let encoded = surface_canonical(&s);
        let decoded = surface_from(&encoded).unwrap();
        prop_assert_eq!(&decoded, &s);
        prop_assert_eq!(surface_canonical(&decoded), encoded);
    }

    /// The body is the hash preimage: the wire verifies a
    /// `block_defaults` line's `hash` as `hash_plain(body)` without
    /// reading the body.
    #[test]
    fn block_defaults_round_trip_and_hash(d in block_defaults()) {
        let encoded = block_defaults_canonical(&d);
        let decoded = block_defaults_from(&encoded).unwrap();
        prop_assert_eq!(&decoded, &d);
        prop_assert_eq!(block_defaults_canonical(&decoded), encoded.clone());
        prop_assert_eq!(hash_plain(&encoded).unwrap(), d.content_hash());
    }
}

// ----------------------------------------------------------- negatives

fn s(t: &str) -> CanonicalValue {
    CanonicalValue::String(t.into())
}

fn obj(pairs: Vec<(&str, CanonicalValue)>) -> CanonicalValue {
    CanonicalValue::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

#[test]
fn decoders_are_strict() {
    // Formats: names are lowercase, regex is the only object form.
    assert!(format_from(&s("Email")).is_err());
    assert!(format_from(&obj(vec![("pattern", s("a"))])).is_err());
    assert!(format_from(&obj(vec![("regex", s("a")), ("flags", s("i"))])).is_err());
    assert!(format_from(&CanonicalValue::Null).is_err());

    // Nodes: exactly one discriminating key, exactly the emitted keys.
    let column = |extra: Vec<(&str, CanonicalValue)>| {
        let mut pairs = vec![
            ("column", s("c")),
            ("prompt", CanonicalValue::Null),
            ("help", CanonicalValue::Null),
            ("visibility", CanonicalValue::Null),
            ("required", CanonicalValue::Null),
            ("writable", CanonicalValue::Bool(true)),
            ("override_derived", CanonicalValue::Bool(false)),
            ("format", CanonicalValue::Null),
        ];
        pairs.extend(extra);
        obj(pairs)
    };
    assert!(node_from(&column(vec![])).is_ok());
    assert!(
        node_from(&column(vec![("group", s("g"))])).is_err(),
        "two kinds"
    );
    assert!(
        node_from(&column(vec![("label", s("x"))])).is_err(),
        "stray key"
    );
    let mut missing = column(vec![]);
    if let CanonicalValue::Object(m) = &mut missing {
        m.remove("format");
    }
    assert!(
        node_from(&missing).is_err(),
        "optional means null, not absent"
    );
    // A rule slot takes a rule or null — never a string.
    assert!(node_from(&column(vec![("required", s("always"))])).is_err());
    assert!(
        node_from(&obj(vec![("note", s("body"))])).is_err(),
        "title is required (null allowed)"
    );
    assert!(
        node_from(&obj(vec![
            ("note", s("body")),
            ("title", CanonicalValue::Null)
        ]))
        .is_ok()
    );
    // Groups carry children; a child must itself be a node.
    assert!(
        node_from(&obj(vec![
            ("group", s("g")),
            ("prompt", CanonicalValue::Null),
            ("visibility", CanonicalValue::Null),
            ("children", CanonicalValue::Array(vec![s("not a node")])),
        ]))
        .is_err()
    );

    // Surfaces.
    let surface = |extra: Vec<(&str, CanonicalValue)>| {
        let mut pairs = vec![
            ("id", s("form")),
            ("revision", s("rev-1")),
            ("nodes", CanonicalValue::Array(vec![])),
            ("ineligibility", CanonicalValue::Null),
        ];
        pairs.extend(extra);
        obj(pairs)
    };
    assert!(surface_from(&surface(vec![])).is_ok());
    assert!(
        surface_from(&surface(vec![("k", s("surface"))])).is_err(),
        "envelope keys stay outside"
    );
    assert!(
        surface_from(&surface(vec![(
            "ineligibility",
            obj(vec![("message", s("non"))])
        )]))
        .is_err(),
        "ineligibility needs its rule"
    );

    // Block defaults: a group node, an integer version.
    assert!(
        block_defaults_from(&obj(vec![
            ("block", s("rib")),
            ("version", CanonicalValue::Int(1)),
            ("node", column(vec![])),
        ]))
        .is_err(),
        "defaults are a group node"
    );
    assert!(
        block_defaults_from(&obj(vec![
            ("block", s("rib")),
            ("version", s("1")),
            (
                "node",
                obj(vec![
                    ("group", s("rib")),
                    ("prompt", CanonicalValue::Null),
                    ("visibility", CanonicalValue::Null),
                    ("children", CanonicalValue::Array(vec![])),
                ])
            ),
        ]))
        .is_err()
    );
}
