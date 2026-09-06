//! `…/schema/preview`: the **fillable preview** of the revision
//! draft (G.12) — the authored tree rendered as the form it will
//! publish as, over the draft's scratch value bag
//! (`revisionDraft.preview`): filling it and pressing *Check the
//! form* posts every control as one `updatePreview` batch
//! ([`fill::submit`], post → 303 → get), and the landing page renders
//! the admissibility findings the server evaluated — required and
//! format, per compiled surface, deduplicated in presentation —
//! beside their controls. Filling never forks the draft; discard and
//! publication clear the bag. The editor and the preview stay two
//! tabs over one draft (`tabs_trigger` links); the full tree renders
//! — the administrator's view — with reviewer-only elements carrying
//! the editor's amber audience badge (platform P.4).
//!
//! Rendering by kind: a section is a heading (level by nesting
//! depth) with its help text, a group a `<fieldset>` with its label
//! as `<legend>` — a *many* group as one row-fieldset per item of
//! the bag's list, with *Remove row* / *Add a row* submit buttons
//! riding the same form — a note a dashed aside, and a column a
//! labelled control: text formats pick the input type (email →
//! `type=email`, phone → `type=tel`), numbers show their unit in the
//! caption, a choice is a select (`multiple` when many), an
//! attachment a file input with its `accept` list and geometry a
//! stated gap — both stay inert (G.12: no upload slots against a
//! draft, no map), though a required one still shows its finding.
//! The form is `novalidate`: the browser's own `required` gate would
//! hide exactly what the preview exists to show.

use std::collections::BTreeMap;

use cynic::{MutationBuilder, QueryBuilder};
use platform_client::preview::{
    AdmissibilityFinding, Cell, CellStateInput, CellWriteInput, ProcedurePreview,
    ProcedurePreviewQuery, ProcedurePreviewVariables, RowSegmentInput, UpdatePreview,
    UpdatePreviewInput, UpdatePreviewVariables,
};
use platform_client::revision_draft::{
    Cardinality, Column, ColumnType, Element, TextFormat, TextType,
};
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::RouterErrorExt, href, page, path_param},
    view::{BoxView, View, ViewExt, attributes, component, view},
};

use crate::{
    client,
    components::{
        badge::{BadgeVariant, badge},
        button::{ButtonSize, ButtonVariant, button},
        input::input,
        label::label,
        notice::{NoticeTone, notice as notice_box},
        select::select,
    },
    flash,
    i18n::{t, t_args},
    pages::one_arg,
};

use super::super::super::super::OrganizationId;
use super::element::{effectively_reviewer, id_of, parent_of, reviewer_badge, unit_name, unit_of};
use super::{NOTICE, Notice, NoticeKind, ProcedureId, header, procedure_draft, refused};

/// One row-path segment as the page carries it: `(group id, item id)`.
type Seg = (String, String);

/// A cell's address: column id plus its row path.
type CellKey = (String, Vec<Seg>);

/// The preview page.
#[page]
pub(super) async fn page(cx: &Cx) -> Result<impl View> {
    // Two reads: the draft for the shared header (its counts and
    // state line), the preview for the form — values, item lists,
    // findings.
    let procedure = procedure_draft(cx).await?;
    let preview = procedure_preview(cx).await?;
    let notice = flash::take::<Notice>(cx, NOTICE);
    Ok(view! { preview_page(procedure: procedure, preview: preview, notice: notice) })
}

/// The procedure with its preview through the client; 404 when the
/// schema answers `null`.
async fn procedure_preview(cx: &Cx) -> Result<ProcedurePreview> {
    let id = path_param::<ProcedureId>(cx)?;
    let client = client(cx).await?;
    Ok(platform_client::run(
        &client,
        ProcedurePreviewQuery::build(ProcedurePreviewVariables {
            id: cynic::Id::new(id.to_string()),
        }),
    )
    .await?
    .procedure
    .ok_or_not_found()?)
}

/// A prefill value, reduced to what a control needs.
enum CellView {
    /// Text, numbers, dates: the control's `value` string.
    Text(String),
    Boolean(bool),
    /// The selected option ids.
    Options(Vec<String>),
}

/// A control's findings, deduplicated across the surface pair (G.12:
/// the composition is presentation's).
#[derive(Default, Clone, Copy)]
struct Findings {
    required: bool,
    format: bool,
}

/// The tree, the value bag, and the localized texts the recursive
/// components need, bundled so they carry one reference.
struct Preview {
    elements: Vec<Element>,
    cells: BTreeMap<CellKey, CellView>,
    /// `(group, parent path) → ordered item ids`.
    items: Vec<(String, Vec<Seg>, Vec<String>)>,
    findings: BTreeMap<CellKey, Findings>,
    reviewer: String,
    many_rows: String,
    geometry: String,
    required_msg: String,
    format_msg: String,
    add_row: String,
}

impl Preview {
    fn new(procedure: &ProcedurePreview) -> Self {
        let bag = &procedure.revision_draft.preview;
        let mut cells = BTreeMap::new();
        for cell in &bag.cells {
            let Some((column, path)) = cell.address() else {
                continue;
            };
            let key = (
                column.inner().to_owned(),
                path.iter()
                    .map(|seg| {
                        (
                            seg.group_id.inner().to_owned(),
                            seg.item_id.inner().to_owned(),
                        )
                    })
                    .collect(),
            );
            let view = match cell {
                Cell::Text(c) => CellView::Text(c.value.clone()),
                Cell::Integer(c) => CellView::Text(c.value.clone()),
                Cell::Decimal(c) => CellView::Text(c.value.clone()),
                Cell::Date(c) => CellView::Text(c.value.clone()),
                Cell::Datetime(c) => CellView::Text(c.value.clone()),
                Cell::Boolean(c) => CellView::Boolean(c.value),
                Cell::Enum(c) => CellView::Options(
                    c.option_ids
                        .iter()
                        .map(|id| id.inner().to_owned())
                        .collect(),
                ),
                // Written-blank prefills nothing; unknown kinds are a
                // newer server's business.
                Cell::Empty(_) | Cell::Unknown => continue,
            };
            cells.insert(key, view);
        }
        let items = bag
            .items
            .iter()
            .map(|list| {
                (
                    list.group_id.inner().to_owned(),
                    list.parent
                        .iter()
                        .map(|seg| {
                            (
                                seg.group_id.inner().to_owned(),
                                seg.item_id.inner().to_owned(),
                            )
                        })
                        .collect(),
                    list.item_ids
                        .iter()
                        .map(|id| id.inner().to_owned())
                        .collect(),
                )
            })
            .collect();
        let mut findings: BTreeMap<CellKey, Findings> = BTreeMap::new();
        for finding in &bag.findings {
            let (column, path, kind) = match finding {
                AdmissibilityFinding::MissingRequired(f) => (&f.column_id, &f.path, true),
                AdmissibilityFinding::FormatViolation(f) => (&f.column_id, &f.path, false),
                AdmissibilityFinding::Unknown => continue,
            };
            let key = (
                column.inner().to_owned(),
                path.iter()
                    .map(|seg| {
                        (
                            seg.group_id.inner().to_owned(),
                            seg.item_id.inner().to_owned(),
                        )
                    })
                    .collect(),
            );
            let entry = findings.entry(key).or_default();
            if kind {
                entry.required = true;
            } else {
                entry.format = true;
            }
        }
        Self {
            elements: procedure.revision_draft.elements.clone(),
            cells,
            items,
            findings,
            reviewer: String::new(),
            many_rows: String::new(),
            geometry: String::new(),
            required_msg: String::new(),
            format_msg: String::new(),
            add_row: String::new(),
        }
    }

    fn children(&self, parent: Option<&str>) -> Vec<&Element> {
        self.elements
            .iter()
            .filter(|e| !matches!(e, Element::Unknown))
            .filter(|e| parent_of(e).as_deref() == parent)
            .collect()
    }

    /// Effectively reviewer-only (platform P.4 — inheritance).
    fn reviewer_only(&self, id: &str) -> bool {
        effectively_reviewer(&self.elements, id)
    }

    /// The bag's item list of one `many` group instance.
    fn rows(&self, group: &str, path: &[Seg]) -> Vec<String> {
        self.items
            .iter()
            .find(|(g, parent, _)| g == group && parent == path)
            .map(|(_, _, ids)| ids.clone())
            .unwrap_or_default()
    }

    /// How many findings the whole bag carries, deduplicated.
    fn finding_count(&self) -> usize {
        self.findings
            .values()
            .map(|f| usize::from(f.required) + usize::from(f.format))
            .sum()
    }
}

/// `cell:<column>[:<group>.<item>]*` — the control's form name; ids
/// are opaque hex, so the separators cannot collide (and neither
/// needs url-encoding in a form name, unlike `=`).
fn control_name(column: &str, path: &[Seg]) -> String {
    let mut name = format!("cell:{column}");
    for (group, item) in path {
        name.push_str(&format!(":{group}.{item}"));
    }
    name
}

/// `preview-<column>[-<item>]*` — the control's DOM id, unique per
/// row so label↔control pairing survives repetition.
fn control_id(column: &str, path: &[Seg]) -> String {
    let mut id = format!("preview-{column}");
    for (_, item) in path {
        id.push_str(&format!("-{item}"));
    }
    id
}

/// `[<group>.<item>:]*<group>` — an *Add a row* button's value: the
/// parent path, then the group the item joins.
fn add_value(group: &str, path: &[Seg]) -> String {
    let mut value = String::new();
    for (group, item) in path {
        value.push_str(&format!("{group}.{item}:"));
    }
    value.push_str(group);
    value
}

/// `[<group>.<item>:]*<group>.<item>` — a *Remove row* button's
/// value: the row's own full path; its last segment is the item.
fn remove_value(group: &str, item: &str, path: &[Seg]) -> String {
    format!("{}.{item}", add_value(group, path))
}

/// The page: the shared header with *Preview* active, the notice
/// slot, and the form over the bag.
#[component]
async fn preview_page(
    cx: &Cx,
    procedure: platform_client::revision_draft::ProcedureRevisionDraft,
    preview: ProcedurePreview,
    notice: Option<Notice>,
) -> Result<impl View> {
    let organization_id: uuid::Uuid = procedure.organization.id.inner().parse()?;
    let procedure_id: uuid::Uuid = procedure.id.inner().parse()?;
    let mut tree = Preview::new(&preview);
    tree.reviewer = t(cx, "schema.audience.reviewer").await?;
    tree.many_rows = t(cx, "schema.cardinality.many").await?;
    tree.geometry = t(cx, "schema.preview.geometry").await?;
    tree.required_msg = t(cx, "schema.preview.finding.required").await?;
    tree.format_msg = t(cx, "schema.preview.finding.format").await?;
    tree.add_row = t(cx, "schema.preview.add_row").await?;
    let is_empty = tree.elements.is_empty();
    let panel_heading = t(cx, "schema.tab.preview").await?;
    let empty = t(cx, "schema.preview.empty").await?;
    let check = t(cx, "schema.preview.check").await?;
    let summary = t_args(
        cx,
        "schema.preview.findings",
        &one_arg("n", tree.finding_count() as i64),
    )
    .await?;
    let fill_href = href!(
        fill::submit,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    )
    .resolve(cx);
    Ok(view! {
        // Nothing on this tab autosaves, so the header's draft-state
        // shard never re-fetches; the counter exists for the shared
        // header alone.
        signal revision = 0.0;

        <div class="flex flex-col gap-6">
            header::header(
                procedure: procedure.clone(),
                tab: header::Tab::Preview,
                offer_discard: false,
                offer_publish: false,
                revision: revision
            )
            <h2 class="sr-only">(panel_heading)</h2>
            // The notice slot is always there, at one height, so the
            // form never moves when a refusal comes or goes.
            <div class="min-h-12" aria-live="polite" data-preview-notices="">
                if let Some(notice) = &notice {
                    notice_box(
                        tone: match notice.kind {
                            NoticeKind::Status => NoticeTone::Success,
                            NoticeKind::Alert => NoticeTone::Error,
                        },
                        attrs: attributes! {
                            role=(match notice.kind {
                                NoticeKind::Status => "status",
                                NoticeKind::Alert => "alert",
                            })
                            data-preview-notice=""
                        },
                        (notice.text.as_str())
                    )
                }
            </div>
            if is_empty {
                <p class="text-sm text-muted-foreground" data-preview-empty="">
                    (empty)
                </p>
            } else {
                // `novalidate`: the browser's own `required` gate
                // would block exactly the submission whose findings
                // the preview exists to show.
                <form
                    method="post"
                    action=(fill_href.as_str())
                    novalidate=""
                    class="flex max-w-2xl flex-col gap-4"
                >
                    <p class="text-sm text-muted-foreground" data-preview-summary="">
                        (summary.as_str())
                    </p>
                    preview_list(
                        tree: &tree,
                        parent: None,
                        path: Vec::new(),
                        heading: 3
                    )
                    <div>
                        button(
                            variant: ButtonVariant::Primary,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" data-preview-check="" },
                            (check.as_str())
                        )
                    </div>
                </form>
            }
        </div>
    })
}

/// One level of the tree, document order, inside one row scope.
/// Boxed, like the editor's `tree_list`, to keep each recursion
/// level's render frame small.
#[component]
async fn preview_list(
    tree: &Preview,
    parent: Option<String>,
    path: Vec<Seg>,
    heading: usize,
) -> Result<impl View> {
    let children: Vec<Element> = tree
        .children(parent.as_deref())
        .into_iter()
        .cloned()
        .collect();
    Ok(view! {
        for element in children {
            preview_element(
                tree: tree,
                element: element,
                path: path.clone(),
                heading: heading
            )
        }
    }
    .boxed())
}

/// One element: a column as a labelled control over the bag, a group
/// as a `<fieldset>` — a *many* group as its rows —, a section as a
/// heading with its help, a note as a dashed aside. Boxed, like
/// [`preview_list`].
#[component]
async fn preview_element(
    tree: &Preview,
    element: Element,
    path: Vec<Seg>,
    heading: usize,
) -> Result<impl View> {
    let id = id_of(&element).to_owned();
    let reviewer_only = tree.reviewer_only(&id);
    Ok(view! {
        match element {
            Element::Column(column) => {
                <div class="flex flex-col gap-2" data-preview-element=(id.as_str())>
                    preview_column(
                        tree: tree,
                        column: column,
                        path: path.clone(),
                        reviewer_only: reviewer_only
                    )
                </div>
            }
            Element::Group(group) => {
                <fieldset
                    class="flex flex-col gap-4 rounded-lg border border-border p-4"
                    data-preview-element=(id.as_str())
                >
                    <legend class="flex items-center gap-2 px-1 text-sm font-medium">
                        (group.label.as_str())
                        if matches!(group.cardinality, Cardinality::Many) {
                            badge(
                                variant: BadgeVariant::Secondary,
                                (tree.many_rows.as_str())
                            )
                        }
                        if reviewer_only {
                            reviewer_badge(text: tree.reviewer.clone())
                        }
                    </legend>
                    if matches!(group.cardinality, Cardinality::Many) {
                        preview_rows(
                            tree: tree,
                            group: group.clone(),
                            path: path.clone(),
                            heading: heading
                        )
                    } else {
                        preview_list(
                            tree: tree,
                            parent: Some(id.clone()),
                            path: path.clone(),
                            heading: heading
                        )
                    }
                </fieldset>
            }
            Element::Section(section) => {
                <section class="flex flex-col gap-4" data-preview-element=(id.as_str())>
                    <div class="flex flex-col gap-1">
                        <div class="flex items-center gap-2">
                            preview_heading(level: heading, text: section.title.clone())
                            if reviewer_only {
                                reviewer_badge(text: tree.reviewer.clone())
                            }
                        </div>
                        if let Some(help) = &section.help {
                            if !help.is_empty() {
                                <p class="text-sm text-muted-foreground">
                                    (help.as_str())
                                </p>
                            }
                        }
                    </div>
                    preview_list(
                        tree: tree,
                        parent: Some(id.clone()),
                        path: path.clone(),
                        heading: (heading + 1).min(6)
                    )
                </section>
            }
            Element::Note(note) => {
                <div
                    class="flex flex-col gap-1 rounded-lg border border-dashed border-border px-4 py-3"
                    data-preview-element=(id.as_str())
                >
                    if note.title.is_some() || reviewer_only {
                        <div class="flex items-center gap-2">
                            if let Some(title) = &note.title {
                                <p class="text-sm font-medium">(title.as_str())</p>
                            }
                            if reviewer_only {
                                reviewer_badge(text: tree.reviewer.clone())
                            }
                        </div>
                    }
                    <p class="text-sm text-muted-foreground">(note.body.as_str())</p>
                </div>
            }
            Element::Unknown => {
                ""
            }
        }
    }
    .boxed())
}

/// A `many` group's rows: one inner `<fieldset>` per item of the
/// bag's list — its children rendered one path segment deeper, with
/// a *Remove row* button — then *Add a row*. Both are submit buttons
/// on the page's one form, so typed values elsewhere ride along and
/// survive the round-trip.
#[component]
async fn preview_rows(
    tree: &Preview,
    group: platform_client::revision_draft::Group,
    path: Vec<Seg>,
    heading: usize,
) -> Result<impl View> {
    let group_id = group.id.inner().to_owned();
    let rows = tree.rows(&group_id, &path);
    let add = add_value(&group_id, &path);
    let numbered: Vec<(usize, String)> = rows
        .into_iter()
        .enumerate()
        .map(|(i, item)| (i + 1, item))
        .collect();
    let rows: Vec<(usize, Vec<Seg>)> = numbered
        .into_iter()
        .map(|(n, item)| {
            let mut row_path = path.clone();
            row_path.push((group_id.clone(), item));
            (n, row_path)
        })
        .collect();
    Ok(view! {
        for (n, row_path) in rows {
            preview_row(tree: tree, n: n, row_path: row_path, heading: heading)
        }
        <div>
            button(
                variant: ButtonVariant::Outline,
                size: ButtonSize::Sm,
                attrs: attributes! {
                    type="submit"
                    name="add"
                    value=(add.as_str())
                    data-preview-add=(group_id.as_str())
                },
                (tree.add_row.as_str())
            )
        </div>
    }
    .boxed())
}

/// One row of a `many` group: a numbered `<fieldset>` whose children
/// carry the row's path segment (`row_path` ends with it), and its
/// *Remove row* button.
#[component]
async fn preview_row(
    cx: &Cx,
    tree: &Preview,
    n: usize,
    row_path: Vec<Seg>,
    heading: usize,
) -> Result<impl View> {
    let (group_id, item) = row_path
        .last()
        .expect("a row path ends with its row")
        .clone();
    let row_label = t_args(cx, "schema.preview.row", &one_arg("n", n as i64)).await?;
    let remove_label = t_args(cx, "schema.preview.remove_row", &one_arg("n", n as i64)).await?;
    let remove = remove_value(&group_id, &item, &row_path[..row_path.len() - 1]);
    Ok(view! {
        <fieldset
            class="flex flex-col gap-4 rounded-md border border-border/70 p-3"
            data-preview-row=(item.as_str())
        >
            <legend class="px-1 text-xs font-medium text-muted-foreground">
                (row_label.as_str())
            </legend>
            preview_list(
                tree: tree,
                parent: Some(group_id.clone()),
                path: row_path.clone(),
                heading: heading
            )
            <div>
                button(
                    variant: ButtonVariant::Outline,
                    size: ButtonSize::Sm,
                    attrs: attributes! {
                        type="submit"
                        name="remove"
                        value=(remove.as_str())
                        data-preview-remove=(item.as_str())
                    },
                    (remove_label.as_str())
                )
            </div>
        </fieldset>
    }
    .boxed())
}

/// A section's heading at its nesting depth: the page `<h1>` and
/// the panel's hidden `<h2>` are above, so top-level sections are
/// `<h3>`, nested ones one deeper, capped at `<h6>` (the kernel
/// allows deeper trees than HTML has levels).
#[component]
async fn preview_heading(level: usize, text: String) -> Result<impl View> {
    Ok(view! {
        match level {
            3 => {
                <h3 class="text-base font-semibold">(text.as_str())</h3>
            }
            4 => {
                <h4 class="text-base font-semibold">(text.as_str())</h4>
            }
            5 => {
                <h5 class="text-sm font-semibold">(text.as_str())</h5>
            }
            _ => {
                <h6 class="text-sm font-semibold">(text.as_str())</h6>
            }
        }
    })
}

/// An RFC 3339 instant, cut to the `datetime-local` value form
/// (`YYYY-MM-DDTHH:MM`).
fn datetime_local(value: &str) -> String {
    value.get(..16).unwrap_or(value).to_owned()
}

/// One column at one row path: the caption row (label — with the
/// unit for numbers — and the audience badge), the control its type
/// asks for prefilled from the bag, and its findings as text the
/// control references (`aria-describedby` + `aria-invalid`).
/// Geometry has no control, so its caption is a plain paragraph,
/// never a dangling `<label for>`.
#[component]
async fn preview_column(
    tree: &Preview,
    column: Column,
    path: Vec<Seg>,
    reviewer_only: bool,
) -> Result<impl View> {
    let column_id = column.id.inner().to_owned();
    let key: CellKey = (column_id.clone(), path.clone());
    let control = control_id(&column_id, &path);
    let name = control_name(&column_id, &path);
    let caption = match unit_of(&column.ty).map(unit_name) {
        Some(unit) => format!("{} ({unit})", column.label),
        None => column.label.clone(),
    };
    let required = column.required.then_some("");
    let accept = match &column.ty {
        ColumnType::Attachment(a) if !a.accept.is_empty() => Some(a.accept.join(",")),
        _ => None,
    };
    let has_control = !matches!(column.ty, ColumnType::Geometry(_));
    let text_value = match tree.cells.get(&key) {
        Some(CellView::Text(value)) => Some(value.clone()),
        _ => None,
    };
    let checked = matches!(tree.cells.get(&key), Some(CellView::Boolean(true)));
    let selected: Vec<String> = match tree.cells.get(&key) {
        Some(CellView::Options(ids)) => ids.clone(),
        _ => Vec::new(),
    };
    let findings = tree.findings.get(&key).copied().unwrap_or_default();
    let has_findings = findings.required || findings.format;
    let finding_id = format!("{control}-finding");
    let describedby = has_findings.then_some(finding_id.clone());
    let invalid = has_findings.then_some("true");
    let datetime_value = text_value.as_deref().map(datetime_local);
    Ok(view! {
        <div class="flex items-center gap-2">
            if has_control {
                label(attrs: attributes! { for=(control.as_str()) }, (caption.as_str()))
            } else {
                <p class="text-sm leading-none font-medium">(caption.as_str())</p>
            }
            if reviewer_only {
                reviewer_badge(text: tree.reviewer.clone())
            }
        </div>
        match &column.ty {
            ColumnType::Boolean(_) => {
                <input
                    id=(control.as_str())
                    name=(name.as_str())
                    type="checkbox"
                    checked=(checked.then_some(""))
                    required=(required)
                    aria-describedby=(describedby.as_deref())
                    aria-invalid=(invalid)
                    class="size-4 shrink-0 rounded border-border accent-primary"
                >
            }
            ColumnType::Integer(_) => {
                input(
                    attrs: attributes! {
                        id=(control.as_str())
                        name=(name.as_str())
                        type="number"
                        step="1"
                        value=(text_value.as_deref())
                        required=(required)
                        aria-describedby=(describedby.as_deref())
                        aria-invalid=(invalid)
                    }
                )
            }
            ColumnType::Decimal(_) => {
                input(
                    attrs: attributes! {
                        id=(control.as_str())
                        name=(name.as_str())
                        type="number"
                        step="any"
                        value=(text_value.as_deref())
                        required=(required)
                        aria-describedby=(describedby.as_deref())
                        aria-invalid=(invalid)
                    }
                )
            }
            ColumnType::Date(_) => {
                input(
                    attrs: attributes! {
                        id=(control.as_str())
                        name=(name.as_str())
                        type="date"
                        value=(text_value.as_deref())
                        required=(required)
                        aria-describedby=(describedby.as_deref())
                        aria-invalid=(invalid)
                    }
                )
            }
            ColumnType::Datetime(_) => {
                input(
                    attrs: attributes! {
                        id=(control.as_str())
                        name=(name.as_str())
                        type="datetime-local"
                        value=(datetime_value.as_deref())
                        required=(required)
                        aria-describedby=(describedby.as_deref())
                        aria-invalid=(invalid)
                    }
                )
            }
            ColumnType::Enum(choice) => {
                select(
                    attrs: attributes! {
                        id=(control.as_str())
                        name=(name.as_str())
                        multiple=(choice.multiple.then_some(""))
                        required=(required)
                        aria-describedby=(describedby.as_deref())
                        aria-invalid=(invalid)
                    },
                    if !choice.multiple {
                        <option value=""></option>
                    }
                    for option in &choice.options {
                        <option
                            value=(option.id.inner())
                            selected=(selected
                                .iter()
                                .any(|id| id == option.id.inner())
                                .then_some(""))
                        >
                            (option.label.as_str())
                        </option>
                    }
                )
            }
            ColumnType::Attachment(attachment) => {
                // Inert (G.12): no name, nothing submits — but the
                // finding below still tells the truth about it.
                input(
                    attrs: attributes! {
                        id=(control.as_str())
                        type="file"
                        accept=(accept.as_deref())
                        multiple=(attachment.multiple.then_some(""))
                        required=(required)
                        aria-describedby=(describedby.as_deref())
                        aria-invalid=(invalid)
                    }
                )
            }
            ColumnType::Geometry(_) => {
                <p class="text-sm text-muted-foreground" data-preview-geometry="">
                    (tree.geometry.as_str())
                </p>
            }
            _ => {
                input(
                    attrs: attributes! {
                        id=(control.as_str())
                        name=(name.as_str())
                        type=(match &column.ty {
                            ColumnType::Text(text) => text_input_type(text),
                            _ => "text",
                        })
                        value=(text_value.as_deref())
                        required=(required)
                        aria-describedby=(describedby.as_deref())
                        aria-invalid=(invalid)
                    }
                )
            }
        }
        if has_findings {
            <p
                id=(finding_id.as_str())
                class="text-sm text-destructive"
                data-preview-finding=(column_id.as_str())
            >
                if findings.required {
                    (tree.required_msg.as_str())
                }
                if findings.required && findings.format {
                    " "
                }
                if findings.format {
                    (tree.format_msg.as_str())
                }
            </p>
        }
    }
    .boxed())
}

/// A text column's input type from its format constraint: email and
/// phone have native input types; IBAN and custom patterns stay
/// plain text.
fn text_input_type(text: &TextType) -> &'static str {
    match &text.format {
        Some(TextFormat::Email(_)) => "email",
        Some(TextFormat::Phone(_)) => "tel",
        _ => "text",
    }
}

/// Every fillable control the tree currently offers: each column
/// (any kind — the submit side skips the inert ones) at each row
/// path the bag's item lists open up. The submit handler walks this
/// to know which absent fields mean *unset*.
fn walk_columns(
    tree: &Preview,
    parent: Option<&str>,
    path: &[Seg],
    out: &mut Vec<(Column, Vec<Seg>)>,
) {
    for element in tree.children(parent) {
        match element {
            Element::Column(column) => out.push((column.clone(), path.to_vec())),
            Element::Group(group) if matches!(group.cardinality, Cardinality::Many) => {
                let group_id = group.id.inner().to_owned();
                for item in tree.rows(&group_id, path) {
                    let mut row_path = path.to_vec();
                    row_path.push((group_id.clone(), item));
                    walk_columns(tree, Some(group.id.inner()), &row_path, out);
                }
            }
            Element::Group(group) => {
                walk_columns(tree, Some(group.id.inner()), path, out);
            }
            Element::Section(section) => {
                walk_columns(tree, Some(section.id.inner()), path, out);
            }
            Element::Note(_) | Element::Unknown => {}
        }
    }
}

/// `…/schema/preview/fill`: [`fill::submit`].
pub(in crate::pages) mod fill {
    use super::*;

    /// `[<group>.<item>:]*<tail>` — the path segments, and the tail
    /// an action value ends with (`<group>` for add,
    /// `<group>.<item>` for remove).
    fn parse_segments(value: &str) -> Option<(Vec<Seg>, &str)> {
        let mut parts: Vec<&str> = value.split(':').collect();
        let tail = parts.pop()?;
        let mut path = Vec::new();
        for part in parts {
            let (group, item) = part.split_once('.')?;
            if group.is_empty() || item.is_empty() {
                return None;
            }
            path.push((group.to_owned(), item.to_owned()));
        }
        Some((path, tail))
    }

    fn segments_input(path: &[Seg]) -> Vec<RowSegmentInput> {
        path.iter()
            .map(|(group, item)| RowSegmentInput {
                group_id: cynic::Id::new(group),
                item_id: cynic::Id::new(item),
            })
            .collect()
    }

    /// An *Add a row* value → the `addItem` write; `None` for a
    /// value this form never produced.
    fn parse_add(value: &str) -> Option<CellWriteInput> {
        let (path, group) = parse_segments(value)?;
        (!group.is_empty() && !group.contains('.'))
            .then(|| CellWriteInput::add_item(cynic::Id::new(group), segments_input(&path), None))
    }

    /// A *Remove row* value → the `removeItem` write; `None` for a
    /// value this form never produced.
    fn parse_remove(value: &str) -> Option<CellWriteInput> {
        let (path, tail) = parse_segments(value)?;
        let (group, item) = tail.split_once('.')?;
        (!group.is_empty() && !item.is_empty()).then(|| {
            CellWriteInput::remove_item(
                cynic::Id::new(group),
                segments_input(&path),
                cynic::Id::new(item),
            )
        })
    }

    /// A submitted control's values → the cell state to set; `None`
    /// means the field is blank, which the caller maps to *unset*
    /// (a plain form cannot say written-blank — absence is its one
    /// empty).
    fn state_of(ty: &ColumnType, values: &[&str]) -> Option<CellStateInput> {
        match ty {
            // A checkbox submits only when checked.
            ColumnType::Boolean(_) => Some(CellStateInput::boolean(true)),
            ColumnType::Integer(_) => Some(CellStateInput::integer(*values.first()?)),
            ColumnType::Decimal(_) => Some(CellStateInput::decimal(*values.first()?)),
            ColumnType::Date(_) => Some(CellStateInput::date(*values.first()?)),
            ColumnType::Datetime(_) => {
                let value = *values.first()?;
                // `datetime-local` has no zone; the preview reads it
                // as UTC, the page's display convention.
                let instant = if value.len() == 16 {
                    format!("{value}:00Z")
                } else if value.ends_with('Z') {
                    value.to_owned()
                } else {
                    format!("{value}Z")
                };
                Some(CellStateInput::datetime(instant))
            }
            ColumnType::Enum(choice) if choice.multiple => Some(CellStateInput::enum_options(
                values.iter().map(|value| cynic::Id::new(*value)).collect(),
            )),
            ColumnType::Enum(_) => Some(CellStateInput::enum_option(cynic::Id::new(
                *values.first()?,
            ))),
            ColumnType::Text(_) => Some(CellStateInput::text(*values.first()?)),
            // Inert in the preview (G.12); never submitted.
            ColumnType::Attachment(_) | ColumnType::Geometry(_) | ColumnType::Unknown => None,
        }
    }

    /// The whole form as one `updatePreview` batch: every offered
    /// control sets or unsets its cell, and the clicked row action —
    /// if any — rides the same batch, so typed values survive adding
    /// or removing a row. A refused batch lands back as the alert
    /// notice; success needs none — the findings are the answer.
    #[page(POST)]
    pub(in crate::pages) async fn submit(
        cx: &Cx,
        Form(pairs): Form<Vec<(String, String)>>,
    ) -> Result<impl View> {
        let client = client(cx).await?;
        let procedure = procedure_preview(cx).await?;
        let tree = Preview::new(&procedure);

        let mut fields: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut add = None;
        let mut remove = None;
        for (name, value) in &pairs {
            match name.as_str() {
                "add" => add = parse_add(value),
                "remove" => remove = parse_remove(value),
                _ => fields.entry(name).or_default().push(value),
            }
        }

        let mut columns = Vec::new();
        walk_columns(&tree, None, &[], &mut columns);
        let mut writes = Vec::new();
        for (column, path) in columns {
            let column_id = column.id.inner().to_owned();
            let name = control_name(&column_id, &path);
            let values: Vec<&str> = fields
                .get(name.as_str())
                .map(|values| {
                    values
                        .iter()
                        .copied()
                        .filter(|value| !value.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let present = !values.is_empty();
            match state_of(&column.ty, &values) {
                Some(state) if present => writes.push(CellWriteInput::set(
                    cynic::Id::new(&column_id),
                    segments_input(&path),
                    state,
                )),
                _ => {
                    // Blank means back to absent — but only when a
                    // cell is there to clear.
                    let key: CellKey = (column_id.clone(), path.clone());
                    if tree.cells.contains_key(&key) {
                        writes.push(CellWriteInput::unset(
                            cynic::Id::new(&column_id),
                            segments_input(&path),
                        ));
                    }
                }
            }
        }
        writes.extend(add);
        writes.extend(remove);

        let notice = if writes.is_empty() {
            None
        } else {
            let outcome = platform_client::run(
                &client,
                UpdatePreview::build(UpdatePreviewVariables {
                    input: UpdatePreviewInput {
                        procedure_id: procedure.id.clone(),
                        writes,
                    },
                }),
            )
            .await;
            match outcome {
                Ok(_) => None,
                Err(error) => Some(refused(cx, error).await?),
            }
        };
        back_to_preview(cx, notice).await
    }
}

/// Where the fill POST lands: the preview, with a notice when the
/// batch was refused.
async fn back_to_preview(cx: &Cx, notice: Option<Notice>) -> Result<BoxView<'static>> {
    let organization = path_param::<OrganizationId>(cx)?;
    let procedure = path_param::<ProcedureId>(cx)?;
    if let Some(notice) = notice {
        flash::set(cx, NOTICE, notice)?;
    }
    let location = href!(page, OrganizationId(*organization), ProcedureId(*procedure)).resolve(cx);
    crate::pages::redirect_to(cx, location)
}
