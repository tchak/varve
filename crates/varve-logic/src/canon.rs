//! Canonical (JSON-shaped) form of expressions: what surfaces hash and
//! the wire carries. Hand-mapped like `varve-record`'s op canon — no
//! serde on internal types (§9). `from_canonical` is the parse
//! direction untrusted import bytes eventually reach: strict, total,
//! and a fuzz target once `varve-wire` exists.

use std::collections::BTreeMap;

use varve_core::canonical::CanonicalValue;
use varve_core::primitives::{Date, Decimal, Instant};
use varve_core::{ColumnId, OptionId, ResolverId};
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
        Atom::Pending { resolver } => obj(vec![
            ("op", string("pending")),
            ("resolver", string(resolver)),
        ]),
        Atom::NotPending { resolver } => obj(vec![
            ("op", string("not_pending")),
            ("resolver", string(resolver)),
        ]),
    }
}

pub fn from_canonical(value: &CanonicalValue) -> Result<Expr, DecodeError> {
    let map = as_object(value)?;
    if let Some(operands) = map.get("and") {
        return Ok(Expr::And(exprs(operands)?));
    }
    if let Some(operands) = map.get("or") {
        return Ok(Expr::Or(exprs(operands)?));
    }
    Ok(Expr::Atom(atom_from(map)?))
}

fn exprs(value: &CanonicalValue) -> Result<Vec<Expr>, DecodeError> {
    let CanonicalValue::Array(items) = value else {
        return Err(DecodeError("combinator operands must be an array".into()));
    };
    items.iter().map(from_canonical).collect()
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

fn source_from(map: &Object) -> Result<ColumnRef, DecodeError> {
    let source = as_object(
        map.get("source")
            .ok_or_else(|| DecodeError("missing 'source'".into()))?,
    )?;
    Ok(ColumnRef {
        column: ColumnId::new(text(source, "column")?),
        field: match source.get("field") {
            None => None,
            Some(CanonicalValue::String(s)) => Some(s.clone()),
            Some(_) => return Err(DecodeError("'field' must be a string".into())),
        },
    })
}

fn const_from(value: &CanonicalValue) -> Result<Const, DecodeError> {
    let map = as_object(value)?;
    let (kind, inner) = map
        .iter()
        .next()
        .ok_or_else(|| DecodeError("empty constant".into()))?;
    Ok(match kind.as_str() {
        "boolean" => match inner {
            CanonicalValue::Bool(b) => Const::Boolean(*b),
            _ => return Err(DecodeError("'boolean' must be a bool".into())),
        },
        "number" => {
            let number = as_object(inner)?;
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
    let right = as_object(
        map.get("right")
            .ok_or_else(|| DecodeError("missing 'right'".into()))?,
    )?;
    if let Some(constant) = right.get("const") {
        return Ok(Operand::Const(const_from(constant)?));
    }
    if let Some(column) = right.get("column_ref") {
        let column = as_object(column)?;
        return Ok(Operand::Column(ColumnRef {
            column: ColumnId::new(text(column, "column")?),
            field: match column.get("field") {
                None => None,
                Some(CanonicalValue::String(s)) => Some(s.clone()),
                Some(_) => return Err(DecodeError("'field' must be a string".into())),
            },
        }));
    }
    Err(DecodeError("operand must be 'const' or 'column_ref'".into()))
}

fn atom_from(map: &Object) -> Result<Atom, DecodeError> {
    let op = text(map, "op")?;
    let comparison = |ctor: fn(ColumnRef, Operand) -> Atom| -> Result<Atom, DecodeError> {
        Ok(ctor(source_from(map)?, operand_from(map)?))
    };
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
        "pending" => Ok(Atom::Pending {
            resolver: ResolverId::new(text(map, "resolver")?),
        }),
        "not_pending" => Ok(Atom::NotPending {
            resolver: ResolverId::new(text(map, "resolver")?),
        }),
        other => Err(DecodeError(format!("unknown op '{other}'"))),
    }
}
