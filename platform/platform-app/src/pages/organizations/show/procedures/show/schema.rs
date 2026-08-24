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
//! the new label without a reload. A text column's format is the one
//! edit two controls make together — the select and the pattern
//! input both call [`save_text_format`] with *both* values, since the
//! format is derived from the pair and neither half can be applied
//! against a column the other half never changed. Type-dependent fieldsets hide
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

pub(super) mod add_form;
pub(super) mod controls;
pub(super) mod edit;
pub(super) mod element;
pub(super) mod elements;
pub(super) mod preview;
pub(super) mod structure;

use cynic::{MutationBuilder, QueryBuilder};
use platform_client::revision_draft::{
    AddColumn, AddColumnInput, AddColumnVariables, AddGroup, AddGroupInput, AddGroupVariables,
    AddNote, AddNoteInput, AddNoteVariables, AddSection, AddSectionInput, AddSectionVariables,
    AttachmentType, Audience, Cardinality, Column, ColumnType, DiscardRevisionDraft,
    DiscardRevisionDraftInput, DiscardRevisionDraftVariables, Element, Group, Note, PlacementInput,
    ProcedureRevisionDraft, ProcedureRevisionDraftQuery, ProcedureRevisionDraftVariables, Section,
    Unit,
};
use platform_client::{Code, Error};
use serde::{Deserialize, Serialize};
use topcoat::{
    Result,
    context::Cx,
    icon::{icon, iconify::iconify_icon},
    router::{content::Form, error::not_found, href, page, path_param, query_params},
    runtime::{Event, procedure},
    view::{attributes, component, view},
};

use crate::{
    client,
    components::{
        alert::{AlertVariant, alert, alert_description},
        badge::{BadgeVariant, badge},
        button::{ButtonSize, ButtonVariant, button, button_variants},
        card::{card, card_content, card_footer, card_header},
        label::label,
        notice::{NoticeTone, notice as notice_box},
        page_title::page_title,
        tabs::{tabs, tabs_list, tabs_trigger},
    },
    flash,
    i18n::{t, t_args},
    pages::{args, one_arg, utc_date_arg},
};

use super::super::super::OrganizationId;
use super::{ProcedureId, counts, procedure_draft};
use add_form::AddFacts;
use controls::{INPUT, SELECT, SWITCH_THUMB, SWITCH_TRACK, TEXTAREA};
use edit::Fields;
use element::{
    KINDS, UNITS, effectively_reviewer, id_of, kind_message_id_of, kind_of, multiple_of, parent_of,
    text_format_of, unit_name, unit_of,
};
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

/// A confirmed action's notice naming what it acted on.
pub(super) async fn done_with(cx: &Cx, id: &str, subject: &str) -> Result<Notice> {
    Ok(Notice {
        kind: NoticeKind::Status,
        text: t_args(cx, id, &one_arg("label", subject.to_owned())).await?,
    })
}

/// An add submission: a column or a group, its label, where it goes.
#[derive(Deserialize)]
pub(super) struct Addition {
    what: String,
    label: String,
    /// The column's kind (`KINDS`); absent or unknown = text.
    #[serde(default)]
    kind: String,
    #[serde(default)]
    parent: String,
    #[serde(default)]
    before: String,
    /// `REVIEWER` narrows from the start; anything else is `ALL`
    /// (and the server clamps to the parent's effective audience).
    #[serde(default)]
    audience: String,
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
        let label_for_notice = new_label.clone();
        let parent = (!input.parent.is_empty()).then_some(input.parent.as_str());
        let before = (!input.before.is_empty()).then_some(input.before.as_str());
        let audience = (input.audience == "REVIEWER").then_some(Audience::Reviewer);
        if new_label.is_empty() {
            let notice = Notice {
                kind: NoticeKind::Alert,
                text: t(
                    cx,
                    match input.what.as_str() {
                        "section" => "schema.error.title-required",
                        "note" => "schema.error.body-required",
                        _ => "schema.error.label-required",
                    },
                )
                .await?,
            };
            return back_to_editor(cx, parent, Some(notice)).await;
        }
        let placement = Some(PlacementInput {
            parent_id: parent.map(cynic::Id::new),
            before_id: before.map(cynic::Id::new),
        });
        let procedure_id = cynic::Id::new(procedure.id.inner());
        let result = match input.what.as_str() {
            "group" => platform_client::run(
                &client,
                AddGroup::build(AddGroupVariables {
                    input: AddGroupInput {
                        procedure_id,
                        placement,
                        label: new_label,
                        cardinality: None,
                        audience,
                    },
                }),
            )
            .await
            .map(|r| r.add_group),
            "section" => platform_client::run(
                &client,
                AddSection::build(AddSectionVariables {
                    input: AddSectionInput {
                        procedure_id,
                        placement,
                        title: new_label,
                        help: None,
                        audience,
                    },
                }),
            )
            .await
            .map(|r| r.add_section),
            "note" => platform_client::run(
                &client,
                AddNote::build(AddNoteVariables {
                    input: AddNoteInput {
                        procedure_id,
                        placement,
                        title: None,
                        body: new_label,
                        audience,
                    },
                }),
            )
            .await
            .map(|r| r.add_note),
            _ => platform_client::run(
                &client,
                AddColumn::build(AddColumnVariables {
                    input: AddColumnInput {
                        procedure_id,
                        placement,
                        label: new_label,
                        ty: edit::kind_input(&input.kind),
                        required: None,
                        audience,
                    },
                }),
            )
            .await
            .map(|r| r.add_column),
        };
        match result {
            Ok(procedure) => {
                let elements = procedure
                    .revision_draft
                    .as_ref()
                    .map(|d| d.elements.as_slice())
                    .unwrap_or_default();
                let created = new_element_id(elements, parent, before);
                let notice = match input.what.as_str() {
                    "note" => done(cx, "schema.notice.note-added").await?,
                    what => {
                        done_with(
                            cx,
                            match what {
                                "group" => "schema.notice.group-added",
                                "section" => "schema.notice.section-added",
                                _ => "schema.notice.column-added",
                            },
                            &label_for_notice,
                        )
                        .await?
                    }
                };
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
    elements: &[Element],
    parent: Option<&str>,
    before: Option<&str>,
) -> Option<String> {
    let siblings: Vec<&Element> = elements
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

/// The procedure's draft by id, `None` when the id is not a uuid or
/// the schema answers `null` — both are the editor's conflict.
async fn draft_of(
    cx: &Cx,
    client: &platform_graphql::InProcess,
    procedure_id: &str,
) -> Result<Option<ProcedureRevisionDraft>> {
    let _ = cx;
    let Ok(id) = procedure_id.parse::<uuid::Uuid>() else {
        return Ok(None);
    };
    Ok(platform_client::run(
        client,
        ProcedureRevisionDraftQuery::build(ProcedureRevisionDraftVariables {
            id: cynic::Id::new(id.to_string()),
        }),
    )
    .await
    .ok()
    .and_then(|query| query.procedure))
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
    let Some(procedure) = draft_of(cx, &client, &procedure_id).await? else {
        return Ok(Err(t(cx, "schema.error.conflict").await?));
    };
    let fields = Fields::from_pairs(vec![(field_name, value)]);
    match edit::apply_update(cx, &client, &procedure, &element_id, &fields).await? {
        Ok(()) => Ok(Ok(t(cx, "schema.status.saved").await?)),
        Err(notice) => Ok(Err(notice.text)),
    }
}

/// Autosave of a text column's format constraint. The choice and
/// the custom pattern are **one** edit, never two: a column's format
/// is derived from both at once ([`elements::apply_update`] fills
/// what a submission omits from the *stored* column), so sending
/// them a field at a time cannot reach `REGEX` — picking it with no
/// stored pattern is refused, and the pattern that follows is then
/// read against a column whose format the refusal never changed.
/// Same outcome shape as [`save_field`].
#[procedure]
async fn save_text_format(
    cx: &Cx,
    procedure_id: String,
    element_id: String,
    format: String,
    pattern: String,
) -> Result<std::result::Result<String, String>> {
    let client = client(cx).await?;
    let Some(procedure) = draft_of(cx, &client, &procedure_id).await? else {
        return Ok(Err(t(cx, "schema.error.conflict").await?));
    };
    let fields = Fields::from_pairs(vec![
        ("format".to_owned(), format),
        ("pattern".to_owned(), pattern),
    ]);
    match edit::apply_update(cx, &client, &procedure, &element_id, &fields).await? {
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
    let Some(procedure) = draft_of(cx, &client, &procedure_id).await? else {
        return Ok(Err(t(cx, "schema.error.conflict").await?));
    };
    match edit::rename_option(cx, &client, &procedure, &element_id, option_id, &value).await? {
        Ok(()) => Ok(Ok(t(cx, "schema.status.saved").await?)),
        Err(notice) => Ok(Err(notice.text)),
    }
}

// ---------------------------------------------------------------- views

/// What the detail panel shows for the selected element, computed
/// before the view so the runtime handlers (which can only reach
/// signals and the event) have every value as a signal.
enum Detail {
    Column(ColumnDetail),
    Group(GroupDetail),
    Section(SectionDetail),
    Note(NoteDetail),
    Nothing,
}

struct ColumnDetail {
    column: Column,
    kind: String,
    /// The format select's value: "", EMAIL, PHONE, IBAN or REGEX.
    format: String,
    /// The custom pattern, when the format is REGEX.
    pattern: String,
    unit: Option<Unit>,
    /// `(id, label, accessible name of its remove button)`.
    options: Vec<(String, String, String)>,
    accept: String,
    max_bytes: Option<String>,
}

struct GroupDetail {
    group: Group,
}

struct SectionDetail {
    section: Section,
}

struct NoteDetail {
    note: Note,
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
    let discard_label = t(cx, "schema.discard").await?;
    let discard_question = t(cx, "schema.discard.question").await?;
    let discard_confirm = t(cx, "schema.discard.confirm").await?;
    let discard_keep = t(cx, "schema.discard.keep").await?;
    let structure_heading = t(cx, "schema.structure.title").await?;
    let add_element_label = t(cx, "schema.add.title").await?;
    let tab_editor = t(cx, "schema.tab.editor").await?;
    let tab_preview = t(cx, "schema.tab.preview").await?;
    let selected_element = selected
        .as_deref()
        .and_then(|id| elements.iter().find(|e| id_of(e) == id).cloned());
    let selected_id = selected_element.as_ref().map(|e| id_of(e).to_owned());
    // Inside an effectively reviewer-only container the audience
    // cannot change (wider is refused, narrower is moot — P.4
    // inheritance), so the detail form drops its select and the add
    // form its field.
    let audience_locked = selected_element
        .as_ref()
        .and_then(parent_of)
        .is_some_and(|parent| effectively_reviewer(&elements, &parent));
    let container_reviewer = selected_id
        .as_deref()
        .is_some_and(|id| effectively_reviewer(&elements, id));
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
    let preview_href = href!(
        preview::page,
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
    let audience_label = t(cx, "schema.audience").await?;
    let audience_all = t(cx, "schema.audience.all").await?;
    let audience_reviewer = t(cx, "schema.audience.reviewer").await?;
    let title_label = t(cx, "form.title").await?;
    let section_help_label = t(cx, "schema.section.help").await?;
    let note_body_label = t(cx, "schema.note.body").await?;
    let required_label = t(cx, "schema.required").await?;
    let format_label = t(cx, "schema.format").await?;
    let format_none = t(cx, "schema.format.none").await?;
    let format_email = t(cx, "schema.format.email").await?;
    let format_phone = t(cx, "schema.format.phone").await?;
    let format_iban = t(cx, "schema.format.iban").await?;
    let format_regex = t(cx, "schema.format.regex").await?;
    let pattern_label = t(cx, "schema.format.pattern").await?;
    let pattern_help = t(cx, "schema.format.pattern.help").await?;
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
        Some(Element::Column(column)) => Detail::Column(ColumnDetail {
            column: column.clone(),
            kind: kind_of(&column.ty).to_owned(),
            format: text_format_of(&column.ty).0.to_owned(),
            pattern: text_format_of(&column.ty).1.to_owned(),
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
        Some(Element::Group(group)) => Detail::Group(GroupDetail {
            group: group.clone(),
        }),
        Some(Element::Section(section)) => Detail::Section(SectionDetail {
            section: section.clone(),
        }),
        Some(Element::Note(note)) => Detail::Note(NoteDetail { note: note.clone() }),
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
        Detail::Section(s) => {
            t_args(
                cx,
                "schema.detail.section",
                &one_arg("label", s.section.title.clone()),
            )
            .await?
        }
        Detail::Note(_) => t(cx, "schema.detail.note").await?,
        Detail::Nothing => String::new(),
    };
    let initial_kind = match &detail {
        Detail::Column(c) => c.kind.clone(),
        _ => String::new(),
    };
    let initial_format = match &detail {
        Detail::Column(c) => c.format.clone(),
        _ => String::new(),
    };
    let initial_pattern = match &detail {
        Detail::Column(c) => c.pattern.clone(),
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
        signal format = initial_format.clone();
        signal pattern = initial_pattern.clone();
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
            tabs(
                tabs_list(
                    tabs_trigger(
                        active: true,
                        attrs: attributes! { href=(page_href()) },
                        (tab_editor)
                    )
                    tabs_trigger(
                        attrs: attributes! { href=(preview_href) },
                        (tab_preview)
                    )
                )
            )
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
            // The notice slot is always there, at one height, so the
            // panels below never move when a notice comes or goes.
            <div class="min-h-12" aria-live="polite" data-schema-notices="">
                if let Some(notice) = &notice {
                    notice_box(
                        tone: match notice.kind {
                            NoticeKind::Status => NoticeTone::Success,
                            NoticeKind::Alert => NoticeTone::Error,
                        },
                        // A confirmation fades once read (the `class`
                        // below, animated in app.css); a refusal stays
                        // until the next action. The comment sits outside
                        // the `attributes!` block on purpose: `topcoat fmt`
                        // 0.6.2 re-emits any comment written *inside* one
                        // into the enclosing call's children, growing the
                        // file by a copy on every run.
                        attrs: attributes! {
                            role=(match notice.kind {
                                NoticeKind::Status => "status",
                                NoticeKind::Alert => "alert",
                            })
                            class=(match notice.kind {
                                NoticeKind::Status => {
                                    "motion-safe:animate-[varve-notice-fade_0.8s_ease-in_6s_forwards]"
                                }
                                NoticeKind::Alert => "",
                            })
                            data-schema-notice=""
                        },
                        (notice.text.as_str())
                    )
                }
            </div>
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
                    structure::panel(
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
                                            if !audience_locked {
                                                <div class="flex flex-col gap-2">
                                                    label(
                                                        attrs: attributes! { for="element-audience" },
                                                        (audience_label.as_str())
                                                    )
                                                    <select
                                                        id="element-audience"
                                                        class=(SELECT)
                                                        name="audience"
                                                        @change=$(async |e: Event| {
                                                            status.set(saving.get());
                                                            let outcome = save_field(
                                                                    pid.get(),
                                                                    eid.get(),
                                                                    "audience".to_owned(),
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
                                                            value="ALL"
                                                            selected=(matches!(g.group.audience, Audience::All))
                                                        >
                                                            (audience_all.as_str())
                                                        </option>
                                                        <option
                                                            value="REVIEWER"
                                                            selected=(matches!(g.group.audience, Audience::Reviewer))
                                                        >
                                                            (audience_reviewer.as_str())
                                                        </option>
                                                    </select>
                                                </div>
                                            }
                                            <p
                                                role="status"
                                                class="min-h-5 text-sm text-muted-foreground"
                                                data-save-status=""
                                            >
                                                $(status.get())
                                            </p>
                                        </div>
                                    )
                                    // Every field autosaves with the runtime; the
                                    // button exists for the script-less path only,
                                    // where a `<select>` cannot submit on its own.
                                    <noscript>
                                        card_footer(
                                            button(
                                                attrs: attributes! { type="submit" },
                                                (save.as_str())
                                            )
                                        )
                                    </noscript>
                                </form>
                            )
                            <div class="mt-6">
                                add_form::form(
                                    procedure_id: procedure_id,
                                    organization_id: organization_id,
                                    facts: AddFacts {
                                        parent: Some(selected_string.clone()),
                                        section_parent: false,
                                        audience_offered: !container_reviewer,
                                        heading_level_top: false,
                                    }
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
                                            <div
                                                class="flex flex-col gap-2"
                                                :hidden=$(if kind.get() == "ENUM" {
                                                    false
                                                } else if kind.get() == "ATTACHMENT" {
                                                    false
                                                } else {
                                                    kind.get() != "GEOMETRY"
                                                })
                                                data-facet="arity"
                                            >
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
                                                    <option value="ONE" selected=(!multiple_of(&c.column.ty))>
                                                        (arity_one.as_str())
                                                    </option>
                                                    <option value="MANY" selected=(multiple_of(&c.column.ty))>
                                                        (arity_many.as_str())
                                                    </option>
                                                </select>
                                            </div>
                                            <div
                                                class="flex flex-col gap-4"
                                                :hidden=$(kind.get() != "TEXT")
                                                data-facet="format"
                                            >
                                                <div class="flex flex-col gap-2">
                                                    label(
                                                        attrs: attributes! { for="element-format" },
                                                        (format_label.as_str())
                                                    )
                                                    <select
                                                        id="element-format"
                                                        class=(SELECT)
                                                        name="format"
                                                        @change=$(async |e: Event| {
                                                            format.set(e.target.value.to_owned());
                                                            status.set(saving.get());
                                                            let outcome = save_text_format(
                                                                    pid.get(),
                                                                    eid.get(),
                                                                    e.target.value,
                                                                    pattern.get(),
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
                                                        <option value="" selected=(c.format.is_empty())>
                                                            (format_none.as_str())
                                                        </option>
                                                        <option value="EMAIL" selected=(c.format == "EMAIL")>
                                                            (format_email.as_str())
                                                        </option>
                                                        <option value="PHONE" selected=(c.format == "PHONE")>
                                                            (format_phone.as_str())
                                                        </option>
                                                        <option value="IBAN" selected=(c.format == "IBAN")>
                                                            (format_iban.as_str())
                                                        </option>
                                                        <option value="REGEX" selected=(c.format == "REGEX")>
                                                            (format_regex.as_str())
                                                        </option>
                                                    </select>
                                                </div>
                                                <div
                                                    class="flex flex-col gap-2"
                                                    :hidden=$(format.get() != "REGEX")
                                                    data-facet="pattern"
                                                >
                                                    label(
                                                        attrs: attributes! { for="element-pattern" },
                                                        (pattern_label.as_str())
                                                    )
                                                    <input
                                                        id="element-pattern"
                                                        class=(INPUT)
                                                        type="text"
                                                        name="pattern"
                                                        value=(c.pattern.as_str())
                                                        autocomplete="off"
                                                        aria-describedby="element-pattern-help"
                                                        @change=$(async |e: Event| {
                                                            pattern.set(e.target.value.to_owned());
                                                            status.set(saving.get());
                                                            let outcome = save_text_format(
                                                                    pid.get(),
                                                                    eid.get(),
                                                                    format.get(),
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
                                                        id="element-pattern-help"
                                                        class="text-sm text-muted-foreground"
                                                    >
                                                        (pattern_help.as_str())
                                                    </p>
                                                </div>
                                            </div>
                                            <div
                                                class="flex items-center gap-2"
                                                data-required=(if c.column.required {
                                                    "true"
                                                } else {
                                                    "false"
                                                })
                                            >
                                                <input type="hidden" name="required" value="false">
                                                <span class="relative inline-flex shrink-0">
                                                    <input
                                                        id="element-required"
                                                        type="checkbox"
                                                        role="switch"
                                                        class=(SWITCH_TRACK)
                                                        name="required"
                                                        value="true"
                                                        checked=(c.column.required.then_some(""))
                                                        @change=$(async |e: Event| {
                                                            status.set(saving.get());
                                                            let value = if e.target.checked {
                                                                "true".to_owned()
                                                            } else {
                                                                "false".to_owned()
                                                            };
                                                            let outcome = save_field(
                                                                    pid.get(),
                                                                    eid.get(),
                                                                    "required".to_owned(),
                                                                    value,
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
                                                    <span class=(SWITCH_THUMB)></span>
                                                </span>
                                                label(
                                                    attrs: attributes! { for="element-required" },
                                                    (required_label.as_str())
                                                )
                                            </div>
                                            if !audience_locked {
                                                <div class="flex flex-col gap-2">
                                                    label(
                                                        attrs: attributes! { for="element-audience" },
                                                        (audience_label.as_str())
                                                    )
                                                    <select
                                                        id="element-audience"
                                                        class=(SELECT)
                                                        name="audience"
                                                        @change=$(async |e: Event| {
                                                            status.set(saving.get());
                                                            let outcome = save_field(
                                                                    pid.get(),
                                                                    eid.get(),
                                                                    "audience".to_owned(),
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
                                                            value="ALL"
                                                            selected=(matches!(c.column.audience, Audience::All))
                                                        >
                                                            (audience_all.as_str())
                                                        </option>
                                                        <option
                                                            value="REVIEWER"
                                                            selected=(matches!(c.column.audience, Audience::Reviewer))
                                                        >
                                                            (audience_reviewer.as_str())
                                                        </option>
                                                    </select>
                                                </div>
                                            }
                                            <p
                                                role="status"
                                                class="min-h-5 text-sm text-muted-foreground"
                                                data-save-status=""
                                            >
                                                $(status.get())
                                            </p>
                                        </div>
                                    )
                                    // Every field autosaves with the runtime; the
                                    // button exists for the script-less path only,
                                    // where a `<select>` cannot submit on its own.
                                    <noscript>
                                        card_footer(
                                            button(
                                                attrs: attributes! { type="submit" },
                                                (save.as_str())
                                            )
                                        )
                                    </noscript>
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
                        Detail::Section(s) => {
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
                                                    attrs: attributes! { for="element-title" },
                                                    (title_label.as_str())
                                                )
                                                <input
                                                    id="element-title"
                                                    class=(INPUT)
                                                    type="text"
                                                    name="title"
                                                    value=(s.section.title.as_str())
                                                    required=""
                                                    autocomplete="off"
                                                    @change=$(async |e: Event| {
                                                        status.set(saving.get());
                                                        let outcome = save_field(
                                                                pid.get(),
                                                                eid.get(),
                                                                "title".to_owned(),
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
                                                    attrs: attributes! { for="element-help" },
                                                    (section_help_label.as_str())
                                                )
                                                <input
                                                    id="element-help"
                                                    class=(INPUT)
                                                    type="text"
                                                    name="help"
                                                    value=(s.section.help.as_deref().unwrap_or(""))
                                                    autocomplete="off"
                                                    @change=$(async |e: Event| {
                                                        status.set(saving.get());
                                                        let outcome = save_field(
                                                                pid.get(),
                                                                eid.get(),
                                                                "help".to_owned(),
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
                                            if !audience_locked {
                                                <div class="flex flex-col gap-2">
                                                    label(
                                                        attrs: attributes! { for="element-audience" },
                                                        (audience_label.as_str())
                                                    )
                                                    <select
                                                        id="element-audience"
                                                        class=(SELECT)
                                                        name="audience"
                                                        @change=$(async |e: Event| {
                                                            status.set(saving.get());
                                                            let outcome = save_field(
                                                                    pid.get(),
                                                                    eid.get(),
                                                                    "audience".to_owned(),
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
                                                            value="ALL"
                                                            selected=(matches!(s.section.audience, Audience::All))
                                                        >
                                                            (audience_all.as_str())
                                                        </option>
                                                        <option
                                                            value="REVIEWER"
                                                            selected=(matches!(s.section.audience, Audience::Reviewer))
                                                        >
                                                            (audience_reviewer.as_str())
                                                        </option>
                                                    </select>
                                                </div>
                                            }
                                            <p
                                                role="status"
                                                class="min-h-5 text-sm text-muted-foreground"
                                                data-save-status=""
                                            >
                                                $(status.get())
                                            </p>
                                        </div>
                                    )
                                    <noscript>
                                        card_footer(
                                            button(
                                                attrs: attributes! { type="submit" },
                                                (save.as_str())
                                            )
                                        )
                                    </noscript>
                                </form>
                            )
                            <div class="mt-6">
                                add_form::form(
                                    procedure_id: procedure_id,
                                    organization_id: organization_id,
                                    facts: AddFacts {
                                        parent: Some(selected_string.clone()),
                                        section_parent: true,
                                        audience_offered: !container_reviewer,
                                        heading_level_top: false,
                                    }
                                )
                            </div>
                        }
                        Detail::Note(n) => {
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
                                                    attrs: attributes! { for="element-title" },
                                                    (title_label.as_str())
                                                )
                                                <input
                                                    id="element-title"
                                                    class=(INPUT)
                                                    type="text"
                                                    name="title"
                                                    value=(n.note.title.as_deref().unwrap_or(""))
                                                    autocomplete="off"
                                                    @change=$(async |e: Event| {
                                                        status.set(saving.get());
                                                        let outcome = save_field(
                                                                pid.get(),
                                                                eid.get(),
                                                                "title".to_owned(),
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
                                                    attrs: attributes! { for="element-body" },
                                                    (note_body_label.as_str())
                                                )
                                                <textarea
                                                    id="element-body"
                                                    class=(TEXTAREA)
                                                    name="body"
                                                    rows="4"
                                                    required=""
                                                    @change=$(async |e: Event| {
                                                        status.set(saving.get());
                                                        let outcome = save_field(
                                                                pid.get(),
                                                                eid.get(),
                                                                "body".to_owned(),
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
                                                    (n.note.body.as_str())
                                                </textarea>
                                            </div>
                                            if !audience_locked {
                                                <div class="flex flex-col gap-2">
                                                    label(
                                                        attrs: attributes! { for="element-audience" },
                                                        (audience_label.as_str())
                                                    )
                                                    <select
                                                        id="element-audience"
                                                        class=(SELECT)
                                                        name="audience"
                                                        @change=$(async |e: Event| {
                                                            status.set(saving.get());
                                                            let outcome = save_field(
                                                                    pid.get(),
                                                                    eid.get(),
                                                                    "audience".to_owned(),
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
                                                            value="ALL"
                                                            selected=(matches!(n.note.audience, Audience::All))
                                                        >
                                                            (audience_all.as_str())
                                                        </option>
                                                        <option
                                                            value="REVIEWER"
                                                            selected=(matches!(n.note.audience, Audience::Reviewer))
                                                        >
                                                            (audience_reviewer.as_str())
                                                        </option>
                                                    </select>
                                                </div>
                                            }
                                            <p
                                                role="status"
                                                class="min-h-5 text-sm text-muted-foreground"
                                                data-save-status=""
                                            >
                                                $(status.get())
                                            </p>
                                        </div>
                                    )
                                    <noscript>
                                        card_footer(
                                            button(
                                                attrs: attributes! { type="submit" },
                                                (save.as_str())
                                            )
                                        )
                                    </noscript>
                                </form>
                            )
                        }
                        Detail::Nothing => add_form::form(
                            procedure_id: procedure_id,
                            organization_id: organization_id,
                            facts: AddFacts {
                                parent: None,
                                section_parent: false,
                                audience_offered: true,
                                heading_level_top: true,
                            }
                        ),
                    }
                </section>
            </div>
        </div>
    }
}
