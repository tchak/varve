//! Canonical (JSON-shaped) form of expressions: what surfaces hash and
//! the wire carries. Hand-mapped like `varve-record`'s op canon — no
//! serde on internal types (§9). `from_canonical` is the parse
//! direction untrusted import bytes eventually reach: strict, total,
//! and a fuzz target once `varve-wire` exists.

use std::collections::BTreeMap;

use varve_core::canonical::CanonicalValue;
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, GroupId, OptionId};
use varve_schema::Unit;

use crate::{Atom, ColumnRef, Const, Expr, Operand};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed expression: {0}")]
pub struct DecodeError(pub String);

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

pub fn to_canonical(expr: &Expr) -> CanonicalValue {
    match expr {
        Expr::And(operands) => obj(vec![(
            "and",
            CanonicalValue::Array(operands.iter().map(to_canonical).collect()),
        )]),
        Expr::Or(operands) => obj(vec![(
            "or",
            CanonicalValue::Array(operands.iter().map(to_canonical).collect()),
        )]),
        Expr::Atom(atom) => atom_to_canonical(atom),
    }
}

fn column_ref(source: &ColumnRef) -> CanonicalValue {
    let mut pairs = vec![("column", string(&source.column))];
    if let Some(field) = &source.field {
        pairs.push(("field", string(field)));
    }
    obj(pairs)
}

fn const_to_canonical(constant: &Const) -> CanonicalValue {
    match constant {
        Const::Boolean(b) => obj(vec![("boolean", CanonicalValue::Bool(*b))]),
        Const::Number { value, unit } => {
            let mut pairs = vec![("value", string(value))];
            if let Some(unit) = unit {
                pairs.push(("unit", string(unit.name())));
            }
            obj(vec![("number", obj(pairs))])
        }
        Const::Date(d) => obj(vec![("date", string(d))]),
        Const::Datetime(t) => obj(vec![("datetime", string(t))]),
        Const::Option(o) => obj(vec![("option", string(o))]),
        Const::Text(t) => obj(vec![("text", string(t))]),
    }
}

fn operand(right: &Operand) -> CanonicalValue {
    match right {
        Operand::Const(c) => obj(vec![("const", const_to_canonical(c))]),
        Operand::Column(c) => obj(vec![("column_ref", column_ref(c))]),
    }
}

fn atom_to_canonical(atom: &Atom) -> CanonicalValue {
    let comparison = |op: &'static str, source: &ColumnRef, right: &Operand| {
        obj(vec![
            ("op", string(op)),
            ("source", column_ref(source)),
            ("right", operand(right)),
        ])
    };
    match atom {
        Atom::Eq { source, right } => comparison("eq", source, right),
        Atom::NotEq { source, right } => comparison("not_eq", source, right),
        Atom::Lt { source, right } => comparison("lt", source, right),
        Atom::Le { source, right } => comparison("le", source, right),
        Atom::Gt { source, right } => comparison("gt", source, right),
        Atom::Ge { source, right } => comparison("ge", source, right),
        Atom::IsEmpty { source } => {
            obj(vec![("op", string("is_empty")), ("source", column_ref(source))])
        }
        Atom::IsFilled { source } => {
            obj(vec![("op", string("is_filled")), ("source", column_ref(source))])
        }
        Atom::Contains { source, option } => obj(vec![
            ("op", string("contains")),
            ("source", column_ref(source)),
            ("option", string(option)),
        ]),
        Atom::Excludes { source, option } => obj(vec![
            ("op", string("excludes")),
            ("source", column_ref(source)),
            ("option", string(option)),
        ]),
        Atom::Pending { group } => obj(vec![
            ("op", string("pending")),
            ("group", string(group)),
        ]),
        Atom::NotPending { group } => obj(vec![
            ("op", string("not_pending")),
            ("group", string(group)),
        ]),
    }
}

/// Strict: exactly the keys the canonical form emits, no more — so
/// `to_canonical ∘ from_canonical` is the identity on every accepted
/// value and no two texts decode to one expression.
pub fn from_canonical(value: &CanonicalValue) -> Result<Expr, DecodeError> {
    expr_from(value, 0)
}

fn expr_from(value: &CanonicalValue, depth: usize) -> Result<Expr, DecodeError> {
    if depth > crate::MAX_DEPTH {
        return Err(DecodeError(format!("expression nests deeper than {}", crate::MAX_DEPTH)));
    }
    let map = as_object(value)?;
    match (map.get("and"), map.get("or")) {
        (Some(_), Some(_)) => Err(DecodeError("an expression is 'and' or 'or', not both".into())),
        (Some(operands), None) => {
            only_keys(map, &["and"])?;
            Ok(Expr::And(exprs(operands, depth + 1)?))
        }
        (None, Some(operands)) => {
            only_keys(map, &["or"])?;
            Ok(Expr::Or(exprs(operands, depth + 1)?))
        }
        (None, None) => Ok(Expr::Atom(atom_from(map)?)),
    }
}

/// Refuse keys the canonical form never emits.
fn only_keys(map: &Object, allowed: &[&str]) -> Result<(), DecodeError> {
    match map.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(extra) => Err(DecodeError(format!("unexpected key '{extra}'"))),
        None => Ok(()),
    }
}

/// A tagged union: exactly one key.
fn single_key<'a>(map: &'a Object, what: &str) -> Result<(&'a String, &'a CanonicalValue), DecodeError> {
    let mut it = map.iter();
    match (it.next(), it.next()) {
        (Some(entry), None) => Ok(entry),
        (None, _) => Err(DecodeError(format!("empty {what}"))),
        (Some(_), Some(_)) => Err(DecodeError(format!("{what} must have exactly one key"))),
    }
}

fn exprs(value: &CanonicalValue, depth: usize) -> Result<Vec<Expr>, DecodeError> {
    let CanonicalValue::Array(items) = value else {
        return Err(DecodeError("combinator operands must be an array".into()));
    };
    items.iter().map(|v| expr_from(v, depth)).collect()
}

type Object = BTreeMap<String, CanonicalValue>;

fn as_object(value: &CanonicalValue) -> Result<&Object, DecodeError> {
    match value {
        CanonicalValue::Object(map) => Ok(map),
        _ => Err(DecodeError("expected an object".into())),
    }
}

fn text(map: &Object, key: &str) -> Result<String, DecodeError> {
    match map.get(key) {
        Some(CanonicalValue::String(s)) => Ok(s.clone()),
        _ => Err(DecodeError(format!("missing string field '{key}'"))),
    }
}

fn column_ref_from(value: &CanonicalValue) -> Result<ColumnRef, DecodeError> {
    let source = as_object(value)?;
    only_keys(source, &["column", "field"])?;
    Ok(ColumnRef {
        column: ColumnId::new(text(source, "column")?),
        field: match source.get("field") {
            None => None,
            Some(CanonicalValue::String(s)) => Some(s.clone()),
            Some(_) => return Err(DecodeError("'field' must be a string".into())),
        },
    })
}

fn source_from(map: &Object) -> Result<ColumnRef, DecodeError> {
    column_ref_from(map.get("source").ok_or_else(|| DecodeError("missing 'source'".into()))?)
}

fn const_from(value: &CanonicalValue) -> Result<Const, DecodeError> {
    let map = as_object(value)?;
    let (kind, inner) = single_key(map, "constant")?;
    Ok(match kind.as_str() {
        "boolean" => match inner {
            CanonicalValue::Bool(b) => Const::Boolean(*b),
            _ => return Err(DecodeError("'boolean' must be a bool".into())),
        },
        "number" => {
            let number = as_object(inner)?;
            only_keys(number, &["value", "unit"])?;
            let value = Decimal::parse(&text(number, "value")?)
                .map_err(|e| DecodeError(format!("bad number: {e}")))?;
            let unit = match number.get("unit") {
                None => None,
                Some(CanonicalValue::String(name)) => Some(
                    Unit::parse(name)
                        .ok_or_else(|| DecodeError(format!("unknown unit '{name}'")))?,
                ),
                Some(_) => return Err(DecodeError("'unit' must be a string".into())),
            };
            Const::Number { value, unit }
        }
        "date" => Const::Date(
            Date::parse(&string_of(inner)?)
                .map_err(|e| DecodeError(format!("bad date: {e}")))?,
        ),
        "datetime" => Const::Datetime(
            Instant::parse(&string_of(inner)?)
                .map_err(|e| DecodeError(format!("bad datetime: {e}")))?,
        ),
        "option" => Const::Option(OptionId::new(string_of(inner)?)),
        "text" => Const::Text(string_of(inner)?),
        other => return Err(DecodeError(format!("unknown constant kind '{other}'"))),
    })
}

fn string_of(value: &CanonicalValue) -> Result<String, DecodeError> {
    match value {
        CanonicalValue::String(s) => Ok(s.clone()),
        _ => Err(DecodeError("expected a string".into())),
    }
}

fn operand_from(map: &Object) -> Result<Operand, DecodeError> {
    let right = as_object(map.get("right").ok_or_else(|| DecodeError("missing 'right'".into()))?)?;
    match single_key(right, "operand")? {
        (k, constant) if k == "const" => Ok(Operand::Const(const_from(constant)?)),
        (k, column) if k == "column_ref" => Ok(Operand::Column(column_ref_from(column)?)),
        _ => Err(DecodeError("operand must be 'const' or 'column_ref'".into())),
    }
}

fn atom_from(map: &Object) -> Result<Atom, DecodeError> {
    let op = text(map, "op")?;
    let comparison = |ctor: fn(ColumnRef, Operand) -> Atom| -> Result<Atom, DecodeError> {
        only_keys(map, &["op", "source", "right"])?;
        Ok(ctor(source_from(map)?, operand_from(map)?))
    };
    match op.as_str() {
        "is_empty" | "is_filled" => only_keys(map, &["op", "source"])?,
        "contains" | "excludes" => only_keys(map, &["op", "source", "option"])?,
        "pending" | "not_pending" => only_keys(map, &["op", "group"])?,
        _ => {}
    }
    match op.as_str() {
        "eq" => comparison(|source, right| Atom::Eq { source, right }),
        "not_eq" => comparison(|source, right| Atom::NotEq { source, right }),
        "lt" => comparison(|source, right| Atom::Lt { source, right }),
        "le" => comparison(|source, right| Atom::Le { source, right }),
        "gt" => comparison(|source, right| Atom::Gt { source, right }),
        "ge" => comparison(|source, right| Atom::Ge { source, right }),
        "is_empty" => Ok(Atom::IsEmpty { source: source_from(map)? }),
        "is_filled" => Ok(Atom::IsFilled { source: source_from(map)? }),
        "contains" => Ok(Atom::Contains {
            source: source_from(map)?,
            option: OptionId::new(text(map, "option")?),
        }),
        "excludes" => Ok(Atom::Excludes {
            source: source_from(map)?,
            option: OptionId::new(text(map, "option")?),
        }),
        "pending" => Ok(Atom::Pending { group: GroupId::new(text(map, "group")?) }),
        "not_pending" => Ok(Atom::NotPending { group: GroupId::new(text(map, "group")?) }),
        other => Err(DecodeError(format!("unknown op '{other}'"))),
    }
}
