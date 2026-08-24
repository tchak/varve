//! `…/schema/preview`: the **read-only preview** of the revision
//! draft — the authored tree rendered as the form it will publish
//! as, with real, inert controls: no `<form>` element, no submit,
//! nothing persisted (the fillable preview, over draft record
//! values, is a later slice). The editor and the preview are two
//! tabs over one draft (`tabs_trigger` links, the settings-shell
//! pattern); the full tree renders — the administrator's view —
//! with reviewer-only elements carrying the editor's amber
//! audience badge (platform P.4).
//!
//! Rendering by kind: a section is a heading (level by nesting
//! depth) with its help text, a group a `<fieldset>` with its
//! label as `<legend>`, a note a dashed aside, and a column a
//! labelled control — text formats pick the input type (email →
//! `type=email`, phone → `type=tel`), numbers show their unit in
//! the caption, a choice is a select (`multiple` when many), an
//! attachment a file input with its `accept` list, and geometry a
//! stated gap (no map in the preview). `required` rides as the
//! attribute; with no form it never fires.

use platform_client::revision_draft::{
    Cardinality, Column, ColumnType, Element, ProcedureRevisionDraft, TextFormat, TextType,
};
use topcoat::{
    Result,
    context::Cx,
    icon::{icon, iconify::iconify_icon},
    router::{href, page},
    view::{attributes, component, view},
};

use crate::{
    components::{
        badge::{BadgeVariant, badge},
        input::input,
        label::label,
        page_title::page_title,
        select::select,
        tabs::{tabs, tabs_list, tabs_trigger},
    },
    i18n::{t, t_args},
    pages::{args, one_arg, utc_date_arg},
};

use super::element::{effectively_reviewer, id_of, parent_of, unit_name, unit_of};
use super::{OrganizationId, ProcedureId, counts, procedure_draft};

/// The preview page.
#[page]
pub(super) async fn page(cx: &Cx) -> Result {
    let procedure = procedure_draft(cx).await?;
    view! { preview_page(procedure: procedure) }
}

/// The tree and the localized texts its rendering needs, bundled so
/// the recursive components carry one reference.
struct Preview {
    elements: Vec<Element>,
    reviewer: String,
    many_rows: String,
    geometry: String,
}

impl Preview {
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
}

/// The page: the editor's header (title, back link, draft state),
/// the tab rail with *Preview* active, and the rendered form.
#[component]
async fn preview_page(cx: &Cx, procedure: ProcedureRevisionDraft) -> Result {
    let organization_id: uuid::Uuid = procedure.organization.id.inner().parse()?;
    let procedure_id: uuid::Uuid = procedure.id.inner().parse()?;
    let title = t_args(
        cx,
        "schema.title",
        &one_arg("procedure", procedure.title.clone()),
    )
    .await?;
    let back = t(cx, "schema.back").await?;
    let draft_badge = t(cx, "procedure.draft.badge").await?;
    let tab_editor = t(cx, "schema.tab.editor").await?;
    let tab_preview = t(cx, "schema.tab.preview").await?;
    let panel_heading = tab_preview.clone();
    let empty = t(cx, "schema.preview.empty").await?;
    let elements = procedure
        .revision_draft
        .as_ref()
        .map(|d| d.elements.clone())
        .unwrap_or_default();
    let (columns, groups) = counts(&elements);
    let state = match &procedure.revision_draft {
        Some(_) => {
            t_args(
                cx,
                "schema.state.draft",
                &args([
                    ("columns", (columns as i64).into()),
                    ("groups", (groups as i64).into()),
                    ("date", utc_date_arg(procedure.updated_at)),
                ]),
            )
            .await?
        }
        None => t(cx, "schema.state.none").await?,
    };
    let tree = Preview {
        elements,
        reviewer: t(cx, "schema.audience.reviewer").await?,
        many_rows: t(cx, "schema.cardinality.many").await?,
        geometry: t(cx, "schema.preview.geometry").await?,
    };
    let is_empty = tree.elements.is_empty();
    let editor_href = href!(
        super::page,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    );
    let preview_href = href!(
        page,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    );
    let procedure_href = href!(
        super::super::page,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    );
    view! {
        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2">
                page_title((title))
                <p class="text-sm text-muted-foreground">
                    <a
                        href=(procedure_href)
                        class="font-medium text-foreground underline-offset-4 hover:underline"
                    >
                        (back)
                    </a>
                </p>
                <div
                    class="flex flex-wrap items-center gap-3 text-sm text-muted-foreground"
                >
                    if procedure.revision_draft.is_some() {
                        badge(variant: BadgeVariant::Secondary, (draft_badge))
                    }
                    <span data-schema-state="">(state)</span>
                </div>
            </div>
            tabs(
                tabs_list(
                    tabs_trigger(
                        attrs: attributes! { href=(editor_href) },
                        (tab_editor)
                    )
                    tabs_trigger(
                        active: true,
                        attrs: attributes! { href=(preview_href) },
                        (tab_preview)
                    )
                )
            )
            <h2 class="sr-only">(panel_heading)</h2>
            if is_empty {
                <p class="text-sm text-muted-foreground" data-preview-empty="">
                    (empty)
                </p>
            } else {
                <div class="flex max-w-2xl flex-col gap-4">
                    preview_list(tree: &tree, parent: None, heading: 3)
                </div>
            }
        </div>
    }
}

/// One level of the tree, document order. Boxed, like the editor's
/// `tree_list`, to keep each recursion level's render frame small.
#[component(boxed)]
async fn preview_list(tree: &Preview, parent: Option<String>, heading: usize) -> Result {
    let children: Vec<Element> = tree
        .children(parent.as_deref())
        .into_iter()
        .cloned()
        .collect();
    view! {
        for element in children {
            preview_element(tree: tree, element: element, heading: heading)
        }
    }
}

/// One element: a column as a labelled control, a group as a
/// `<fieldset>`, a section as a heading with its help, a note as a
/// dashed aside. Boxed, like [`preview_list`].
#[component(boxed)]
async fn preview_element(tree: &Preview, element: Element, heading: usize) -> Result {
    let id = id_of(&element).to_owned();
    let reviewer_only = tree.reviewer_only(&id);
    view! {
        match element {
            Element::Column(column) => {
                <div class="flex flex-col gap-2" data-preview-element=(id.as_str())>
                    preview_column(
                        tree: tree,
                        column: column,
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
                    preview_list(tree: tree, parent: Some(id.clone()), heading: heading)
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
}

/// A section's heading at its nesting depth: the page `<h1>` and
/// the panel's hidden `<h2>` are above, so top-level sections are
/// `<h3>`, nested ones one deeper, capped at `<h6>` (the kernel
/// allows deeper trees than HTML has levels).
#[component]
async fn preview_heading(level: usize, text: String) -> Result {
    view! {
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
    }
}

/// One column: the caption row (label — with the unit for numbers —
/// and the audience badge), then the control its type asks for.
/// Geometry has no control, so its caption is a plain paragraph,
/// never a dangling `<label for>`.
#[component]
async fn preview_column(tree: &Preview, column: Column, reviewer_only: bool) -> Result {
    let control = format!("preview-{}", column.id.inner());
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
    view! {
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
                    type="checkbox"
                    required=(required)
                    class="size-4 shrink-0 rounded border-border accent-primary"
                >
            }
            ColumnType::Integer(_) => {
                input(
                    attrs: attributes! {
                        id=(control.as_str())
                        type="number"
                        step="1"
                        required=(required)
                    }
                )
            }
            ColumnType::Decimal(_) => {
                input(
                    attrs: attributes! {
                        id=(control.as_str())
                        type="number"
                        step="any"
                        required=(required)
                    }
                )
            }
            ColumnType::Date(_) => {
                input(
                    attrs: attributes! { id=(control.as_str()) type="date" required=(required) }
                )
            }
            ColumnType::Datetime(_) => {
                input(
                    attrs: attributes! {
                        id=(control.as_str())
                        type="datetime-local"
                        required=(required)
                    }
                )
            }
            ColumnType::Enum(choice) => {
                select(
                    attrs: attributes! {
                        id=(control.as_str())
                        multiple=(choice.multiple.then_some(""))
                        required=(required)
                    },
                    if !choice.multiple {
                        <option value=""></option>
                    }
                    for option in &choice.options {
                        <option value=(option.id.inner())>
                            (option.label.as_str())
                        </option>
                    }
                )
            }
            ColumnType::Attachment(attachment) => {
                input(
                    attrs: attributes! {
                        id=(control.as_str())
                        type="file"
                        accept=(accept.as_deref())
                        multiple=(attachment.multiple.then_some(""))
                        required=(required)
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
                        type=(match &column.ty {
                            ColumnType::Text(text) => text_input_type(text),
                            _ => "text",
                        })
                        required=(required)
                    }
                )
            }
        }
    }
}

/// The editor's amber audience marker, on the preview's captions:
/// the eye-off glyph and the text carry it, never the colour alone
/// (platform P.4).
#[component]
async fn reviewer_badge(text: String) -> Result {
    view! {
        badge(
            variant: BadgeVariant::Outline,
            attrs: attributes! {
                class="border-transparent bg-amber-100 text-amber-900 \
                    dark:bg-amber-500/15 dark:text-amber-300"
            },
            icon(
                data: iconify_icon!("feather:eye-off"),
                attrs: attributes! { class="size-3" }
            )
            (text.as_str())
        )
    }
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
