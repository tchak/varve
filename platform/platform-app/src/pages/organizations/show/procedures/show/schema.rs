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
//! [`elements::element::remove`]), and discarding the draft
//! ([`discard::submit`], behind a confirmation state). One POST per
//! module segment is what `module_router!` offers, hence one inline
//! module per action. Post → 303 → get, a one-shot notice on the
//! landing page.
//!
//! **Then the runtime, as an enhancement.** With the browser script
//! (pages.rs: linked when the asset bundle is loaded — never in
//! router-level tests, so everything above is also proven without
//! it), the detail form autosaves on change and the structure panel
//! re-renders itself. Both are the browser's half of the same page,
//! and both are also reachable without it.
//!
//! # What lives where
//!
//! This module is the page and the routes that are not about one
//! element: the layout, the discard confirmation, the tab rail, and
//! the `add` and `discard` POSTs. The rest is one module per part:
//!
//! - [`structure`] — the tree panel and its `#[shard]`.
//! - [`detail`] — the selected element's form, one component per
//!   kind, and the state those forms edit.
//! - [`autosave`] — the `#[procedure]`s the browser calls and the
//!   field components that call them.
//! - [`add_form`] — the add form, which appears in two places.
//! - [`elements`] — the POST routes on one element.
//! - [`edit`] — what a submission *means* for an element, the one
//!   door both the routes and the procedures go through.
//! - [`element`] — the total helpers for reading a client `Element`.
//! - [`controls`] — the vendored controls' look, for the plain
//!   elements that carry runtime handlers.
//! - [`preview`] — the read-only second tab.
//!
//! **Signals are the seam.** A `signal` declaration lowers to an
//! ordinary `&Signal<T>` binding and a runtime expression captures it
//! by reference, so a signal declared in one `view!` can be passed
//! down as a prop and driven by a handler written in a child
//! component's own `view!`. That is why the detail panel can be
//! components at all: this page declares only `revision` — the
//! counter its structure shard watches — and hands it to
//! [`detail::panel`], which declares everything else it needs.
//!
//! **No drag and drop**: moving is *up*, *down*, *top*, *bottom*,
//! *move after* a sibling and *move to* a container — named actions a
//! keyboard and a screen reader reach, and that the API's
//! sibling-anchored placement (G.7) expresses directly.

pub(super) mod add_form;
pub(super) mod autosave;
pub(super) mod controls;
pub(super) mod detail;
pub(super) mod edit;
pub(super) mod element;
pub(super) mod elements;
pub(super) mod header;
pub(super) mod preview;
pub(super) mod structure;

use cynic::MutationBuilder;
use platform_client::revision_draft::{
    AddColumn, AddColumnInput, AddColumnVariables, AddGroup, AddGroupInput, AddGroupVariables,
    AddNote, AddNoteInput, AddNoteVariables, AddSection, AddSectionInput, AddSectionVariables,
    Audience, DiscardRevisionDraft, DiscardRevisionDraftInput, DiscardRevisionDraftVariables,
    Element, PlacementInput, ProcedureRevisionDraft,
};
use platform_client::{Code, Error};
use serde::{Deserialize, Serialize};
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::not_found, href, page, path_param, query_params},
    view::{attributes, component, view},
};

use crate::{
    client,
    components::{
        alert::{AlertVariant, alert, alert_description},
        button::{ButtonSize, ButtonVariant, button, button_variants},
        notice::{NoticeTone, notice as notice_box},
    },
    flash,
    i18n::{t, t_args},
    pages::one_arg,
};

use super::super::super::OrganizationId;
use super::{ProcedureId, procedure_draft};
use add_form::AddFacts;
use element::{effectively_reviewer, id_of, parent_of};

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

// ---------------------------------------------------------------- views

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
    let elements = procedure
        .revision_draft
        .as_ref()
        .map(|d| d.elements.clone())
        .unwrap_or_default();
    let discard_question = t(cx, "schema.discard.question").await?;
    let discard_confirm = t(cx, "schema.discard.confirm").await?;
    let discard_keep = t(cx, "schema.discard.keep").await?;
    let structure_heading = t(cx, "schema.structure.title").await?;
    let add_element_label = t(cx, "schema.add.title").await?;
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
    let procedure_id_string = procedure_id.to_string();
    let selected_string = selected_id.clone().unwrap_or_default();

    view! {
        // The one signal the page itself reads: the structure shard
        // below is re-rendered when an autosave bumps it. Everything
        // else the detail panel needs it declares itself.
        signal revision = 0.0;
        signal pid = procedure_id_string.clone();
        signal eid = selected_string.clone();

        <div class="flex flex-col gap-6">
            header::header(
                procedure: procedure.clone(),
                tab: header::Tab::Editor,
                offer_discard: !confirm_discard,
                revision: revision
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
                    if let Some(element) = &selected_element {
                        detail::panel(
                            editing: detail::Editing {
                                at: detail::At {
                                    organization_id,
                                    procedure_id,
                                },
                                element: element.clone(),
                            },
                            offers: detail::Offers {
                                audience: !audience_locked,
                                audience_below: !container_reviewer,
                            },
                            revision: revision
                        )
                    } else {
                        add_form::form(
                            procedure_id: procedure_id,
                            organization_id: organization_id,
                            facts: AddFacts {
                                parent: None,
                                section_parent: false,
                                audience_offered: true,
                                heading_level_top: true,
                            }
                        )
                    }
                </section>
            </div>
        </div>
    }
}
