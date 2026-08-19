//! Canonical forms of surface-side objects — surfaces, nodes, formats,
//! block defaults — and their strict decoders. This crate owns the
//! codec (§5, settled 2026-08-19): on the wire a `surface` or
//! `block_defaults` line carries an envelope typed by `varve-wire` and
//! a body `varve-wire` does not interpret; the body is exactly what
//! these functions produce and accept, and a Tier 5 exporter/importer
//! joins the two. Nothing depends on this crate (§7) — the invariant
//! holds for the wire too.
//!
//! Round-trip is the law: `from(to(x)) == x`, and re-encoding the
//! decoded value gives the same canonical value (one object, one text).
//! Decoders are strict and total: exactly the keys the encoder emits,
//! every alternative spelling refused.

use std::collections::BTreeMap;

use varve_core::canonical::{CanonicalValue, ContentHash, hash_plain};
use varve_core::{ColumnId, GroupId, RevisionId, SurfaceId};
use varve_logic::{Expr, from_canonical, to_canonical};
use varve_schema::BlockRef;

use crate::block::BlockDefaults;
use crate::{
    ColumnNode, Format, GroupNode, Ineligibility, Node, Note, Section, Surface, WritePolicy,
};

type Obj = BTreeMap<String, CanonicalValue>;

fn obj(pairs: Vec<(&str, CanonicalValue)>) -> CanonicalValue {
    CanonicalValue::Object(
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<BTreeMap<_, _>>(),
    )
}

pub(crate) fn string(s: impl ToString) -> CanonicalValue {
    CanonicalValue::String(s.to_string())
}

fn opt(v: &Option<String>) -> CanonicalValue {
    match v {
        None => CanonicalValue::Null,
        Some(t) => string(t),
    }
}

fn rule(r: &Option<Expr>) -> CanonicalValue {
    match r {
        None => CanonicalValue::Null,
        Some(e) => to_canonical(e),
    }
}

// ------------------------------------------------------------ encoders

/// Canonical form of a format constraint (§2.13 decision 7: shapes live
/// in code, never `Debug`).
pub fn format_canonical(format: &Format) -> CanonicalValue {
    match format {
        Format::Email => string("email"),
        Format::Phone => string("phone"),
        Format::Iban => string("iban"),
        Format::Regex(pattern) => obj(vec![("regex", string(pattern))]),
    }
}

/// Canonical form of a surface node — identity-bearing content of a
/// surface fragment (a changed rule or prompt is a new version).
pub fn node_canonical(n: &Node) -> CanonicalValue {
    match n {
        Node::Column(c) => obj(vec![
            ("column", string(&c.column)),
            ("prompt", opt(&c.prompt)),
            ("help", opt(&c.help)),
            ("visibility", rule(&c.visibility)),
            ("required", rule(&c.required)),
            ("writable", CanonicalValue::Bool(c.write.writable)),
            (
                "override_derived",
                CanonicalValue::Bool(c.write.override_derived),
            ),
            (
                "format",
                match &c.format {
                    None => CanonicalValue::Null,
                    Some(f) => format_canonical(f),
                },
            ),
        ]),
        Node::Group(g) => obj(vec![
            ("group", string(&g.group)),
            ("prompt", opt(&g.prompt)),
            ("visibility", rule(&g.visibility)),
            (
                "children",
                CanonicalValue::Array(g.children.iter().map(node_canonical).collect()),
            ),
        ]),
        Node::Section(sec) => obj(vec![
            ("section", string(&sec.title)),
            ("help", opt(&sec.help)),
            ("visibility", rule(&sec.visibility)),
            (
                "children",
                CanonicalValue::Array(sec.children.iter().map(node_canonical).collect()),
            ),
        ]),
        Node::Note(note) => obj(vec![
            ("note", string(&note.body)),
            ("title", opt(&note.title)),
        ]),
    }
}

/// Canonical form of a whole surface: the body of a `surface` wire line
/// (§5). Self-contained — it carries its own id and revision, which the
/// line's envelope repeats for typed reading; an importer checks they
/// agree.
pub fn surface_canonical(s: &Surface) -> CanonicalValue {
    obj(vec![
        ("id", string(&s.id)),
        ("revision", string(&s.revision)),
        (
            "nodes",
            CanonicalValue::Array(s.nodes.iter().map(node_canonical).collect()),
        ),
        (
            "ineligibility",
            match &s.ineligibility {
                None => CanonicalValue::Null,
                Some(i) => obj(vec![
                    ("rule", to_canonical(&i.rule)),
                    ("message", string(&i.message)),
                ]),
            },
        ),
    ])
}

/// Canonical form of block defaults: the body of a `block_defaults`
/// wire line (§5) and the preimage of [`BlockDefaults::content_hash`] —
/// so the wire verifies the line's `hash` as `hash_plain(body)` without
/// reading the body.
pub fn block_defaults_canonical(d: &BlockDefaults) -> CanonicalValue {
    obj(vec![
        ("block", string(&d.block.id)),
        ("version", CanonicalValue::Int(i64::from(d.block.version))),
        ("node", node_canonical(&Node::Group(d.node.clone()))),
    ])
}

impl BlockDefaults {
    /// Content address of the defaults (plain regime — a surface
    /// fragment, like a surface). A changed rule or prompt is a new
    /// version of the defaults.
    pub fn content_hash(&self) -> ContentHash {
        hash_plain(&block_defaults_canonical(self)).expect("surface fragments carry no floats")
    }
}

// ------------------------------------------------------------ decoders

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed surface data: {0}")]
pub struct SurfaceDecodeError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, SurfaceDecodeError> {
    Err(SurfaceDecodeError(msg.into()))
}

fn as_obj(v: &CanonicalValue) -> Result<&Obj, SurfaceDecodeError> {
    match v {
        CanonicalValue::Object(m) => Ok(m),
        _ => err("expected an object"),
    }
}

fn as_arr(v: &CanonicalValue) -> Result<&[CanonicalValue], SurfaceDecodeError> {
    match v {
        CanonicalValue::Array(a) => Ok(a),
        _ => err("expected an array"),
    }
}

/// Strict decoding: exactly the keys the canonical form emits.
fn only_keys(m: &Obj, allowed: &[&str]) -> Result<(), SurfaceDecodeError> {
    if let Some(extra) = m.keys().find(|k| !allowed.contains(&k.as_str())) {
        return err(format!("unexpected key '{extra}'"));
    }
    if let Some(missing) = allowed.iter().find(|k| !m.contains_key(**k)) {
        return err(format!("missing key '{missing}'"));
    }
    Ok(())
}

fn get<'a>(m: &'a Obj, key: &str) -> Result<&'a CanonicalValue, SurfaceDecodeError> {
    m.get(key)
        .ok_or_else(|| SurfaceDecodeError(format!("missing '{key}'")))
}

fn get_str(m: &Obj, key: &str) -> Result<String, SurfaceDecodeError> {
    match m.get(key) {
        Some(CanonicalValue::String(s)) => Ok(s.clone()),
        _ => err(format!("missing string '{key}'")),
    }
}

fn get_bool(m: &Obj, key: &str) -> Result<bool, SurfaceDecodeError> {
    match m.get(key) {
        Some(CanonicalValue::Bool(b)) => Ok(*b),
        _ => err(format!("missing boolean '{key}'")),
    }
}

fn opt_str(m: &Obj, key: &str) -> Result<Option<String>, SurfaceDecodeError> {
    match get(m, key)? {
        CanonicalValue::Null => Ok(None),
        CanonicalValue::String(s) => Ok(Some(s.clone())),
        _ => err(format!("'{key}' must be a string or null")),
    }
}

fn opt_rule(m: &Obj, key: &str) -> Result<Option<Expr>, SurfaceDecodeError> {
    match get(m, key)? {
        CanonicalValue::Null => Ok(None),
        v => from_canonical(v)
            .map(Some)
            .map_err(|e| SurfaceDecodeError(format!("'{key}': {e}"))),
    }
}

pub fn format_from(v: &CanonicalValue) -> Result<Format, SurfaceDecodeError> {
    match v {
        CanonicalValue::String(s) => match s.as_str() {
            "email" => Ok(Format::Email),
            "phone" => Ok(Format::Phone),
            "iban" => Ok(Format::Iban),
            other => err(format!("unknown format '{other}'")),
        },
        CanonicalValue::Object(m) => {
            only_keys(m, &["regex"])?;
            Ok(Format::Regex(get_str(m, "regex")?))
        }
        _ => err("a format is a name or {\"regex\": ...}"),
    }
}

fn nodes_from(v: &CanonicalValue) -> Result<Vec<Node>, SurfaceDecodeError> {
    as_arr(v)?.iter().map(node_from).collect()
}

pub fn node_from(v: &CanonicalValue) -> Result<Node, SurfaceDecodeError> {
    let m = as_obj(v)?;
    // The discriminating key: exactly one of the four is present.
    let kinds: Vec<&str> = ["column", "group", "section", "note"]
        .into_iter()
        .filter(|k| m.contains_key(*k))
        .collect();
    let [kind] = kinds.as_slice() else {
        return err("a node is exactly one of column, group, section, note");
    };
    Ok(match *kind {
        "column" => {
            only_keys(
                m,
                &[
                    "column",
                    "prompt",
                    "help",
                    "visibility",
                    "required",
                    "writable",
                    "override_derived",
                    "format",
                ],
            )?;
            Node::Column(ColumnNode {
                column: ColumnId::new(get_str(m, "column")?),
                prompt: opt_str(m, "prompt")?,
                help: opt_str(m, "help")?,
                visibility: opt_rule(m, "visibility")?,
                required: opt_rule(m, "required")?,
                write: WritePolicy {
                    writable: get_bool(m, "writable")?,
                    override_derived: get_bool(m, "override_derived")?,
                },
                format: match get(m, "format")? {
                    CanonicalValue::Null => None,
                    f => Some(format_from(f)?),
                },
            })
        }
        "group" => {
            only_keys(m, &["group", "prompt", "visibility", "children"])?;
            Node::Group(GroupNode {
                group: GroupId::new(get_str(m, "group")?),
                prompt: opt_str(m, "prompt")?,
                visibility: opt_rule(m, "visibility")?,
                children: nodes_from(get(m, "children")?)?,
            })
        }
        "section" => {
            only_keys(m, &["section", "help", "visibility", "children"])?;
            Node::Section(Section {
                title: get_str(m, "section")?,
                help: opt_str(m, "help")?,
                visibility: opt_rule(m, "visibility")?,
                children: nodes_from(get(m, "children")?)?,
            })
        }
        "note" => {
            only_keys(m, &["note", "title"])?;
            Node::Note(Note {
                title: opt_str(m, "title")?,
                body: get_str(m, "note")?,
            })
        }
        _ => unreachable!("filtered above"),
    })
}

/// Decode a `surface` line body (§5). Structural only: that the surface
/// fits its revision is `validate()`'s job, at import, with the
/// revision in hand.
pub fn surface_from(v: &CanonicalValue) -> Result<Surface, SurfaceDecodeError> {
    let m = as_obj(v)?;
    only_keys(m, &["id", "revision", "nodes", "ineligibility"])?;
    Ok(Surface {
        id: SurfaceId::new(get_str(m, "id")?),
        revision: RevisionId::new(get_str(m, "revision")?),
        nodes: nodes_from(get(m, "nodes")?)?,
        ineligibility: match get(m, "ineligibility")? {
            CanonicalValue::Null => None,
            v => {
                let i = as_obj(v)?;
                only_keys(i, &["rule", "message"])?;
                Some(Ineligibility {
                    rule: from_canonical(get(i, "rule")?)
                        .map_err(|e| SurfaceDecodeError(format!("ineligibility rule: {e}")))?,
                    message: get_str(i, "message")?,
                })
            }
        },
    })
}

/// Decode a `block_defaults` line body (§5). The body's group node is
/// the defaults' node; `BlockDefaults::validate` checks it against the
/// block at import.
pub fn block_defaults_from(v: &CanonicalValue) -> Result<BlockDefaults, SurfaceDecodeError> {
    let m = as_obj(v)?;
    only_keys(m, &["block", "version", "node"])?;
    let version = match m.get("version") {
        Some(CanonicalValue::Int(i)) => {
            u32::try_from(*i).map_err(|_| SurfaceDecodeError("bad version".into()))?
        }
        _ => return err("missing integer 'version'"),
    };
    let Node::Group(node) = node_from(get(m, "node")?)? else {
        return err("block defaults are a group node");
    };
    Ok(BlockDefaults {
        block: BlockRef {
            id: varve_core::BlockId::new(get_str(m, "block")?),
            version,
        },
        node,
    })
}
