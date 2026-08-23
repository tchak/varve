//! `/organizations/{id}/procedures/{pid}/schema`: the **schema
//! editor** over the procedure's revision draft (design/platform.md
//! P.4, `design/graphql.md` G.7), through the typed client like every
//! other page — the app is integrator #1.
//!
//! **Master–detail, state in the URL.** The structure panel lists the
//! draft as nested lists; `?selected=<id>` picks the element whose
//! form the detail panel shows (deep-linkable, `aria-current` on the
//! row); nothing selected shows the add form. Every action is a real
//! form: adding ([`add::submit`]), the selected element's fields
//! ([`elements::element::update`]), *move up / down / to* and
//! *remove* from a row's actions menu ([`elements::element::relocate`],
//! [`elements::element::remove`]),
//! and discarding the draft ([`discard::submit`], behind a
//! confirmation state). One POST per module segment is what
//! `module_router!` offers, hence one inline module per action. Post → 303 → get, a one-shot notice on the landing page.
//!
//! **Then the runtime, as an enhancement.** With the browser script
//! (pages.rs: linked when the asset bundle is loaded — never in
//! router-level tests, so everything below is also proven without
//! it), the detail form **autosaves on change**: each simple field
//! calls [`save_field`], a `#[procedure]`, and reports through a
//! `role="status"` line (saving / saved / the server's reason); a
//! successful save bumps a revision signal that re-renders the
//! structure panel, a `#[shard]` ([`structure`]), so the tree shows
//! the new label without a reload. Type-dependent fieldsets hide
//! when the chosen kind cannot use them. An enum's options are a
//! card of their own under the column form: one row per option (its
//! label autosaving, a red remove button), an explicit *Add option*
//! form beneath; an empty choice is a draft state, publication's to
//! refuse. Procedure and shard
//! authorize themselves: they are public endpoints whose arguments
//! the caller picks, and the client (`client(cx)`) is the guard,
//! exactly as for a page.
//!
//! **No drag and drop**: moving is *up*, *down*, and *move to* a
//! group or the top level — three named actions a keyboard and a
//! screen reader reach, and that the API's sibling-anchored placement
//! (G.7) expresses directly.

pub(super) mod elements;

use cynic::{MutationBuilder, QueryBuilder};
use platform_client::revision_draft::{
    AddColumn, AddColumnInput, AddColumnVariables, AddGroup, AddGroupInput, AddGroupVariables,
    Arity, AttachmentType, Cardinality, ColumnType, ColumnTypeInput, DiscardRevisionDraft,
    DiscardRevisionDraftInput, DiscardRevisionDraftVariables, PlacementInput,
    ProcedureRevisionDraft, ProcedureRevisionDraftQuery, ProcedureRevisionDraftVariables,
    SchemaColumn, SchemaElement, SchemaGroup, Unit,
};
use platform_client::{Code, Error};
use serde::{Deserialize, Serialize};
use topcoat::{
    Result,
    context::Cx,
    icon::{icon, iconify::iconify_icon},
    router::{
        content::Form,
        error::{RouterErrorExt, not_found},
        href, page, path_param, query_params,
    },
    runtime::{Event, procedure, shard},
    view::{StaticClass, attributes, class, component, view},
};

use crate::{
    client,
    components::{
        alert::{AlertVariant, alert, alert_description},
        badge::{BadgeVariant, badge},
        button::{ButtonSize, ButtonVariant, button, button_variants},
        card::{card, card_content, card_footer, card_header},
        dropdown_menu::{
            dropdown_menu, dropdown_menu_content, dropdown_menu_item, dropdown_menu_separator,
            dropdown_menu_sub, dropdown_menu_sub_content, dropdown_menu_sub_trigger,
            dropdown_menu_trigger,
        },
        field::field,
        label::label,
        page_title::page_title,
        select::select,
    },
    flash,
    i18n::{t, t_args},
    pages::{args, one_arg, utc_date_arg},
};

use super::super::super::OrganizationId;
use super::{ProcedureId, counts, procedure_draft};
use elements::Fields;
use elements::element::ElementId;

/// The editor's query: the selected element, and the pending
/// discard confirmation.
#[query_params(error = redirect("?"))]
pub(super) struct EditorQuery {
    selected: Option<String>,
    discard: Option<String>,
}

/// The one-shot notice a POST leaves for the next GET.
#[derive(Serialize, Deserialize, Clone)]
pub(super) struct Notice {
    /// `status` (neutral confirmation) or `alert` (a refused action).
    pub kind: NoticeKind,
    /// Display text, already localized at POST time.
    pub text: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoticeKind {
    Status,
    Alert,
}

pub(super) const NOTICE: &str = "schema-notice";

/// The editor page.
#[page]
pub async fn page(cx: &Cx) -> Result {
    let procedure = procedure_draft(cx).await?;
    let query = query_params::<EditorQuery>(cx)?;
    let notice = flash::take::<Notice>(cx, NOTICE);
    view! {
        editor_page(
            procedure: procedure,
            selected: query.selected.clone(),
            confirm_discard: query.discard.as_deref() == Some("confirm"),
            notice: notice
        )
    }
}

/// Where a POST lands: the editor, with `selected` and a notice.
pub(super) async fn back_to_editor(
    cx: &Cx,
    selected: Option<&str>,
    notice: Option<Notice>,
) -> Result {
    let organization = path_param::<OrganizationId>(cx)?;
    let procedure = path_param::<ProcedureId>(cx)?;
    if let Some(notice) = notice {
        flash::set(cx, NOTICE, notice)?;
    }
    let href = href!(page, OrganizationId(*organization), ProcedureId(*procedure));
    let location = match selected {
        Some(id) => href.query(&[("selected", id)]).resolve(cx),
        None => href.resolve(cx),
    };
    crate::pages::redirect_to(cx, location).await
}

/// A refused client operation, mapped the editor's way: `FORBIDDEN`
/// is the 404; `INVALID_EDIT` / `CONFLICT` / `INVALID_INPUT` become
/// an alert notice carrying the server's reason; anything else is
/// the request's error.
pub(super) async fn refused(cx: &Cx, error: Error) -> Result<Notice> {
    match error.code() {
        Some(Code::Forbidden) => Err(not_found().into()),
        Some(Code::InvalidEdit | Code::Conflict | Code::InvalidInput) => Ok(Notice {
            kind: NoticeKind::Alert,
            text: match error.code() {
                Some(Code::Conflict) => t(cx, "schema.error.conflict").await?,
                _ => {
                    t_args(
                        cx,
                        "schema.error.refused",
                        &one_arg("reason", error.to_string()),
                    )
                    .await?
                }
            },
        }),
        _ => Err(error.into()),
    }
}

/// A confirmed action's notice.
pub(super) async fn done(cx: &Cx, id: &str) -> Result<Notice> {
    Ok(Notice {
        kind: NoticeKind::Status,
        text: t(cx, id).await?,
    })
}

/// An add submission: a column or a group, its label, where it goes.
#[derive(Deserialize)]
pub(super) struct Addition {
    what: String,
    label: String,
    #[serde(default)]
    parent: String,
    #[serde(default)]
    before: String,
}

/// `…/schema/add`: [`add::submit`].
pub(super) mod add {
    use super::*;

    /// `addColumn` / `addGroup`; the new element becomes the selection.
    /// A blank label lands back with an alert — the add form is the
    /// detail panel's, and the notice is how it reports.
    #[page(POST)]
    pub(super) async fn submit(cx: &Cx, Form(input): Form<Addition>) -> Result {
        let client = client(cx).await?;
        let procedure = procedure_draft(cx).await?;
        let new_label = input.label.trim().to_owned();
        let parent = (!input.parent.is_empty()).then_some(input.parent.as_str());
        let before = (!input.before.is_empty()).then_some(input.before.as_str());
        if new_label.is_empty() {
            let notice = Notice {
                kind: NoticeKind::Alert,
                text: t(cx, "schema.error.label-required").await?,
            };
            return back_to_editor(cx, parent, Some(notice)).await;
        }
        let placement = Some(PlacementInput {
            parent_id: parent.map(cynic::Id::new),
            before_id: before.map(cynic::Id::new),
        });
        let procedure_id = cynic::Id::new(procedure.id.inner());
        let result = if input.what == "group" {
            platform_client::run(
                &client,
                AddGroup::build(AddGroupVariables {
                    input: AddGroupInput {
                        procedure_id,
                        placement,
                        label: new_label,
                        cardinality: None,
                    },
                }),
            )
            .await
            .map(|r| r.add_group)
        } else {
            platform_client::run(
                &client,
                AddColumn::build(AddColumnVariables {
                    input: AddColumnInput {
                        procedure_id,
                        placement,
                        label: new_label,
                        ty: ColumnTypeInput::text(),
                        arity: None,
                    },
                }),
            )
            .await
            .map(|r| r.add_column)
        };
        match result {
            Ok(procedure) => {
                let elements = procedure
                    .revision_draft
                    .as_ref()
                    .map(|d| d.schema.elements.as_slice())
                    .unwrap_or_default();
                let created = new_element_id(elements, parent, before);
                let notice = done(
                    cx,
                    if input.what == "group" {
                        "schema.notice.group-added"
                    } else {
                        "schema.notice.column-added"
                    },
                )
                .await?;
                back_to_editor(cx, created.as_deref(), Some(notice)).await
            }
            Err(error) => {
                let notice = refused(cx, error).await?;
                back_to_editor(cx, parent, Some(notice)).await
            }
        }
    }
}

/// The element an add created (G.7): the one in front of `before` in
/// its parent, or the parent's last child.
fn new_element_id(
    elements: &[SchemaElement],
    parent: Option<&str>,
    before: Option<&str>,
) -> Option<String> {
    let siblings: Vec<&SchemaElement> = elements
        .iter()
        .filter(|e| parent_of(e).as_deref() == parent)
        .collect();
    let index = match before {
        Some(anchor) => siblings
            .iter()
            .position(|e| id_of(e) == anchor)?
            .checked_sub(1)?,
        None => siblings.len().checked_sub(1)?,
    };
    Some(id_of(siblings[index]).to_owned())
}

/// `…/schema/discard`: [`discard::submit`].
pub(super) mod discard {
    use super::*;

    /// `discardRevisionDraft`, after the confirmation state.
    #[page(POST)]
    pub(super) async fn submit(cx: &Cx) -> Result {
        let client = client(cx).await?;
        let procedure = path_param::<ProcedureId>(cx)?;
        let result = platform_client::run(
            &client,
            DiscardRevisionDraft::build(DiscardRevisionDraftVariables {
                input: DiscardRevisionDraftInput {
                    procedure_id: cynic::Id::new(procedure.to_string()),
                },
            }),
        )
        .await;
        match result {
            Ok(_) => {
                let notice = done(cx, "schema.notice.discarded").await?;
                back_to_editor(cx, None, Some(notice)).await
            }
            Err(error) => {
                let notice = refused(cx, error).await?;
                back_to_editor(cx, None, Some(notice)).await
            }
        }
    }
}

/// Autosave of one field of the selected element (the runtime path
/// of [`elements::update`]): `Ok(Ok(text))` is the confirmation to
/// show, `Ok(Err(text))` the reason the save was refused — outcome as
/// data, since a procedure's `Err` is invisible to the caller.
/// Authorizes through the client like every page; `FORBIDDEN` and
/// the rest surface as the refused text.
#[procedure]
async fn save_field(
    cx: &Cx,
    procedure_id: String,
    element_id: String,
    field_name: String,
    value: String,
) -> Result<std::result::Result<String, String>> {
    let client = client(cx).await?;
    let procedure_id: uuid::Uuid = match procedure_id.parse() {
        Ok(id) => id,
        Err(_) => return Ok(Err(t(cx, "schema.error.conflict").await?)),
    };
    let procedure = platform_client::run(
        &client,
        ProcedureRevisionDraftQuery::build(ProcedureRevisionDraftVariables {
            id: cynic::Id::new(procedure_id.to_string()),
        }),
    )
    .await;
    let procedure: ProcedureRevisionDraft = match procedure {
        Ok(ProcedureRevisionDraftQuery { procedure: Some(p) }) => p,
        _ => return Ok(Err(t(cx, "schema.error.conflict").await?)),
    };
    let fields = Fields::from_pairs(vec![(field_name, value)]);
    match elements::apply_update(cx, &client, &procedure, &element_id, &fields).await? {
        Ok(()) => Ok(Ok(t(cx, "schema.status.saved").await?)),
        Err(notice) => Ok(Err(notice.text)),
    }
}

/// Autosave of one enum option's label: `input_id` is the row's
/// input id (`option-<id>`), the one thing a handler can read off the
/// event besides the value. Same outcome shape as [`save_field`].
#[procedure]
async fn save_option(
    cx: &Cx,
    procedure_id: String,
    element_id: String,
    input_id: String,
    value: String,
) -> Result<std::result::Result<String, String>> {
    let client = client(cx).await?;
    let Some(option_id) = input_id.strip_prefix("option-") else {
        return Ok(Err(t(cx, "schema.error.conflict").await?));
    };
    let procedure = match procedure_id.parse::<uuid::Uuid>() {
        Ok(id) => {
            platform_client::run(
                &client,
                ProcedureRevisionDraftQuery::build(ProcedureRevisionDraftVariables {
                    id: cynic::Id::new(id.to_string()),
                }),
            )
            .await
        }
        Err(_) => return Ok(Err(t(cx, "schema.error.conflict").await?)),
    };
    let procedure: ProcedureRevisionDraft = match procedure {
        Ok(ProcedureRevisionDraftQuery { procedure: Some(p) }) => p,
        _ => return Ok(Err(t(cx, "schema.error.conflict").await?)),
    };
    match elements::rename_option(cx, &client, &procedure, &element_id, option_id, &value).await? {
        Ok(()) => Ok(Ok(t(cx, "schema.status.saved").await?)),
        Err(notice) => Ok(Err(notice.text)),
    }
}

/// The structure panel as a shard: re-rendered when `revision`
/// changes (after an autosave). Authorizes itself through the client;
/// an unreadable procedure renders nothing rather than leaking.
#[shard]
async fn structure(cx: &Cx, procedure_id: String, selected: String, revision: f64) -> Result {
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
    view! { structure_panel(procedure: procedure, selected: selected) }
}

// ---------------------------------------------------------------- views

/// What the detail panel shows for the selected element, computed
/// before the view so the runtime handlers (which can only reach
/// signals and the event) have every value as a signal.
enum Detail {
    Column(ColumnDetail),
    Group(GroupDetail),
    Nothing,
}

struct ColumnDetail {
    column: SchemaColumn,
    kind: String,
    unit: Option<Unit>,
    /// `(id, label, accessible name of its remove button)`.
    options: Vec<(String, String, String)>,
    accept: String,
    max_bytes: Option<String>,
}

struct GroupDetail {
    group: SchemaGroup,
}

/// The page: header with the draft state, the notice, the structure
/// shard, and the detail form — one `view!`, because the autosave
/// handlers bump the `revision` signal the shard reads, and a runtime
/// closure reaches signals declared in its own `view!` only.
#[component]
async fn editor_page(
    cx: &Cx,
    procedure: ProcedureRevisionDraft,
    selected: Option<String>,
    confirm_discard: bool,
    notice: Option<Notice>,
) -> Result {
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
    let elements = procedure
        .revision_draft
        .as_ref()
        .map(|d| d.schema.elements.clone())
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
    let discard_label = t(cx, "schema.discard").await?;
    let discard_question = t(cx, "schema.discard.question").await?;
    let discard_confirm = t(cx, "schema.discard.confirm").await?;
    let discard_keep = t(cx, "schema.discard.keep").await?;
    let structure_heading = t(cx, "schema.structure.title").await?;
    let add_element_label = t(cx, "schema.add.title").await?;
    let selected_element = selected
        .as_deref()
        .and_then(|id| elements.iter().find(|e| id_of(e) == id).cloned());
    let selected_id = selected_element.as_ref().map(|e| id_of(e).to_owned());
    let page_href = || {
        href!(
            page,
            OrganizationId(organization_id),
            ProcedureId(procedure_id)
        )
    };
    let discard_href = href!(
        discard::submit,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    );
    let procedure_id_string = procedure_id.to_string();
    let selected_string = selected_id.clone().unwrap_or_default();

    // The detail form's texts and facts.
    let label_label = t(cx, "form.label").await?;
    let kind_label = t(cx, "schema.type").await?;
    let unit_label = t(cx, "schema.unit").await?;
    let unit_none = t(cx, "schema.unit.none").await?;
    let arity_label = t(cx, "schema.arity").await?;
    let arity_one = t(cx, "schema.arity.one").await?;
    let arity_many = t(cx, "schema.arity.many").await?;
    let cardinality_label = t(cx, "schema.cardinality").await?;
    let cardinality_one = t(cx, "schema.cardinality.one").await?;
    let cardinality_many = t(cx, "schema.cardinality.many").await?;
    let options_label = t(cx, "schema.options").await?;
    let options_help = t(cx, "schema.options.help").await?;
    let option_label = t(cx, "schema.options.label").await?;
    let options_empty = t(cx, "schema.options.empty").await?;
    let option_new_label = t(cx, "schema.options.new").await?;
    let options_add = t(cx, "schema.options.add").await?;
    let accept_label = t(cx, "schema.attachment.accept").await?;
    let accept_help = t(cx, "schema.attachment.accept.help").await?;
    let max_bytes_label = t(cx, "schema.attachment.max-bytes").await?;
    let save = t(cx, "schema.save").await?;
    let saving_text = t(cx, "schema.status.saving").await?;
    let mut kind_names = Vec::new();
    for kind in KINDS {
        kind_names.push((*kind, t(cx, kind_message_id_of(kind)).await?));
    }
    let mut detail = match &selected_element {
        Some(SchemaElement::Column(column)) => Detail::Column(ColumnDetail {
            column: column.clone(),
            kind: kind_of(&column.ty).to_owned(),
            unit: unit_of(&column.ty),
            options: match &column.ty {
                ColumnType::Enum(e) => e
                    .options
                    .iter()
                    .map(|o| (o.id.inner().to_owned(), o.label.clone(), String::new()))
                    .collect(),
                _ => Vec::new(),
            },
            accept: match &column.ty {
                ColumnType::Attachment(AttachmentType { accept, .. }) => accept.join(", "),
                _ => String::new(),
            },
            max_bytes: match &column.ty {
                ColumnType::Attachment(AttachmentType { max_bytes, .. }) => {
                    max_bytes.map(|n| n.to_string())
                }
                _ => None,
            },
        }),
        Some(SchemaElement::Group(group)) => Detail::Group(GroupDetail {
            group: group.clone(),
        }),
        _ => Detail::Nothing,
    };
    if let Detail::Column(c) = &mut detail {
        for (_, option_text, remove_name) in &mut c.options {
            *remove_name = t_args(
                cx,
                "schema.options.remove",
                &one_arg("label", option_text.clone()),
            )
            .await?;
        }
    }
    let detail_heading = match &detail {
        Detail::Column(c) => {
            t_args(
                cx,
                "schema.detail.column",
                &one_arg("label", c.column.label.clone()),
            )
            .await?
        }
        Detail::Group(g) => {
            t_args(
                cx,
                "schema.detail.group",
                &one_arg("label", g.group.label.clone()),
            )
            .await?
        }
        Detail::Nothing => String::new(),
    };
    let initial_kind = match &detail {
        Detail::Column(c) => c.kind.clone(),
        _ => String::new(),
    };
    let update_href = || {
        href!(
            elements::element::update::submit,
            OrganizationId(organization_id),
            ProcedureId(procedure_id),
            ElementId(selected_string.clone())
        )
    };
    let option_add_href = href!(
        elements::element::options::add::submit,
        OrganizationId(organization_id),
        ProcedureId(procedure_id),
        ElementId(selected_string.clone())
    );
    let option_update_href = || {
        href!(
            elements::element::options::update::submit,
            OrganizationId(organization_id),
            ProcedureId(procedure_id),
            ElementId(selected_string.clone())
        )
    };
    let option_remove_href = || {
        href!(
            elements::element::options::remove::submit,
            OrganizationId(organization_id),
            ProcedureId(procedure_id),
            ElementId(selected_string.clone())
        )
    };
    view! {
        signal revision = 0.0;
        signal status = String::new();
        signal option_status = String::new();
        signal kind = initial_kind.clone();
        signal pid = procedure_id_string.clone();
        signal eid = selected_string.clone();
        signal saving = saving_text.clone();

        <div class="flex flex-col gap-6">
            <div class="flex flex-col gap-2">
                page_title((title))
                <p class="text-sm text-muted-foreground">
                    <a
                        href=(href!(super::page, OrganizationId(organization_id), ProcedureId(procedure_id)))
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
                    if procedure.revision_draft.is_some() && !confirm_discard {
                        <a
                            href=(page_href().query(&[("discard", "confirm")]))
                            class=(button_variants(
                                ButtonVariant::Ghost,
                                ButtonSize::Sm,
                            ))
                        >
                            (discard_label)
                        </a>
                    }
                </div>
            </div>
            if confirm_discard {
                alert(
                    variant: AlertVariant::Destructive,
                    attrs: attributes! { role="alertdialog" aria-labelledby="discard-question" },
                    alert_description(
                        <p id="discard-question" class="mb-3">(discard_question)</p>
                        <div class="flex gap-2">
                            <form method="post" action=(discard_href)>
                                button(
                                    variant: ButtonVariant::Destructive,
                                    size: ButtonSize::Sm,
                                    attrs: attributes! { type="submit" },
                                    (discard_confirm)
                                )
                            </form>
                            <a
                                href=(page_href())
                                class=(button_variants(
                                    ButtonVariant::Outline,
                                    ButtonSize::Sm,
                                ))
                            >
                                (discard_keep)
                            </a>
                        </div>
                    )
                )
            }
            if let Some(notice) = &notice {
                alert(
                    variant: match notice.kind {
                        NoticeKind::Status => AlertVariant::Neutral,
                        NoticeKind::Alert => AlertVariant::Destructive,
                    },
                    attrs: attributes! {
                        role=(match notice.kind {
                            NoticeKind::Status => "status",
                            NoticeKind::Alert => "alert",
                        })
                        data-schema-notice=""
                    },
                    alert_description((notice.text.as_str()))
                )
            }
            <div class="grid gap-6 md:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
                <section aria-labelledby="schema-structure-heading" class="min-w-0">
                    <div class="mb-3 flex items-center justify-between gap-3">
                        <h2 id="schema-structure-heading" class="text-sm font-semibold">
                            (structure_heading)
                        </h2>
                        // The add form lives in the detail panel's unselected
                        // state; this is the way back to it once a row is
                        // selected.
                        if selected_id.is_some() {
                            <a
                                href=(page_href())
                                class=(button_variants(
                                    ButtonVariant::Outline,
                                    ButtonSize::Sm,
                                ))
                                data-schema-add=""
                            >
                                (add_element_label.as_str())
                            </a>
                        }
                    </div>
                    structure(
                        procedure_id: $(pid.get()),
                        selected: $(eid.get()),
                        revision: $(revision.get())
                    )
                </section>
                <section aria-labelledby="schema-detail-heading" class="min-w-0">
                    match &detail {
                        Detail::Group(g) => {
                            card(
                                card_header(
                                    <h2
                                        id="schema-detail-heading"
                                        class="leading-none font-semibold"
                                    >
                                        (detail_heading.as_str())
                                    </h2>
                                )
                                <form
                                    method="post"
                                    action=(update_href())
                                    class="contents"
                                    data-element-form=(selected_string.as_str())
                                >
                                    card_content(
                                        <div class="flex flex-col gap-4">
                                            <div class="flex flex-col gap-2">
                                                label(
                                                    attrs: attributes! { for="element-label" },
                                                    (label_label.as_str())
                                                )
                                                <input
                                                    id="element-label"
                                                    class=(INPUT)
                                                    type="text"
                                                    name="label"
                                                    value=(g.group.label.as_str())
                                                    required=""
                                                    autocomplete="off"
                                                    @change=$(async |e: Event| {
                                                        status.set(saving.get());
                                                        let outcome = save_field(
                                                                pid.get(),
                                                                eid.get(),
                                                                "label".to_owned(),
                                                                e.target.value,
                                                            )
                                                            .await;
                                                        if outcome.is_ok() {
                                                            status.set(outcome.unwrap());
                                                            revision.increment();
                                                        } else {
                                                            status.set(outcome.unwrap_err());
                                                        }
                                                    })
                                                >
                                            </div>
                                            <div class="flex flex-col gap-2">
                                                label(
                                                    attrs: attributes! { for="element-cardinality" },
                                                    (cardinality_label.as_str())
                                                )
                                                <select
                                                    id="element-cardinality"
                                                    class=(SELECT)
                                                    name="cardinality"
                                                    @change=$(async |e: Event| {
                                                        status.set(saving.get());
                                                        let outcome = save_field(
                                                                pid.get(),
                                                                eid.get(),
                                                                "cardinality".to_owned(),
                                                                e.target.value,
                                                            )
                                                            .await;
                                                        if outcome.is_ok() {
                                                            status.set(outcome.unwrap());
                                                            revision.increment();
                                                        } else {
                                                            status.set(outcome.unwrap_err());
                                                        }
                                                    })
                                                >
                                                    <option
                                                        value="ONE"
                                                        selected=(matches!(g.group.cardinality, Cardinality::One))
                                                    >
                                                        (cardinality_one.as_str())
                                                    </option>
                                                    <option
                                                        value="MANY"
                                                        selected=(matches!(g.group.cardinality, Cardinality::Many))
                                                    >
                                                        (cardinality_many.as_str())
                                                    </option>
                                                </select>
                                            </div>
                                            <p
                                                role="status"
                                                class="min-h-5 text-sm text-muted-foreground"
                                                data-save-status=""
                                            >
                                                $(status.get())
                                            </p>
                                        </div>
                                    )
                                    card_footer(
                                        button(
                                            attrs: attributes! { type="submit" },
                                            (save.as_str())
                                        )
                                    )
                                </form>
                            )
                            <div class="mt-6">
                                add_form(
                                    procedure_id: procedure_id,
                                    organization_id: organization_id,
                                    parent: Some(selected_string.clone()),
                                    heading_level_top: false
                                )
                            </div>
                        }
                        Detail::Column(c) => {
                            card(
                                card_header(
                                    <h2
                                        id="schema-detail-heading"
                                        class="leading-none font-semibold"
                                    >
                                        (detail_heading.as_str())
                                    </h2>
                                )
                                <form
                                    method="post"
                                    action=(update_href())
                                    class="contents"
                                    data-element-form=(selected_string.as_str())
                                >
                                    card_content(
                                        <div class="flex flex-col gap-4">
                                            <div class="flex flex-col gap-2">
                                                label(
                                                    attrs: attributes! { for="element-label" },
                                                    (label_label.as_str())
                                                )
                                                <input
                                                    id="element-label"
                                                    class=(INPUT)
                                                    type="text"
                                                    name="label"
                                                    value=(c.column.label.as_str())
                                                    required=""
                                                    autocomplete="off"
                                                    @change=$(async |e: Event| {
                                                        status.set(saving.get());
                                                        let outcome = save_field(
                                                                pid.get(),
                                                                eid.get(),
                                                                "label".to_owned(),
                                                                e.target.value,
                                                            )
                                                            .await;
                                                        if outcome.is_ok() {
                                                            status.set(outcome.unwrap());
                                                            revision.increment();
                                                        } else {
                                                            status.set(outcome.unwrap_err());
                                                        }
                                                    })
                                                >
                                            </div>
                                            <div class="flex flex-col gap-2">
                                                label(
                                                    attrs: attributes! { for="element-kind" },
                                                    (kind_label.as_str())
                                                )
                                                <select
                                                    id="element-kind"
                                                    class=(SELECT)
                                                    name="kind"
                                                    @change=$(async |e: Event| {
                                                        kind.set(e.target.value.to_owned());
                                                        status.set(saving.get());
                                                        let outcome = save_field(
                                                                pid.get(),
                                                                eid.get(),
                                                                "kind".to_owned(),
                                                                e.target.value,
                                                            )
                                                            .await;
                                                        if outcome.is_ok() {
                                                            status.set(outcome.unwrap());
                                                            revision.increment();
                                                        } else {
                                                            status.set(outcome.unwrap_err());
                                                        }
                                                    })
                                                >
                                                    for (kind_value, name) in &kind_names {
                                                        <option
                                                            value=(*kind_value)
                                                            selected=(*kind_value == c.kind)
                                                        >
                                                            (name.as_str())
                                                        </option>
                                                    }
                                                </select>
                                            </div>
                                            <div
                                                class="flex flex-col gap-2"
                                                :hidden=$(if kind.get() == "INTEGER" {
                                                    false
                                                } else {
                                                    kind.get() != "DECIMAL"
                                                })
                                                data-facet="unit"
                                            >
                                                label(
                                                    attrs: attributes! { for="element-unit" },
                                                    (unit_label.as_str())
                                                )
                                                <select
                                                    id="element-unit"
                                                    class=(SELECT)
                                                    name="unit"
                                                    @change=$(async |e: Event| {
                                                        status.set(saving.get());
                                                        let outcome = save_field(
                                                                pid.get(),
                                                                eid.get(),
                                                                "unit".to_owned(),
                                                                e.target.value,
                                                            )
                                                            .await;
                                                        if outcome.is_ok() {
                                                            status.set(outcome.unwrap());
                                                            revision.increment();
                                                        } else {
                                                            status.set(outcome.unwrap_err());
                                                        }
                                                    })
                                                >
                                                    <option value="" selected=(c.unit.is_none())>
                                                        (unit_none.as_str())
                                                    </option>
                                                    for unit in UNITS {
                                                        <option
                                                            value=(unit_name(*unit))
                                                            selected=(c.unit == Some(*unit))
                                                        >
                                                            (unit_name(*unit))
                                                        </option>
                                                    }
                                                </select>
                                            </div>
                                            <div
                                                class="flex flex-col gap-4"
                                                :hidden=$(kind.get() != "ATTACHMENT")
                                                data-facet="attachment"
                                            >
                                                <div class="flex flex-col gap-2">
                                                    label(
                                                        attrs: attributes! { for="element-accept" },
                                                        (accept_label.as_str())
                                                    )
                                                    <input
                                                        id="element-accept"
                                                        class=(INPUT)
                                                        type="text"
                                                        name="accept"
                                                        value=(c.accept.as_str())
                                                        autocomplete="off"
                                                        aria-describedby="element-accept-help"
                                                        @change=$(async |e: Event| {
                                                            status.set(saving.get());
                                                            let outcome = save_field(
                                                                    pid.get(),
                                                                    eid.get(),
                                                                    "accept".to_owned(),
                                                                    e.target.value,
                                                                )
                                                                .await;
                                                            if outcome.is_ok() {
                                                                status.set(outcome.unwrap());
                                                                revision.increment();
                                                            } else {
                                                                status.set(outcome.unwrap_err());
                                                            }
                                                        })
                                                    >
                                                    <p
                                                        id="element-accept-help"
                                                        class="text-sm text-muted-foreground"
                                                    >
                                                        (accept_help.as_str())
                                                    </p>
                                                </div>
                                                <div class="flex flex-col gap-2">
                                                    label(
                                                        attrs: attributes! { for="element-max-bytes" },
                                                        (max_bytes_label.as_str())
                                                    )
                                                    <input
                                                        id="element-max-bytes"
                                                        class=(INPUT)
                                                        type="number"
                                                        name="max_bytes"
                                                        min="1"
                                                        value=(c.max_bytes.as_deref().unwrap_or(""))
                                                        @change=$(async |e: Event| {
                                                            status.set(saving.get());
                                                            let outcome = save_field(
                                                                    pid.get(),
                                                                    eid.get(),
                                                                    "max_bytes".to_owned(),
                                                                    e.target.value,
                                                                )
                                                                .await;
                                                            if outcome.is_ok() {
                                                                status.set(outcome.unwrap());
                                                                revision.increment();
                                                            } else {
                                                                status.set(outcome.unwrap_err());
                                                            }
                                                        })
                                                    >
                                                </div>
                                            </div>
                                            <div class="flex flex-col gap-2">
                                                label(
                                                    attrs: attributes! { for="element-arity" },
                                                    (arity_label.as_str())
                                                )
                                                <select
                                                    id="element-arity"
                                                    class=(SELECT)
                                                    name="arity"
                                                    @change=$(async |e: Event| {
                                                        status.set(saving.get());
                                                        let outcome = save_field(
                                                                pid.get(),
                                                                eid.get(),
                                                                "arity".to_owned(),
                                                                e.target.value,
                                                            )
                                                            .await;
                                                        if outcome.is_ok() {
                                                            status.set(outcome.unwrap());
                                                            revision.increment();
                                                        } else {
                                                            status.set(outcome.unwrap_err());
                                                        }
                                                    })
                                                >
                                                    <option
                                                        value="ONE"
                                                        selected=(matches!(c.column.arity, Arity::One))
                                                    >
                                                        (arity_one.as_str())
                                                    </option>
                                                    <option
                                                        value="MANY"
                                                        selected=(matches!(c.column.arity, Arity::Many))
                                                    >
                                                        (arity_many.as_str())
                                                    </option>
                                                </select>
                                            </div>
                                            <p
                                                role="status"
                                                class="min-h-5 text-sm text-muted-foreground"
                                                data-save-status=""
                                            >
                                                $(status.get())
                                            </p>
                                        </div>
                                    )
                                    card_footer(
                                        button(
                                            attrs: attributes! { type="submit" },
                                            (save.as_str())
                                        )
                                    )
                                </form>
                            )
                            <div
                                class="mt-6"
                                :hidden=$(kind.get() != "ENUM")
                                data-facet="options"
                            >
                                card(
                                    card_header(
                                        <h3 class="leading-none font-semibold">
                                            (options_label.as_str())
                                        </h3>
                                    )
                                    card_content(
                                        <div class="flex flex-col gap-4">
                                            <p class="text-sm text-muted-foreground">
                                                (options_help.as_str())
                                            </p>
                                            if c.options.is_empty() {
                                                <p
                                                    class="text-sm text-muted-foreground"
                                                    data-options-empty=""
                                                >
                                                    (options_empty.as_str())
                                                </p>
                                            } else {
                                                <ul class="flex flex-col gap-2">
                                                    for (index, (option_id, option_text, remove_name)) in c.options.iter().enumerate() {
                                                        <li
                                                            class="flex items-center gap-2"
                                                            data-option-id=(option_id.as_str())
                                                        >
                                                            <form
                                                                method="post"
                                                                action=(option_update_href())
                                                                class="flex min-w-0 flex-1 items-center gap-2"
                                                            >
                                                                <input
                                                                    type="hidden"
                                                                    name="option_id"
                                                                    value=(option_id.as_str())
                                                                >
                                                                <label for=(format!("option-{option_id}")) class="sr-only">
                                                                    (format!("{option_label} {}", index + 1))
                                                                </label>
                                                                <input
                                                                    id=(format!("option-{option_id}"))
                                                                    class=(INPUT)
                                                                    type="text"
                                                                    name="label"
                                                                    value=(option_text.as_str())
                                                                    required=""
                                                                    autocomplete="off"
                                                                    @change=$(async |e: Event| {
                                                                        option_status.set(saving.get());
                                                                        let outcome = save_option(
                                                                                pid.get(),
                                                                                eid.get(),
                                                                                e.target.id,
                                                                                e.target.value,
                                                                            )
                                                                            .await;
                                                                        if outcome.is_ok() {
                                                                            option_status.set(outcome.unwrap());
                                                                            revision.increment();
                                                                        } else {
                                                                            option_status.set(outcome.unwrap_err());
                                                                        }
                                                                    })
                                                                >
                                                            </form>
                                                            <form method="post" action=(option_remove_href())>
                                                                <input
                                                                    type="hidden"
                                                                    name="option_id"
                                                                    value=(option_id.as_str())
                                                                >
                                                                button(
                                                                    variant: ButtonVariant::Ghost,
                                                                    size: ButtonSize::Icon,
                                                                    attrs: attributes! {
                                                                        type="submit"
                                                                        aria-label=(remove_name.as_str())
                                                                        class="text-destructive"
                                                                        data-option-remove=""
                                                                    },
                                                                    icon(
                                                                        data: iconify_icon!("feather:trash-2"),
                                                                        attrs: attributes! { class="size-4" }
                                                                    )
                                                                )
                                                            </form>
                                                        </li>
                                                    }
                                                </ul>
                                            }
                                            <p
                                                role="status"
                                                class="min-h-5 text-sm text-muted-foreground"
                                                data-option-status=""
                                            >
                                                $(option_status.get())
                                            </p>
                                            <form
                                                method="post"
                                                action=(option_add_href)
                                                class="flex items-end gap-2"
                                            >
                                                <div class="flex min-w-0 flex-1 flex-col gap-2">
                                                    label(
                                                        attrs: attributes! { for="element-option-new" },
                                                        (option_new_label.as_str())
                                                    )
                                                    <input
                                                        id="element-option-new"
                                                        class=(INPUT)
                                                        type="text"
                                                        name="label"
                                                        required=""
                                                        autocomplete="off"
                                                    >
                                                </div>
                                                button(
                                                    variant: ButtonVariant::Outline,
                                                    attrs: attributes! { type="submit" },
                                                    (options_add.as_str())
                                                )
                                            </form>
                                        </div>
                                    )
                                )
                            </div>
                        }
                        Detail::Nothing => add_form(
                            procedure_id: procedure_id,
                            organization_id: organization_id,
                            parent: None,
                            heading_level_top: true
                        ),
                    }
                </section>
            </div>
        </div>
    }
}

/// The vendored input's look, for the controls that carry runtime
/// handlers: a handler cannot travel through `attributes!` into a
/// component (its captures would not outlive the call), so these are
/// plain elements. Kept in step with `components::input::INPUT` and
/// `components::select::SELECT`.
const INPUT: StaticClass = class!(
    "h-9 w-full min-w-0 rounded-lg border border-border bg-background px-3 \
     text-sm shadow-xs transition-colors outline-none \
     placeholder:text-muted-foreground \
     focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 \
     focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-50",
);

const SELECT: StaticClass = class!(
    "h-9 w-full appearance-none items-center rounded-lg border border-border \
     bg-background pr-8 pl-3 text-left text-sm shadow-xs transition-colors outline-none \
     focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 \
     focus-visible:ring-offset-background disabled:pointer-events-none",
);

/// The structure panel: the empty notice, or the tree.
#[component]
async fn structure_panel(
    cx: &Cx,
    procedure: ProcedureRevisionDraft,
    selected: Option<String>,
) -> Result {
    let elements = procedure
        .revision_draft
        .as_ref()
        .map(|d| d.schema.elements.clone())
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
    move_to: String,
    top_level: String,
    remove: String,
    one: String,
    many: String,
}

impl TreeLabels {
    async fn load(cx: &Cx) -> Result<Self> {
        Ok(Self {
            actions: t(cx, "schema.actions").await?,
            move_up: t(cx, "schema.actions.move-up").await?,
            move_down: t(cx, "schema.actions.move-down").await?,
            move_to: t(cx, "schema.actions.move-to").await?,
            top_level: t(cx, "schema.actions.top-level").await?,
            remove: t(cx, "schema.actions.remove").await?,
            one: t(cx, "schema.arity.one").await?,
            many: t(cx, "schema.arity.many").await?,
        })
    }
}

struct Tree {
    elements: Vec<SchemaElement>,
    selected: Option<String>,
    organization_id: uuid::Uuid,
    procedure_id: uuid::Uuid,
    labels: TreeLabels,
}

impl Tree {
    fn children(&self, parent: Option<&str>) -> Vec<&SchemaElement> {
        self.elements
            .iter()
            .filter(|e| parent_of(e).as_deref() == parent)
            .collect()
    }

    /// Groups an element may move into: every group but itself and
    /// its descendants.
    fn destinations(&self, id: &str) -> Vec<&SchemaGroup> {
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
                SchemaElement::Group(g) if !excluded.contains(&g.id.inner().to_owned()) => Some(g),
                _ => None,
            })
            .collect()
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
#[component]
async fn tree_row(
    cx: &Cx,
    tree: &Tree,
    element: SchemaElement,
    first: bool,
    last: bool,
    depth: usize,
) -> Result {
    let id = id_of(&element).to_owned();
    let is_selected = tree.selected.as_deref() == Some(id.as_str());
    let text = label_of(&element).to_owned();
    let summary = element_summary(cx, &element).await?;
    let (multiplicity, is_group) = match &element {
        SchemaElement::Column(c) => (
            match c.arity {
                Arity::One => tree.labels.one.clone(),
                Arity::Many => tree.labels.many.clone(),
            },
            false,
        ),
        SchemaElement::Group(g) => (
            match g.cardinality {
                Cardinality::One => tree.labels.one.clone(),
                Cardinality::Many => tree.labels.many.clone(),
            },
            true,
        ),
        SchemaElement::Unknown => (String::new(), false),
    };
    let actions_name = t_args(cx, "schema.actions.for", &one_arg("label", text.clone())).await?;
    let select_query = [("selected", id.clone())];
    let select_href = href!(
        page,
        OrganizationId(tree.organization_id),
        ProcedureId(tree.procedure_id)
    )
    .query(&select_query);
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
    let at_root = parent_of(&element).is_none();
    view! {
        <li
            data-element-id=(id.as_str())
            data-element-kind=(if is_group { "group" } else { "column" })
        >
            <div
                class=(class!(
                    "flex items-center gap-2 rounded-lg border px-3 py-2",
                    "border-ring ring-2 ring-ring/40" if is_selected else "border-border",
                ))
            >
                <a
                    href=(select_href)
                    class="min-w-0 flex-1 truncate text-sm font-medium underline-offset-4 hover:underline"
                    aria-current=(is_selected.then_some("true"))
                >
                    (text.as_str())
                </a>
                badge(variant: BadgeVariant::Outline, (summary.as_str()))
                badge(variant: BadgeVariant::Secondary, (multiplicity.as_str()))
                dropdown_menu(
                    dropdown_menu_trigger(
                        attrs: attributes! {
                            aria-label=(actions_name.as_str())
                            class=(button_variants(
                                ButtonVariant::Ghost,
                                ButtonSize::Sm,
                            ))
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
                        dropdown_menu_sub(
                            dropdown_menu_sub_trigger((tree.labels.move_to.as_str()))
                            dropdown_menu_sub_content(
                                <form method="post" action=(relocate_href())>
                                    <input type="hidden" name="parent" value="">
                                    dropdown_menu_item(
                                        attrs: attributes! { type="submit" disabled=(at_root) },
                                        (tree.labels.top_level.as_str())
                                    )
                                </form>
                                for group in &destinations {
                                    <form method="post" action=(relocate_href())>
                                        <input
                                            type="hidden"
                                            name="parent"
                                            value=(group.id.inner())
                                        >
                                        dropdown_menu_item(
                                            attrs: attributes! {
                                                type="submit"
                                                disabled=(parent_of(&element).as_deref()
                                                    == Some(group.id.inner()))
                                            },
                                            (group.label.as_str())
                                        )
                                    </form>
                                }
                            )
                        )
                        dropdown_menu_separator()
                        <form method="post" action=(remove_href)>
                            dropdown_menu_item(
                                attrs: attributes! { type="submit" class="text-destructive" },
                                (tree.labels.remove.as_str())
                            )
                        </form>
                    )
                )
            </div>
            if is_group {
                tree_list(tree: tree, parent: Some(id.clone()), depth: depth + 1)
            }
        </li>
    }
}

/// A column's type in a word or two; a group's "group".
async fn element_summary(cx: &Cx, element: &SchemaElement) -> Result<String> {
    Ok(match element {
        SchemaElement::Group(_) => t(cx, "schema.kind.group").await?,
        SchemaElement::Column(c) => {
            let kind = t(cx, kind_message_id(&c.ty)).await?;
            match unit_of(&c.ty) {
                Some(unit) => format!("{kind} ({})", unit_name(unit)),
                None => kind,
            }
        }
        SchemaElement::Unknown => String::new(),
    })
}

/// The add form: what (column / group), the label, into `parent`.
#[component]
async fn add_form(
    cx: &Cx,
    procedure_id: uuid::Uuid,
    organization_id: uuid::Uuid,
    parent: Option<String>,
    heading_level_top: bool,
) -> Result {
    let heading = match &parent {
        Some(_) => t(cx, "schema.add.inside").await?,
        None => t(cx, "schema.add.title").await?,
    };
    let lead = t(cx, "schema.add.lead").await?;
    let what_label = t(cx, "schema.add.what").await?;
    let column_label = t(cx, "schema.kind.column").await?;
    let group_label = t(cx, "schema.kind.group").await?;
    let label_label = t(cx, "form.label").await?;
    let submit = t(cx, "schema.add.submit").await?;
    let prefix = match &parent {
        Some(id) => format!("add-{id}"),
        None => "add".to_owned(),
    };
    let what_id = format!("{prefix}-what");
    let label_id = format!("{prefix}-label");
    view! {
        card(
            card_header(
                if heading_level_top {
                    <h2 id="schema-detail-heading" class="leading-none font-semibold">
                        (heading)
                    </h2>
                } else {
                    <h3 class="leading-none font-semibold">(heading)</h3>
                }
            )
            <form
                method="post"
                action=(href!(add::submit, OrganizationId(organization_id), ProcedureId(procedure_id)))
                class="contents"
            >
                if let Some(parent) = &parent {
                    <input type="hidden" name="parent" value=(parent.as_str())>
                }
                card_content(
                    <div class="flex flex-col gap-4">
                        if parent.is_none() {
                            <p class="text-sm text-muted-foreground">(lead)</p>
                        }
                        <div class="flex flex-col gap-2">
                            label(
                                attrs: attributes! { for=(what_id.as_str()) },
                                (what_label)
                            )
                            select(
                                attrs: attributes! { id=(what_id.as_str()) name="what" },
                                <option value="column">(column_label)</option>
                                <option value="group">(group_label)</option>
                            )
                        </div>
                        field(
                            id: label_id.as_str(),
                            label: label_label,
                            attrs: attributes! { type="text" name="label" required="" autocomplete="off" }
                        )
                    </div>
                )
                card_footer(button(attrs: attributes! { type="submit" }, (submit)))
            </form>
        )
    }
}

// ---------------------------------------------------------------- element helpers

pub(super) fn id_of(element: &SchemaElement) -> &str {
    match element {
        SchemaElement::Column(c) => c.id.inner(),
        SchemaElement::Group(g) => g.id.inner(),
        SchemaElement::Unknown => "",
    }
}

pub(super) fn parent_of(element: &SchemaElement) -> Option<String> {
    match element {
        SchemaElement::Column(c) => c.parent_id.as_ref().map(|p| p.inner().to_owned()),
        SchemaElement::Group(g) => g.parent_id.as_ref().map(|p| p.inner().to_owned()),
        SchemaElement::Unknown => None,
    }
}

pub(super) fn label_of(element: &SchemaElement) -> &str {
    match element {
        SchemaElement::Column(c) => &c.label,
        SchemaElement::Group(g) => &g.label,
        SchemaElement::Unknown => "",
    }
}

/// The kinds the editor offers, as the API spells them.
pub(super) const KINDS: &[&str] = &[
    "TEXT",
    "BOOLEAN",
    "INTEGER",
    "DECIMAL",
    "DATE",
    "DATETIME",
    "ENUM",
    "ATTACHMENT",
    "GEOMETRY",
];

/// A number column's unit; `None` for any other column.
pub(super) fn unit_of(ty: &ColumnType) -> Option<Unit> {
    match ty {
        ColumnType::Integer(n) => n.unit,
        ColumnType::Decimal(n) => n.unit,
        _ => None,
    }
}

pub(super) fn kind_of(ty: &ColumnType) -> &'static str {
    match ty {
        ColumnType::Text(_) => "TEXT",
        ColumnType::Boolean(_) => "BOOLEAN",
        ColumnType::Integer(_) => "INTEGER",
        ColumnType::Decimal(_) => "DECIMAL",
        ColumnType::Date(_) => "DATE",
        ColumnType::Datetime(_) => "DATETIME",
        ColumnType::Enum(_) => "ENUM",
        ColumnType::Attachment(_) => "ATTACHMENT",
        ColumnType::Geometry(_) => "GEOMETRY",
        ColumnType::Unknown => "TEXT",
    }
}

fn kind_message_id(ty: &ColumnType) -> &'static str {
    kind_message_id_of(kind_of(ty))
}

fn kind_message_id_of(kind: &str) -> &'static str {
    match kind {
        "BOOLEAN" => "schema.kind.boolean",
        "INTEGER" => "schema.kind.integer",
        "DECIMAL" => "schema.kind.decimal",
        "DATE" => "schema.kind.date",
        "DATETIME" => "schema.kind.datetime",
        "ENUM" => "schema.kind.enum",
        "ATTACHMENT" => "schema.kind.attachment",
        "GEOMETRY" => "schema.kind.geometry",
        _ => "schema.kind.text",
    }
}

/// Every unit, in the API's order.
pub(super) const UNITS: &[Unit] = &[
    Unit::Millimetre,
    Unit::Centimetre,
    Unit::Metre,
    Unit::Kilometre,
    Unit::Gram,
    Unit::Kilogram,
    Unit::Tonne,
    Unit::Minute,
    Unit::Hour,
    Unit::Day,
    Unit::Week,
    Unit::Month,
    Unit::Year,
    Unit::SquareMetre,
    Unit::Hectare,
    Unit::SquareKilometre,
    Unit::Litre,
    Unit::CubicMetre,
    Unit::Percent,
];

/// The unit's symbol, also its form value (the kernel's spelling).
pub(super) fn unit_name(unit: Unit) -> &'static str {
    match unit {
        Unit::Millimetre => "mm",
        Unit::Centimetre => "cm",
        Unit::Metre => "m",
        Unit::Kilometre => "km",
        Unit::Gram => "g",
        Unit::Kilogram => "kg",
        Unit::Tonne => "t",
        Unit::Minute => "minute",
        Unit::Hour => "hour",
        Unit::Day => "day",
        Unit::Week => "week",
        Unit::Month => "month",
        Unit::Year => "year",
        Unit::SquareMetre => "m2",
        Unit::Hectare => "ha",
        Unit::SquareKilometre => "km2",
        Unit::Litre => "L",
        Unit::CubicMetre => "m3",
        Unit::Percent => "percent",
    }
}

pub(super) fn unit_from_name(name: &str) -> Option<Unit> {
    UNITS.iter().copied().find(|u| unit_name(*u) == name)
}
