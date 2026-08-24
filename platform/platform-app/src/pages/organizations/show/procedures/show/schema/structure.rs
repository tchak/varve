//! The **structure panel**: the draft's authored tree as nested
//! ordered lists — one row per element (icon, selecting link, kind
//! and multiplicity badges, the P.4 audience marker) with the row's
//! actions menu beside it.
//!
//! It is a `#[shard]` ([`structure`]) so an autosave can re-render it
//! without a navigation: the detail form bumps a `revision` signal
//! the shard reads, the browser posts the current arguments back, and
//! the server returns fresh markup. That makes it a **public
//! endpoint whose arguments the caller picks** — the page's guard
//! does not cover it — so it authorizes itself through `client(cx)`
//! and 404s on a procedure this caller cannot read.
//!
//! **No drag and drop**: moving is *up*, *down*, *top*, *bottom*,
//! *move after* a named sibling and *move to* a named container —
//! six actions a keyboard and a screen reader reach, each a real
//! form, and each expressible in the API's sibling-anchored
//! placement (G.7).

use cynic::QueryBuilder;
use platform_client::revision_draft::{
    Cardinality, Element, ProcedureRevisionDraft, ProcedureRevisionDraftQuery,
    ProcedureRevisionDraftVariables,
};
use topcoat::{
    Result,
    context::Cx,
    icon::{icon, iconify::iconify_icon},
    router::{
        error::{RouterErrorExt, not_found},
        href,
    },
    runtime::shard,
    view::{attributes, class, component, view},
};

use crate::{
    client,
    components::{
        badge::{BadgeVariant, badge},
        button::{ButtonSize, ButtonVariant, button_variants},
        dropdown_menu::{
            dropdown_menu, dropdown_menu_content, dropdown_menu_item, dropdown_menu_separator,
            dropdown_menu_sub, dropdown_menu_sub_content, dropdown_menu_sub_trigger,
            dropdown_menu_trigger,
        },
    },
    i18n::{t, t_args},
    pages::one_arg,
};

use super::super::super::super::OrganizationId;
use super::super::ProcedureId;
use super::element::{
    effectively_reviewer, element_icon, element_kind, id_of, kind_message_id, label_of,
    multiple_of_type, parent_of, unit_name, unit_of,
};
use super::elements::element::ElementId;
use super::{elements, page};

/// The panel as a shard: re-rendered when `revision`
/// changes (after an autosave). Authorizes itself through the client;
/// an unreadable procedure renders nothing rather than leaking.
#[shard]
pub(in crate::pages) async fn panel(
    cx: &Cx,
    procedure_id: String,
    selected: String,
    revision: f64,
) -> Result {
    let _ = revision;
    let client = client(cx).await?;
    let Ok(procedure_id) = procedure_id.parse::<uuid::Uuid>() else {
        return Err(not_found().into());
    };
    let procedure = platform_client::run(
        &client,
        ProcedureRevisionDraftQuery::build(ProcedureRevisionDraftVariables {
            id: cynic::Id::new(procedure_id.to_string()),
        }),
    )
    .await?
    .procedure
    .ok_or_not_found()?;
    let selected = (!selected.is_empty()).then_some(selected);
    view! { panel_body(procedure: procedure, selected: selected) }
}

/// The panel's body: the empty notice, or the tree.
#[component]
async fn panel_body(
    cx: &Cx,
    procedure: ProcedureRevisionDraft,
    selected: Option<String>,
) -> Result {
    let elements = procedure
        .revision_draft
        .as_ref()
        .map(|d| d.elements.clone())
        .unwrap_or_default();
    let empty = t(cx, "schema.structure.empty").await?;
    let organization_id: uuid::Uuid = procedure.organization.id.inner().parse()?;
    let procedure_id: uuid::Uuid = procedure.id.inner().parse()?;
    let labels = TreeLabels::load(cx).await?;
    let tree = Tree {
        elements,
        selected,
        organization_id,
        procedure_id,
        labels,
    };
    view! {
        if tree.elements.is_empty() {
            <p class="text-sm text-muted-foreground" data-schema-empty="">(empty)</p>
        } else {
            tree_list(tree: &tree, parent: None, depth: 0)
        }
    }
}

/// Everything the tree rows need, loaded once.
struct TreeLabels {
    actions: String,
    move_up: String,
    move_down: String,
    move_top: String,
    move_bottom: String,
    move_after: String,
    move_to: String,
    top_level: String,
    remove: String,
    one: String,
    many: String,
    reviewer: String,
}

impl TreeLabels {
    async fn load(cx: &Cx) -> Result<Self> {
        Ok(Self {
            actions: t(cx, "schema.actions").await?,
            move_up: t(cx, "schema.actions.move-up").await?,
            move_down: t(cx, "schema.actions.move-down").await?,
            move_top: t(cx, "schema.actions.move-top").await?,
            move_bottom: t(cx, "schema.actions.move-bottom").await?,
            move_after: t(cx, "schema.actions.move-after").await?,
            move_to: t(cx, "schema.actions.move-to").await?,
            top_level: t(cx, "schema.actions.top-level").await?,
            remove: t(cx, "schema.actions.remove").await?,
            one: t(cx, "schema.arity.one").await?,
            many: t(cx, "schema.arity.many").await?,
            reviewer: t(cx, "schema.audience.reviewer").await?,
        })
    }
}

struct Tree {
    elements: Vec<Element>,
    selected: Option<String>,
    organization_id: uuid::Uuid,
    procedure_id: uuid::Uuid,
    labels: TreeLabels,
}

impl Tree {
    /// An element's children in document order. `Element::Unknown` —
    /// a kind this client does not know — is skipped: it has no id,
    /// no label and no route, so a row for it would be an unnamed
    /// link over hrefs with an empty segment. The preview drops it
    /// the same way.
    fn children(&self, parent: Option<&str>) -> Vec<&Element> {
        self.elements
            .iter()
            .filter(|e| !matches!(e, Element::Unknown))
            .filter(|e| parent_of(e).as_deref() == parent)
            .collect()
    }

    /// Containers an element may move into — every group and section
    /// but itself and its descendants — as `(id, display text)`.
    fn destinations(&self, id: &str) -> Vec<(String, String)> {
        let mut excluded = vec![id.to_owned()];
        loop {
            let before = excluded.len();
            for e in &self.elements {
                if let Some(p) = parent_of(e)
                    && excluded.contains(&p)
                    && !excluded.contains(&id_of(e).to_owned())
                {
                    excluded.push(id_of(e).to_owned());
                }
            }
            if excluded.len() == before {
                break;
            }
        }
        self.elements
            .iter()
            .filter_map(|e| match e {
                Element::Group(g) if !excluded.contains(&g.id.inner().to_owned()) => {
                    Some((g.id.inner().to_owned(), g.label.clone()))
                }
                Element::Section(section) if !excluded.contains(&section.id.inner().to_owned()) => {
                    Some((section.id.inner().to_owned(), section.title.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// Effectively reviewer-only (platform P.4): the element or any
    /// ancestor carries the `Reviewer` audience.
    fn reviewer_only(&self, id: &str) -> bool {
        effectively_reviewer(&self.elements, id)
    }
}

/// One level of the tree as an ordered list; groups nest their own.
#[component(boxed)]
async fn tree_list(tree: &Tree, parent: Option<String>, depth: usize) -> Result {
    let children = tree.children(parent.as_deref());
    let count = children.len();
    view! {
        <ol
            class=(class!(
                "flex flex-col gap-2",
                "ml-4 mt-2 border-l border-border pl-3" if depth > 0,
            ))
        >
            for (index, element) in children.iter().enumerate() {
                tree_row(
                    key: id_of(element).to_owned(),
                    tree: tree,
                    element: (*element).clone(),
                    first: index == 0,
                    last: index + 1 == count,
                    depth: depth
                )
            }
        </ol>
    }
}

/// One element: its row (link, badges, actions), then its children.
/// Boxed like [`tree_list`]: the two recurse into each other per
/// nesting level, and an unboxed row's render frame is large enough
/// (badges, menus, facets) that a handful of levels overflow the
/// stack in debug builds — a section inside a section did.
#[component(boxed)]
async fn tree_row(
    cx: &Cx,
    tree: &Tree,
    element: Element,
    first: bool,
    last: bool,
    depth: usize,
) -> Result {
    let id = id_of(&element).to_owned();
    let is_selected = tree.selected.as_deref() == Some(id.as_str());
    let text = label_of(&element).to_owned();
    let summary = element_summary(cx, &element).await?;
    // The multiplicity badge: only types that can hold many values
    // say "one" or "many"; the rest say nothing.
    let (multiplicity, is_container) = match &element {
        Element::Column(c) => (
            match multiple_of_type(&c.ty) {
                Some(true) => tree.labels.many.clone(),
                Some(false) => tree.labels.one.clone(),
                None => String::new(),
            },
            false,
        ),
        Element::Group(g) => (
            match g.cardinality {
                Cardinality::One => tree.labels.one.clone(),
                Cardinality::Many => tree.labels.many.clone(),
            },
            true,
        ),
        Element::Section(_) => (String::new(), true),
        _ => (String::new(), false),
    };
    let actions_name = t_args(cx, "schema.actions.for", &one_arg("label", text.clone())).await?;
    let select_query = [("selected", id.clone())];
    let select_href = href!(
        page,
        OrganizationId(tree.organization_id),
        ProcedureId(tree.procedure_id)
    )
    .query(&select_query);
    let reviewer_only = tree.reviewer_only(&id);
    // A section reads as a heading bar, a note as an aside; groups
    // and columns keep the plain data-carrying row.
    let is_section = matches!(element, Element::Section(_));
    let is_note = matches!(element, Element::Note(_));
    let current_parent = parent_of(&element);
    view! {
        <li
            data-element-id=(id.as_str())
            data-element-kind=(element_kind(&element))
            data-audience=(reviewer_only.then_some("reviewer"))
        >
            <div
                class=(class!(
                    "flex items-center gap-2 rounded-lg border px-3 py-2",
                    "border-ring ring-2 ring-ring/40" if is_selected else "border-border",
                    "bg-muted/50" if is_section,
                    "border-dashed" if is_note,
                ))
            >
                icon(
                    data: element_icon(&element),
                    attrs: attributes! { class="size-4 shrink-0 text-muted-foreground" }
                )
                <a
                    href=(select_href)
                    class=(class!(
                        "min-w-0 flex-1 truncate text-sm underline-offset-4 hover:underline",
                        "font-semibold" if is_section else "font-medium",
                        "italic text-muted-foreground" if is_note,
                    ))
                    aria-current=(is_selected.then_some("true"))
                >
                    (text.as_str())
                </a>
                badge(variant: BadgeVariant::Outline, (summary.as_str()))
                if !multiplicity.is_empty() {
                    badge(variant: BadgeVariant::Secondary, (multiplicity.as_str()))
                }
                if reviewer_only {
                    // The audience marker must not read as one more
                    // type badge: an amber tint (dark values too — a
                    // colour outside the theme tokens, recorded in
                    // platform.md P.4 as the a11y contract asks) and
                    // an eye-off glyph set it apart; the text stays
                    // the carrier, never the colour alone.
                    badge(
                        variant: BadgeVariant::Outline,
                        attrs: attributes! {
                            class="border-transparent bg-amber-100 text-amber-900                                    dark:bg-amber-500/15 dark:text-amber-300"
                        },
                        icon(
                            data: iconify_icon!("feather:eye-off"),
                            attrs: attributes! { class="size-3" }
                        )
                        (tree.labels.reviewer.as_str())
                    )
                }
                row_actions(
                    tree: tree,
                    facts: RowFacts {
                        id: id.clone(),
                        actions_name: actions_name.clone(),
                        first,
                        last,
                        current_parent: current_parent.clone(),
                    }
                )
            </div>
            if is_container {
                tree_list(tree: tree, parent: Some(id.clone()), depth: depth + 1)
            }
        </li>
    }
}

/// One row's placement facts, bundled for [`row_actions`].
struct RowFacts {
    id: String,
    actions_name: String,
    first: bool,
    last: bool,
    current_parent: Option<String>,
}

/// One row's actions menu, a boxed component of its own: it is most
/// of a row's markup, and pulling it out of the recursive
/// [`tree_row`]/[`tree_list`] pair keeps each nesting level's render
/// frame small (the stack-overflow regression
/// `deeply_nested_containers_render` pins).
#[component(boxed)]
async fn row_actions(tree: &Tree, facts: RowFacts) -> Result {
    let RowFacts {
        id,
        actions_name,
        first,
        last,
        current_parent,
    } = facts;
    let relocate_href = || {
        href!(
            elements::element::relocate::submit,
            OrganizationId(tree.organization_id),
            ProcedureId(tree.procedure_id),
            ElementId(id.clone())
        )
    };
    let remove_href = href!(
        elements::element::remove::submit,
        OrganizationId(tree.organization_id),
        ProcedureId(tree.procedure_id),
        ElementId(id.clone())
    );
    let destinations = tree.destinations(&id);
    let at_root = current_parent.is_none();
    // "Move to" offers the top level (unless already there) and every
    // group but the current parent, itself and its descendants; with
    // nothing enabled the trigger itself is a disabled item.
    let move_to_enabled = !at_root
        || destinations
            .iter()
            .any(|(destination_id, _)| current_parent.as_deref() != Some(destination_id.as_str()));
    // "Move after": same-level siblings only, never itself.
    let siblings: Vec<(String, String)> = tree
        .children(current_parent.as_deref())
        .into_iter()
        .filter(|e| id_of(e) != id)
        .map(|e| (id_of(e).to_owned(), label_of(e).to_owned()))
        .collect();
    view! {
        dropdown_menu(
            dropdown_menu_trigger(
                attrs: attributes! {
                    aria-label=(actions_name.as_str())
                    class=(button_variants(ButtonVariant::Ghost, ButtonSize::Sm))
                },
                (tree.labels.actions.as_str())
            )
            dropdown_menu_content(
                attrs: attributes! { class="right-0 left-auto" },
                <form method="post" action=(relocate_href())>
                    <input type="hidden" name="direction" value="up">
                    dropdown_menu_item(
                        attrs: attributes! { type="submit" disabled=(first) },
                        (tree.labels.move_up.as_str())
                    )
                </form>
                <form method="post" action=(relocate_href())>
                    <input type="hidden" name="direction" value="down">
                    dropdown_menu_item(
                        attrs: attributes! { type="submit" disabled=(last) },
                        (tree.labels.move_down.as_str())
                    )
                </form>
                <form method="post" action=(relocate_href())>
                    <input type="hidden" name="direction" value="top">
                    dropdown_menu_item(
                        attrs: attributes! { type="submit" disabled=(first) },
                        (tree.labels.move_top.as_str())
                    )
                </form>
                <form method="post" action=(relocate_href())>
                    <input type="hidden" name="direction" value="bottom">
                    dropdown_menu_item(
                        attrs: attributes! { type="submit" disabled=(last) },
                        (tree.labels.move_bottom.as_str())
                    )
                </form>
                if siblings.is_empty() {
                    dropdown_menu_item(
                        attrs: attributes! { type="button" disabled="" data-menu="move-after" },
                        (tree.labels.move_after.as_str())
                    )
                } else {
                    dropdown_menu_sub(
                        attrs: attributes! { data-menu="move-after" },
                        dropdown_menu_sub_trigger((tree.labels.move_after.as_str()))
                        dropdown_menu_sub_content(
                            for (sibling_id, sibling_label) in &siblings {
                                <form method="post" action=(relocate_href())>
                                    <input
                                        type="hidden"
                                        name="after"
                                        value=(sibling_id.as_str())
                                    >
                                    dropdown_menu_item(
                                        attrs: attributes! { type="submit" },
                                        (sibling_label.as_str())
                                    )
                                </form>
                            }
                        )
                    )
                }
                if !move_to_enabled {
                    dropdown_menu_item(
                        attrs: attributes! { type="button" disabled="" data-menu="move-to" },
                        (tree.labels.move_to.as_str())
                    )
                } else {
                    dropdown_menu_sub(
                        attrs: attributes! { data-menu="move-to" },
                        dropdown_menu_sub_trigger((tree.labels.move_to.as_str()))
                        dropdown_menu_sub_content(
                            if !at_root {
                                <form method="post" action=(relocate_href())>
                                    <input type="hidden" name="parent" value="">
                                    dropdown_menu_item(
                                        attrs: attributes! { type="submit" },
                                        (tree.labels.top_level.as_str())
                                    )
                                </form>
                            }
                            for (destination_id, destination_label) in &destinations {
                                if current_parent.as_deref()
                                    != Some(destination_id.as_str()) {
                                    <form method="post" action=(relocate_href())>
                                        <input
                                            type="hidden"
                                            name="parent"
                                            value=(destination_id.as_str())
                                        >
                                        dropdown_menu_item(
                                            attrs: attributes! { type="submit" },
                                            (destination_label.as_str())
                                        )
                                    </form>
                                }
                            }
                        )
                    )
                }
                dropdown_menu_separator()
                <form method="post" action=(remove_href)>
                    dropdown_menu_item(
                        attrs: attributes! { type="submit" class="text-destructive" },
                        (tree.labels.remove.as_str())
                    )
                </form>
            )
        )
    }
}

/// A column's type in a word or two; a group's "group".
async fn element_summary(cx: &Cx, element: &Element) -> Result<String> {
    Ok(match element {
        Element::Group(_) => t(cx, "schema.kind.group").await?,
        Element::Column(c) => {
            let kind = t(cx, kind_message_id(&c.ty)).await?;
            match unit_of(&c.ty) {
                Some(unit) => format!("{kind} ({})", unit_name(unit)),
                None => kind,
            }
        }
        Element::Section(_) => t(cx, "schema.kind.section").await?,
        Element::Note(_) => t(cx, "schema.kind.note").await?,
        _ => String::new(),
    })
}
