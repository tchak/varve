//! Shared helper: arbitrary bytes → JSON → `CanonicalValue`, under the
//! wire reader's number rule (integer literals within ±(2^53 − 1) are
//! `Int`, everything else a double).

use varve_core::canonical::{CanonicalValue, MAX_SAFE_INTEGER};

pub fn canonical_from_bytes(data: &[u8]) -> Option<CanonicalValue> {
    let json: serde_json::Value = serde_json::from_slice(data).ok()?;
    Some(to_canonical(&json))
}

fn to_canonical(v: &serde_json::Value) -> CanonicalValue {
    match v {
        serde_json::Value::Null => CanonicalValue::Null,
        serde_json::Value::Bool(b) => CanonicalValue::Bool(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) if i.unsigned_abs() <= MAX_SAFE_INTEGER as u64 => CanonicalValue::Int(i),
            _ => CanonicalValue::Float(n.as_f64().unwrap_or(0.0)),
        },
        serde_json::Value::String(s) => CanonicalValue::String(s.clone()),
        serde_json::Value::Array(a) => CanonicalValue::Array(a.iter().map(to_canonical).collect()),
        serde_json::Value::Object(o) => {
            CanonicalValue::Object(o.iter().map(|(k, v)| (k.clone(), to_canonical(v))).collect())
        }
    }
}
