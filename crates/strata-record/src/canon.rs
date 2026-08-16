//! Canonical (JSON-shaped) forms of ops, states and origins — the bytes
//! that per-op commitments commit to (§2.13 decision 4).
//!
//! Scalars are pre-rendered per §2.13 decision 3: decimals and instants
//! as their normalized strings. Geometry commits to its deterministic
//! serialized form (key-sorted); unifying that rendering with JCS float
//! rules across producers is the wire pass's job.

use std::collections::BTreeMap;

use strata_core::RowPath;
use strata_core::canonical::CanonicalValue;
use strata_value::{CellState, CellValue, Op, Scalar};

use crate::{Derivation, Origin};

fn obj(pairs: Vec<(&str, CanonicalValue)>) -> CanonicalValue {
    CanonicalValue::Object(
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn string(s: impl ToString) -> CanonicalValue {
    CanonicalValue::String(s.to_string())
}

fn path(p: &RowPath) -> CanonicalValue {
    CanonicalValue::Array(
        p.segments()
            .iter()
            .map(|seg| {
                CanonicalValue::Array(vec![string(&seg.group), string(&seg.item)])
            })
            .collect(),
    )
}

fn scalar(s: &Scalar) -> CanonicalValue {
    match s {
        Scalar::Text(v) => obj(vec![("text", string(v))]),
        Scalar::Boolean(v) => obj(vec![("boolean", CanonicalValue::Bool(*v))]),
        Scalar::Integer(v) => obj(vec![("integer", CanonicalValue::Int(*v))]),
        Scalar::Decimal(v) => obj(vec![("decimal", string(v))]),
        Scalar::Date(v) => obj(vec![("date", string(v))]),
        Scalar::Datetime(v) => obj(vec![("datetime", string(v))]),
        Scalar::Enum(v) => obj(vec![("option", string(v))]),
        Scalar::Attachment(a) => obj(vec![(
            "attachment",
            obj(vec![
                ("id", string(&a.id)),
                ("sha256", string(&a.sha256)),
                ("filename", string(&a.filename)),
            ]),
        )]),
        Scalar::Geometry(f) => obj(vec![("geometry", string(f))]),
    }
}

fn state(s: &CellState) -> CanonicalValue {
    match s {
        CellState::Empty => string("empty"),
        CellState::Value(CellValue::One(v)) => obj(vec![("one", scalar(v))]),
        CellState::Value(CellValue::Many(vs)) => obj(vec![(
            "many",
            CanonicalValue::Array(vs.iter().map(scalar).collect()),
        )]),
    }
}

pub fn op(op: &Op) -> CanonicalValue {
    match op {
        Op::Set {
            column,
            path: p,
            state: s,
        } => obj(vec![
            ("op", string("set")),
            ("column", string(column)),
            ("path", path(p)),
            ("state", state(s)),
        ]),
        Op::Unset { column, path: p } => obj(vec![
            ("op", string("unset")),
            ("column", string(column)),
            ("path", path(p)),
        ]),
        Op::AddItem {
            group,
            parent,
            item,
            at,
        } => obj(vec![
            ("op", string("add_item")),
            ("group", string(group)),
            ("parent", path(parent)),
            ("item", string(item)),
            ("at", CanonicalValue::Int(*at as i64)),
        ]),
        Op::RemoveItem {
            group,
            parent,
            item,
        } => obj(vec![
            ("op", string("remove_item")),
            ("group", string(group)),
            ("parent", path(parent)),
            ("item", string(item)),
        ]),
        Op::Reorder {
            group,
            parent,
            order,
        } => obj(vec![
            ("op", string("reorder")),
            ("group", string(group)),
            ("parent", path(parent)),
            (
                "order",
                CanonicalValue::Array(order.iter().map(string).collect()),
            ),
        ]),
    }
}

fn derivation(d: &Derivation) -> CanonicalValue {
    obj(vec![
        ("source", string(&d.source)),
        ("source_version", CanonicalValue::Int(d.source_version as i64)),
        ("mapping_version", CanonicalValue::Int(d.mapping_version as i64)),
        ("snapshot_ref", string(d.snapshot_ref)),
    ])
}

/// The entry's redactable metadata: origin and note (§2.13 decision 8 —
/// origin describes the values, so it is content, not envelope).
pub fn meta(origin: &Origin, note: Option<&str>) -> CanonicalValue {
    let origin = match origin {
        Origin::Entered => string("entered"),
        Origin::Derived(d) => obj(vec![("derived", derivation(d))]),
        Origin::Overridden { superseded } => obj(vec![(
            "overridden",
            match superseded {
                None => CanonicalValue::Null,
                Some(d) => derivation(d),
            },
        )]),
    };
    obj(vec![
        ("origin", origin),
        (
            "note",
            match note {
                None => CanonicalValue::Null,
                Some(n) => string(n),
            },
        ),
    ])
}
