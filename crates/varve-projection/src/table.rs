//! The tabular row model (§5 "CSV and tabular views"): tabulation is
//! projection — a record viewed as rows through a revision. Format
//! sinks (CSV, XLSX, …) are Tier 5 (`varve-export`); this module owns
//! the semantics they all share, so the formats can only disagree
//! about presentation, never about content.
//!
//! Layering: inputs are plain descriptors handed down by the caller —
//! a `Surface` computes the visible ordered column set, an
//! `AggregateRevision` + report supply labels and header notes, but
//! none of those Tier 3 types enter this crate (the §7 impact-crate
//! pattern; "nothing depends on `varve-surface`" holds).

use std::collections::BTreeMap;

use varve_core::{ColumnId, GroupId, ItemId, OptionId, RecordId, RowPath};
use varve_schema::{Arity, ScalarType};
use varve_value::{CellAddr, CellState, CellValue, RecordValues, Scalar};

/// One column of the view, as plain data. The caller computes the set
/// and order from the surface (§5: exports are surface-scoped views)
/// and, for aggregates, folds `deprecated_since` / join policies into
/// `note`. Units render in the label — "poids (kg)" — never in cells
/// (§2.14): cells stay machine-parseable numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct TableColumn {
    pub column: ColumnId,
    /// Header row 1, cosmetic.
    pub label: String,
    pub ty: ScalarType,
    pub arity: Arity,
    /// Group chain, outermost first; empty = root scope.
    pub scope: Vec<GroupId>,
    /// Header flag, pre-rendered by the caller ("deprecated since …",
    /// "joined via text", §5.5) — Tier 3 report types stay out.
    pub note: Option<String>,
}

/// The ordered, visible column set plus the fixed leading columns.
#[derive(Debug, Clone, PartialEq)]
pub struct TableSchema {
    pub columns: Vec<TableColumn>,
}

/// §5 leading columns, in order.
pub const LEADING_COLUMNS: [&str; 4] = ["record_id", "scope", "item_id", "item_ordinal"];

impl TableSchema {
    /// Header row 1: labels (cosmetic), notes appended in brackets.
    pub fn header_labels(&self) -> Vec<String> {
        LEADING_COLUMNS
            .iter()
            .map(|s| s.to_string())
            .chain(self.columns.iter().map(|c| match &c.note {
                Some(note) => format!("{} [{note}]", c.label),
                None => c.label.clone(),
            }))
            .collect()
    }

    /// Header row 2: column ids (authoritative).
    pub fn header_ids(&self) -> Vec<String> {
        LEADING_COLUMNS
            .iter()
            .map(|s| s.to_string())
            .chain(self.columns.iter().map(|c| c.column.to_string()))
            .collect()
    }
}

/// A cell of the view, typed. `Blank` covers both absent and
/// written-empty: the Q13 provenance distinction is deliberately part
/// of what this lossy view loses (§5).
#[derive(Debug, Clone, PartialEq)]
pub enum TableValue {
    Blank,
    One(Scalar),
    Many(Vec<Scalar>),
}

/// One output row: the record's root row, or one row per item of a
/// `many` group (§5 — REDCap shape). `values` aligns with
/// `TableSchema::columns`; parent-scope values are repeated on item
/// rows for human usability.
#[derive(Debug, Clone, PartialEq)]
pub struct TableRow {
    pub record: RecordId,
    /// `None` on the root row; the item's group otherwise.
    pub scope: Option<GroupId>,
    pub item: Option<ItemId>,
    /// 1-based position within the group instance's item list.
    pub ordinal: Option<u64>,
    pub values: Vec<TableValue>,
}

/// Where the formats plug in (Tier 5 implements; §5). Deliberately
/// minimal: `begin` sees the schema, then one call per row.
pub trait TableSink {
    type Error;
    fn begin(&mut self, schema: &TableSchema) -> Result<(), Self::Error>;
    fn row(&mut self, row: &TableRow) -> Result<(), Self::Error>;
    fn finish(&mut self) -> Result<(), Self::Error>;
}

/// The cell path a column reads at, for a row at `path`: the prefix of
/// `path` whose group chain equals the column's scope. Root columns
/// (empty scope) read at root — which is what repeats parent values on
/// item rows. `None` = column not on this row's scope chain: blank.
fn scope_prefix(scope: &[GroupId], path: &RowPath) -> Option<RowPath> {
    let segs = path.segments();
    if scope.len() > segs.len() {
        return None;
    }
    if scope.iter().zip(segs).any(|(g, s)| *g != s.group) {
        return None;
    }
    let mut out = RowPath::root();
    for seg in &segs[..scope.len()] {
        out = out.child(seg.clone());
    }
    Some(out)
}

fn value_at(values: &RecordValues, column: &ColumnId, path: RowPath) -> TableValue {
    match values.cells.get(&CellAddr { column: column.clone(), path }) {
        None | Some(CellState::Empty) => TableValue::Blank,
        Some(CellState::Value(CellValue::One(s))) => TableValue::One(s.clone()),
        Some(CellState::Value(CellValue::Many(s))) => TableValue::Many(s.clone()),
    }
}

fn row_values(values: &RecordValues, schema: &TableSchema, path: &RowPath) -> Vec<TableValue> {
    schema
        .columns
        .iter()
        .map(|c| match scope_prefix(&c.scope, path) {
            Some(cell_path) => value_at(values, &c.column, cell_path),
            None => TableValue::Blank,
        })
        .collect()
}

/// One record → its rows: the root row first, then, for each group
/// with visible columns (in first-appearance order), one row per item
/// in stored order. Streaming-friendly: work is local to the record.
pub fn tabulate(record: &RecordId, values: &RecordValues, schema: &TableSchema) -> Vec<TableRow> {
    let mut rows = vec![TableRow {
        record: record.clone(),
        scope: None,
        item: None,
        ordinal: None,
        values: row_values(values, schema, &RowPath::root()),
    }];

    // Groups in first-appearance order of their innermost scope group.
    let mut groups: Vec<&GroupId> = Vec::new();
    for c in &schema.columns {
        if let Some(g) = c.scope.last()
            && !groups.contains(&g)
        {
            groups.push(g);
        }
    }

    for group in groups {
        for (addr, items) in &values.items {
            if addr.group != *group {
                continue;
            }
            for (idx, item) in items.iter().enumerate() {
                let path = addr.parent.child(varve_core::PathSeg {
                    group: group.clone(),
                    item: item.clone(),
                });
                rows.push(TableRow {
                    record: record.clone(),
                    scope: Some(group.clone()),
                    item: Some(item.clone()),
                    ordinal: Some(idx as u64 + 1),
                    values: row_values(values, schema, &path),
                });
            }
        }
    }
    rows
}

/// Per-column enum labels, resolved by the caller through the
/// *writer's* nomenclature lens (§2.11) and handed down as plain data.
pub type EnumLabels = BTreeMap<ColumnId, BTreeMap<OptionId, String>>;

/// Text-rendering knobs shared by text-shaped sinks. The list
/// separator packs `Many` cell values into one field — never one row
/// per element, which would conflate the two multiplicities (§2.2).
/// Default `" | "`: collides with neither `,` nor `;` field
/// delimiters and survives naive comma-splitting consumers (§5).
#[derive(Debug, Clone)]
pub struct TextStyle {
    pub list_separator: String,
    /// Render enum option ids instead of labels (opt-in; §5).
    pub enum_ids: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self { list_separator: " | ".into(), enum_ids: false }
    }
}

/// Canonical text rendering of one scalar (§5): what CSV writes and
/// what typed sinks fall back to. Enum labels come from the handed-down
/// map, falling back to the option id when the writer's revision no
/// longer carries the option. Attachments render as their filename;
/// geometry renders as its JCS text (its `Display`).
pub fn render_scalar(
    column: &ColumnId,
    scalar: &Scalar,
    labels: &EnumLabels,
    style: &TextStyle,
) -> String {
    match scalar {
        Scalar::Text(s) => s.clone(),
        Scalar::Boolean(b) => b.to_string(),
        Scalar::Integer(i) => i.to_string(),
        Scalar::Decimal(d) => d.to_string(),
        Scalar::Date(d) => d.to_string(),
        Scalar::Datetime(t) => t.to_string(),
        Scalar::Enum(option) => {
            if style.enum_ids {
                option.to_string()
            } else {
                labels
                    .get(column)
                    .and_then(|m| m.get(option))
                    .cloned()
                    .unwrap_or_else(|| option.to_string())
            }
        }
        Scalar::Attachment(a) => a.filename.clone(),
        Scalar::Geometry(f) => f.to_string(),
    }
}

pub fn render_value(
    column: &ColumnId,
    value: &TableValue,
    labels: &EnumLabels,
    style: &TextStyle,
) -> String {
    match value {
        TableValue::Blank => String::new(),
        TableValue::One(s) => render_scalar(column, s, labels, style),
        TableValue::Many(list) => list
            .iter()
            .map(|s| render_scalar(column, s, labels, style))
            .collect::<Vec<_>>()
            .join(&style.list_separator),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use varve_value::ItemsAddr;

    fn col(id: &str, label: &str, ty: ScalarType, scope: &[&str]) -> TableColumn {
        TableColumn {
            column: ColumnId::new(id),
            label: label.into(),
            ty,
            arity: Arity::One,
            scope: scope.iter().map(|g| GroupId::new(*g)).collect(),
            note: None,
        }
    }

    fn schema() -> TableSchema {
        TableSchema {
            columns: vec![
                col("name", "Nom", ScalarType::Text, &[]),
                col("tags", "Tags", ScalarType::Text, &[]),
                col("child_name", "Prénom enfant", ScalarType::Text, &["children"]),
            ],
        }
    }

    fn set(values: &mut RecordValues, column: &str, path: RowPath, v: CellValue) {
        values
            .cells
            .insert(CellAddr { column: ColumnId::new(column), path }, CellState::Value(v));
    }

    fn one(s: &str) -> CellValue {
        CellValue::One(Scalar::Text(s.into()))
    }

    fn record() -> RecordValues {
        let mut v = RecordValues::new();
        set(&mut v, "name", RowPath::root(), one("Ada"));
        set(
            &mut v,
            "tags",
            RowPath::root(),
            CellValue::Many(vec![Scalar::Text("a".into()), Scalar::Text("b, c".into())]),
        );
        let group = GroupId::new("children");
        v.items.insert(
            ItemsAddr { group: group.clone(), parent: RowPath::root() },
            vec![ItemId::new("i1"), ItemId::new("i2")],
        );
        for (item, name) in [("i1", "Alice"), ("i2", "Bob")] {
            let path = RowPath::root().child(varve_core::PathSeg {
                group: group.clone(),
                item: ItemId::new(item),
            });
            set(&mut v, "child_name", path, one(name));
        }
        v
    }

    #[test]
    fn root_row_plus_one_row_per_item() {
        let rows = tabulate(&RecordId::new("r1"), &record(), &schema());
        assert_eq!(rows.len(), 3);

        let root = &rows[0];
        assert_eq!(root.scope, None);
        assert_eq!(root.values[0], TableValue::One(Scalar::Text("Ada".into())));
        // Item-scoped column is blank on the root row.
        assert_eq!(root.values[2], TableValue::Blank);

        let first = &rows[1];
        assert_eq!(first.scope, Some(GroupId::new("children")));
        assert_eq!(first.item, Some(ItemId::new("i1")));
        assert_eq!(first.ordinal, Some(1));
        // Parent value repeated on the item row (§5).
        assert_eq!(first.values[0], TableValue::One(Scalar::Text("Ada".into())));
        assert_eq!(first.values[2], TableValue::One(Scalar::Text("Alice".into())));
        assert_eq!(rows[2].ordinal, Some(2));
        assert_eq!(rows[2].values[2], TableValue::One(Scalar::Text("Bob".into())));
    }

    #[test]
    fn empty_and_absent_both_render_blank() {
        let mut v = RecordValues::new();
        v.cells.insert(
            CellAddr { column: ColumnId::new("name"), path: RowPath::root() },
            CellState::Empty,
        );
        let rows = tabulate(&RecordId::new("r1"), &v, &schema());
        assert_eq!(rows.len(), 1); // no items → no item rows
        assert_eq!(rows[0].values[0], TableValue::Blank); // written-empty
        assert_eq!(rows[0].values[1], TableValue::Blank); // absent
        let style = TextStyle::default();
        let labels = EnumLabels::new();
        assert_eq!(render_value(&ColumnId::new("name"), &rows[0].values[0], &labels, &style), "");
    }

    #[test]
    fn headers_and_text_rendering() {
        let mut s = schema();
        s.columns[2].note = Some("deprecated since r5".into());
        assert_eq!(
            s.header_labels(),
            vec![
                "record_id",
                "scope",
                "item_id",
                "item_ordinal",
                "Nom",
                "Tags",
                "Prénom enfant [deprecated since r5]"
            ]
        );
        assert_eq!(s.header_ids()[4..], ["name", "tags", "child_name"]);

        let rows = tabulate(&RecordId::new("r1"), &record(), &s);
        let style = TextStyle::default();
        let labels = EnumLabels::new();
        // Many packs into one field; elements containing ", " stay
        // unambiguous under the pipe separator.
        assert_eq!(
            render_value(&ColumnId::new("tags"), &rows[0].values[1], &labels, &style),
            "a | b, c"
        );
    }

    #[test]
    fn enum_labels_with_fallback() {
        let column = ColumnId::new("civility");
        let mut labels = EnumLabels::new();
        labels
            .entry(column.clone())
            .or_default()
            .insert(OptionId::new("mme"), "Madame".into());
        let style = TextStyle::default();
        let known = TableValue::One(Scalar::Enum(OptionId::new("mme")));
        let unknown = TableValue::One(Scalar::Enum(OptionId::new("gone")));
        assert_eq!(render_value(&column, &known, &labels, &style), "Madame");
        assert_eq!(render_value(&column, &unknown, &labels, &style), "gone");
        let ids = TextStyle { enum_ids: true, ..TextStyle::default() };
        assert_eq!(render_value(&column, &known, &labels, &ids), "mme");
    }

    #[test]
    fn sink_drives_in_order() {
        struct Collect {
            begun: bool,
            rows: Vec<TableRow>,
            finished: bool,
        }
        impl TableSink for Collect {
            type Error = ();
            fn begin(&mut self, _schema: &TableSchema) -> Result<(), ()> {
                self.begun = true;
                Ok(())
            }
            fn row(&mut self, row: &TableRow) -> Result<(), ()> {
                self.rows.push(row.clone());
                Ok(())
            }
            fn finish(&mut self) -> Result<(), ()> {
                self.finished = true;
                Ok(())
            }
        }
        let s = schema();
        let mut sink = Collect { begun: false, rows: Vec::new(), finished: false };
        sink.begin(&s).unwrap();
        for row in tabulate(&RecordId::new("r1"), &record(), &s) {
            sink.row(&row).unwrap();
        }
        sink.finish().unwrap();
        assert!(sink.begun && sink.finished);
        assert_eq!(sink.rows.len(), 3);
    }
}
