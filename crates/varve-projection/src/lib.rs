//! Tier 2 (§7): records viewed through a revision they weren't written
//! on. Casts applied, lossiness reported — never silently (§5.5).
//!
//! The shape is Avro's reader/writer resolution (§3): a per-column
//! function over stable column ids. Cells are revision-agnostic; only
//! interpretation is revision-dependent — so projection is a no-op over
//! the vast majority of any record, and says so in its report.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use varve_core::primitives::Decimal;
use varve_core::{ColumnId, OptionId};
use varve_schema::{
    Arity, CastError, ColumnInfo, NomenclatureTable, ScalarType, Schema,
    SchemaIndex, Unit, column_cast, conversion, nomenclature_rows,
};
use varve_value::{CellState, CellValue, RecordValues, Scalar};

/// Projected values plus the report that keeps lossiness loud.
#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    pub values: RecordValues,
    pub report: ProjectionReport,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectionReport {
    pub columns: BTreeMap<ColumnId, ColumnReport>,
    /// Writer-only columns: ignored by this reader, retained in storage
    /// (§3 "column removed — free").
    pub ignored_writer_columns: Vec<ColumnId>,
    /// Item lists whose group the reader no longer has (or has at a
    /// different scope).
    pub dropped_item_lists: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnReport {
    pub status: ColumnStatus,
    pub cells_projected: u64,
    /// Cells where information was actually lost (many→one truncation,
    /// datetime→date with a nonzero time…), not merely could have been.
    pub cells_lossy: u64,
    /// Cells a checked cast rejected: omitted from the projection,
    /// counted here — this is the number the impact report wants (§7).
    pub cells_failed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnStatus {
    /// Same type, same arity, same scope: copied.
    Identity,
    /// A cast ran (widening, lossy, or checked — see the cell counts).
    Cast,
    /// Reader-only column: every record simply reads absent (§3).
    AddedAbsent,
    /// §3 correction: the column moved into or out of a `many` group —
    /// its row-path arity changed. Breaking; cells not projected.
    ScopeMoved,
    /// No cast exists between the types (§3 "probably breaking").
    Forbidden,
}

impl ProjectionReport {
    /// §5.5: an aggregate export must carry exactly these totals.
    pub fn total_lossy(&self) -> u64 {
        self.columns.values().map(|c| c.cells_lossy).sum()
    }

    pub fn total_failed(&self) -> u64 {
        self.columns.values().map(|c| c.cells_failed).sum()
    }

    pub fn is_clean(&self) -> bool {
        self.total_lossy() == 0
            && self.total_failed() == 0
            && self.dropped_item_lists == 0
            && !self
                .columns
                .values()
                .any(|c| matches!(c.status, ColumnStatus::ScopeMoved | ColumnStatus::Forbidden))
    }
}

/// Project `values`, written under `writer`, into `reader`'s view.
///
/// The lens for enum labels is the *writer's* nomenclature: that is
/// what the stored option ids mean (§2.11).
pub fn project(
    values: &RecordValues,
    writer: &Schema,
    reader: &Schema,
    nomenclatures: &NomenclatureTable,
) -> Result<Projection, CastError> {
    let writer_index = SchemaIndex::build(writer);
    let reader_index = SchemaIndex::build(reader);
    let mut out = RecordValues::new();
    let mut report = ProjectionReport::default();

    for (column, rinfo) in &reader_index.columns {
        let Some(winfo) = writer_index.columns.get(column) else {
            report.columns.insert(
                column.clone(),
                ColumnReport {
                    status: ColumnStatus::AddedAbsent,
                    cells_projected: 0,
                    cells_lossy: 0,
                    cells_failed: 0,
                },
            );
            continue;
        };
        if winfo.scope != rinfo.scope {
            report.columns.insert(
                column.clone(),
                ColumnReport {
                    status: ColumnStatus::ScopeMoved,
                    cells_projected: 0,
                    cells_lossy: 0,
                    cells_failed: 0,
                },
            );
            continue;
        }
        let cast = column_cast(
            (&winfo.ty, winfo.arity),
            (&rinfo.ty, rinfo.arity),
            nomenclatures,
        )?;
        let mut column_report = ColumnReport {
            status: if !cast.possible {
                ColumnStatus::Forbidden
            } else if cast.class() == varve_schema::CastClass::Identity {
                ColumnStatus::Identity
            } else {
                ColumnStatus::Cast
            },
            cells_projected: 0,
            cells_lossy: 0,
            cells_failed: 0,
        };
        if cast.possible {
            for (addr, state) in values.cells.iter().filter(|(a, _)| a.column == *column) {
                match project_state(state, winfo, rinfo, nomenclatures)? {
                    Outcome::Kept { state, lossy } => {
                        column_report.cells_projected += 1;
                        if lossy {
                            column_report.cells_lossy += 1;
                        }
                        out.cells.insert(addr.clone(), state);
                    }
                    Outcome::Failed => column_report.cells_failed += 1,
                }
            }
        }
        report.columns.insert(column.clone(), column_report);
    }

    for column in writer_index.columns.keys() {
        if !reader_index.columns.contains_key(column) {
            report.ignored_writer_columns.push(column.clone());
        }
    }

    for (addr, items) in &values.items {
        let keep = reader_index.groups.get(&addr.group).is_some_and(|g| {
            g.cardinality == varve_schema::Cardinality::Many
                && addr
                    .parent
                    .segments()
                    .iter()
                    .map(|s| &s.group)
                    .eq(g.parent_scope.iter())
        });
        if keep {
            out.items.insert(addr.clone(), items.clone());
        } else {
            report.dropped_item_lists += 1;
        }
    }

    Ok(Projection {
        values: out,
        report,
    })
}

enum Outcome {
    Kept { state: CellState, lossy: bool },
    Failed,
}

fn project_state(
    state: &CellState,
    writer: &ColumnInfo,
    reader: &ColumnInfo,
    nomenclatures: &NomenclatureTable,
) -> Result<Outcome, CastError> {
    let CellState::Value(value) = state else {
        // Empty is empty under every interpretation (§2.4).
        return Ok(Outcome::Kept {
            state: CellState::Empty,
            lossy: false,
        });
    };

    // Arity resolution first (§3): one→many wraps, many→one truncates.
    let (scalars, mut lossy): (Vec<&Scalar>, bool) = match (value, reader.arity) {
        (CellValue::One(s), _) => (vec![s], false),
        (CellValue::Many(list), Arity::Many) => (list.iter().collect(), false),
        (CellValue::Many(list), Arity::One) => {
            (list.iter().take(1).collect(), list.len() > 1)
        }
    };

    let mut projected = Vec::with_capacity(scalars.len());
    for scalar in scalars {
        match project_scalar(scalar, &writer.ty, &reader.ty, nomenclatures)? {
            Some((s, l)) => {
                lossy |= l;
                projected.push(s);
            }
            // Strict: one failing element fails the cell.
            None => return Ok(Outcome::Failed),
        }
    }

    let state = match reader.arity {
        Arity::Many => CellState::Value(CellValue::Many(projected)),
        Arity::One => match projected.into_iter().next() {
            Some(s) => CellState::Value(CellValue::One(s)),
            // A many cell with zero elements narrows to empty.
            None => CellState::Empty,
        },
    };
    Ok(Outcome::Kept { state, lossy })
}

/// One scalar through one cast. `None` = checked cast rejected the
/// value. The bool is per-cell actual loss.
fn project_scalar(
    scalar: &Scalar,
    from: &ScalarType,
    to: &ScalarType,
    nomenclatures: &NomenclatureTable,
) -> Result<Option<(Scalar, bool)>, CastError> {
    use ScalarType as T;
    let ok = |s: Scalar| Ok(Some((s, false)));
    match (from, to) {
        (a, b) if a == b => ok(scalar.clone()),
        (T::Enum(_), T::Enum(r)) => {
            let Scalar::Enum(id) = scalar else {
                return Ok(None);
            };
            let rows = nomenclature_rows(r, nomenclatures)?;
            if rows.iter().any(|row| row.id == *id) {
                ok(scalar.clone())
            } else {
                // Option removed from the reader's nomenclature: exactly
                // the flagged case of §3's enum rows.
                Ok(None)
            }
        }
        // Number casts: §2.14 unit conversion on exact rationals,
        // composed with the representation rule — exact-or-nothing at
        // every step. (Equal types never reach here: the identity arm
        // above catches them.)
        (T::Integer(fu), T::Integer(tu)) => {
            let Scalar::Integer(i) = scalar else {
                return Ok(None);
            };
            Ok(convert_number(Decimal::from_i64(*i), *fu, *tu)
                .and_then(|d| d.to_i64())
                .map(|i| (Scalar::Integer(i), unit_dropped(*fu, *tu))))
        }
        (T::Integer(fu), T::Decimal(tu)) => {
            let Scalar::Integer(i) = scalar else {
                return Ok(None);
            };
            Ok(convert_number(Decimal::from_i64(*i), *fu, *tu)
                .map(|d| (Scalar::Decimal(d), unit_dropped(*fu, *tu))))
        }
        (T::Decimal(fu), T::Decimal(tu)) => {
            let Scalar::Decimal(d) = scalar else {
                return Ok(None);
            };
            Ok(convert_number(d.clone(), *fu, *tu)
                .map(|d| (Scalar::Decimal(d), unit_dropped(*fu, *tu))))
        }
        (T::Decimal(fu), T::Integer(tu)) => {
            let Scalar::Decimal(d) = scalar else {
                return Ok(None);
            };
            Ok(convert_number(d.clone(), *fu, *tu)
                .and_then(|d| d.to_i64())
                .map(|i| (Scalar::Integer(i), unit_dropped(*fu, *tu))))
        }
        (T::Date, T::Datetime) => {
            let Scalar::Date(d) = scalar else {
                return Ok(None);
            };
            ok(Scalar::Datetime(d.at_midnight_utc()))
        }
        (T::Datetime, T::Date) => {
            let Scalar::Datetime(t) = scalar else {
                return Ok(None);
            };
            let date = t.utc_date();
            // Actual loss only when a time-of-day existed.
            let lossless = date.at_midnight_utc() == *t;
            Ok(Some((Scalar::Date(date), !lossless)))
        }
        (T::Enum(w), T::Text) => {
            let Scalar::Enum(id) = scalar else {
                return Ok(None);
            };
            // The lens: the writer's nomenclature is what the id means.
            let rows = nomenclature_rows(w, nomenclatures)?;
            match rows.iter().find(|row| row.id == *id) {
                Some(row) => ok(Scalar::Text(row.label.clone())),
                None => Ok(None),
            }
        }
        // §2.15: constraint changes re-check the claims; a file the
        // narrowed constraints reject fails the cell, counted.
        (T::Attachment(_), T::Attachment(to)) => {
            let Scalar::Attachment(a) = scalar else {
                return Ok(None);
            };
            if to.accepts(&a.content_type) && to.admits_size(a.byte_size) {
                ok(scalar.clone())
            } else {
                Ok(None)
            }
        }
        (_, T::Text) => ok(Scalar::Text(render_text(scalar)?)),
        (T::Text, T::Integer(_)) => {
            text(scalar, |s| s.parse().ok().map(Scalar::Integer))
        }
        (T::Text, T::Decimal(_)) => {
            text(scalar, |s| Decimal::parse(s).ok().map(Scalar::Decimal))
        }
        (T::Text, T::Date) => text(scalar, |s| {
            varve_core::primitives::Date::parse(s).ok().map(Scalar::Date)
        }),
        (T::Text, T::Datetime) => text(scalar, |s| {
            varve_core::primitives::Instant::parse(s)
                .ok()
                .map(Scalar::Datetime)
        }),
        (T::Text, T::Boolean) => text(scalar, |s| match s {
            "true" => Some(Scalar::Boolean(true)),
            "false" => Some(Scalar::Boolean(false)),
            _ => None,
        }),
        (T::Text, T::Enum(r)) => {
            let Scalar::Text(s) = scalar else {
                return Ok(None);
            };
            let rows = nomenclature_rows(r, nomenclatures)?;
            Ok(rows
                .iter()
                .find(|row| row.label == *s)
                .map(|row| (Scalar::Enum(OptionId::new(row.id.as_str())), false)))
        }
        _ => Ok(None),
    }
}

/// §2.14: exact unit conversion. Same unit or a pure reinterpretation
/// (unit added/removed) passes the value through; a within-dimension
/// change converts on exact rationals or fails; a cross-dimension pair
/// never gets here (the cast table forbids the column).
fn convert_number(
    value: Decimal,
    from: Option<Unit>,
    to: Option<Unit>,
) -> Option<Decimal> {
    match (from, to) {
        (None, None) => Some(value),
        (None, Some(_)) | (Some(_), None) => Some(value),
        (Some(a), Some(b)) if a == b => Some(value),
        (Some(a), Some(b)) => {
            let (num, den) = conversion(a, b)?;
            value.mul_div_exact(num, den)
        }
    }
}

/// §2.14: removing a unit is lossy — the value's bytes survive, its
/// meaning does not — so every such cell counts in the lossiness report.
fn unit_dropped(from: Option<Unit>, to: Option<Unit>) -> bool {
    from.is_some() && to.is_none()
}

fn text(
    scalar: &Scalar,
    parse: impl Fn(&str) -> Option<Scalar>,
) -> Result<Option<(Scalar, bool)>, CastError> {
    let Scalar::Text(s) = scalar else {
        return Ok(None);
    };
    Ok(parse(s).map(|v| (v, false)))
}

/// Canonical text renderings for the widening →Text casts (§2.13
/// decision 3 forms).
fn render_text(scalar: &Scalar) -> Result<String, CastError> {
    Ok(match scalar {
        Scalar::Text(s) => s.clone(),
        Scalar::Boolean(b) => b.to_string(),
        Scalar::Integer(i) => i.to_string(),
        Scalar::Decimal(d) => d.to_string(),
        Scalar::Date(d) => d.to_string(),
        Scalar::Datetime(t) => t.to_string(),
        // Enum handled earlier (needs the lens); attachment/geometry
        // never reach here (cast table forbids them).
        Scalar::Enum(id) => id.to_string(),
        Scalar::Attachment(_) | Scalar::Geometry(_) => String::new(),
    })
}
