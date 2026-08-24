//! **Autosave**: the server procedures the editor's detail form
//! calls from the browser, and the field components that call them.
//!
//! Every simple control saves itself on `change`: it sets the status
//! line to "Saving…", awaits its procedure, and shows either the
//! confirmation or the server's reason. A successful save bumps the
//! page's `revision` signal, which is what re-renders the structure
//! shard, so the tree shows the new label without a navigation.
//!
//! **Signals travel as props.** A `signal` declaration lowers to an
//! ordinary `&Signal<T>` binding, and a free identifier inside a
//! runtime expression is captured by reference to the *same* signal
//! id — so the page declares the five signals of [`Autosave`] once
//! and hands them down, and a handler written in a child component's
//! own `view!` drives the page's state. That is what lets the detail
//! panel be components at all rather than one enormous `view!`.
//!
//! **Procedures are public endpoints.** Their arguments are the
//! caller's to pick and the page's guard does not cover them, so each
//! authorizes through `client(cx)` exactly as a page does; the kernel
//! refuses edits the caller may not make (a widened audience, P.4)
//! and the refusal comes back as the status line's text.
//!
//! An `Err` from a procedure is invisible to the caller, so the
//! outcome rides as data: `Ok(Ok(text))` is the confirmation to show,
//! `Ok(Err(text))` the reason it was refused.

use cynic::QueryBuilder;
use platform_client::revision_draft::{
    Audience, ProcedureRevisionDraft, ProcedureRevisionDraftQuery, ProcedureRevisionDraftVariables,
};
use topcoat::{
    Result,
    context::Cx,
    runtime::{Event, Signal, procedure},
    view::{attributes, component, view},
};

use crate::{
    client,
    components::{button::button, card::card_footer, label::label},
    i18n::t,
};

use super::controls::{INPUT, SELECT, SWITCH_THUMB, SWITCH_TRACK, TEXTAREA};
use super::detail;
use super::edit::{self, Fields};
use super::element::id_of;

/// The procedure's draft by id, `None` when the id is not a uuid or
/// the schema answers `null` — both are the editor's conflict.
pub(in crate::pages) async fn draft_of(
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

/// What an autosave answers with: on success the detail card's
/// heading as it now reads (a rename changes it), on refusal the
/// reason to show.
///
/// **One value, and it has to be this one.** A procedure's answer is
/// hydrated by the browser runtime, which knows strings, numbers,
/// bools, `Option`, `Result`, signals and procedures — and nothing
/// else, so a tuple carrying several updates is not an option. The
/// confirmation text does not need the round trip (it is the same
/// string every time, and rides as a signal like "Saving…"), and the
/// draft-state line follows `revision` through
/// [`super::header::state`], a shard. That leaves the heading, which
/// is neither constant nor elsewhere.
///
/// It costs nothing to compute: every update mutation already
/// answers with the draft as it now stands.
type Saved = std::result::Result<String, String>;

/// The heading of `element_id` in the draft a mutation answered with.
async fn saved(cx: &Cx, procedure: &ProcedureRevisionDraft, element_id: &str) -> Result<Saved> {
    let element = procedure
        .revision_draft
        .as_ref()
        .and_then(|draft| draft.elements.iter().find(|e| id_of(e) == element_id));
    Ok(Ok(match element {
        Some(element) => detail::heading_for(cx, element).await?,
        None => String::new(),
    }))
}

/// Autosave of one field of the selected element (the runtime path
/// of [`elements::update`]): `Ok(Ok(text))` is the confirmation to
/// show, `Ok(Err(text))` the reason the save was refused — outcome as
/// data, since a procedure's `Err` is invisible to the caller.
/// Authorizes through the client like every page; `FORBIDDEN` and
/// the rest surface as the refused text.
#[procedure]
pub(in crate::pages) async fn save_field(
    cx: &Cx,
    procedure_id: String,
    element_id: String,
    field_name: String,
    value: String,
) -> Result<Saved> {
    let client = client(cx).await?;
    let Some(procedure) = draft_of(cx, &client, &procedure_id).await? else {
        return Ok(Err(t(cx, "schema.error.conflict").await?));
    };
    let fields = Fields::from_pairs(vec![(field_name, value)]);
    match edit::apply_update(cx, &client, &procedure, &element_id, &fields).await? {
        Ok(updated) => saved(cx, &updated, &element_id).await,
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
pub(in crate::pages) async fn save_text_format(
    cx: &Cx,
    procedure_id: String,
    element_id: String,
    format: String,
    pattern: String,
) -> Result<Saved> {
    let client = client(cx).await?;
    let Some(procedure) = draft_of(cx, &client, &procedure_id).await? else {
        return Ok(Err(t(cx, "schema.error.conflict").await?));
    };
    let fields = Fields::from_pairs(vec![
        ("format".to_owned(), format),
        ("pattern".to_owned(), pattern),
    ]);
    match edit::apply_update(cx, &client, &procedure, &element_id, &fields).await? {
        Ok(updated) => saved(cx, &updated, &element_id).await,
        Err(notice) => Ok(Err(notice.text)),
    }
}

/// Autosave of one enum option's label: `input_id` is the row's
/// input id (`option-<id>`), the one thing a handler can read off the
/// event besides the value. Same outcome shape as [`save_field`].
#[procedure]
pub(in crate::pages) async fn save_option(
    cx: &Cx,
    procedure_id: String,
    element_id: String,
    input_id: String,
    value: String,
) -> Result<Saved> {
    let client = client(cx).await?;
    let Some(option_id) = input_id.strip_prefix("option-") else {
        return Ok(Err(t(cx, "schema.error.conflict").await?));
    };
    let Some(procedure) = draft_of(cx, &client, &procedure_id).await? else {
        return Ok(Err(t(cx, "schema.error.conflict").await?));
    };
    match edit::rename_option(cx, &client, &procedure, &element_id, option_id, &value).await? {
        Ok(updated) => saved(cx, &updated, &element_id).await,
        Err(notice) => Ok(Err(notice.text)),
    }
}

// ------------------------------------------------------------- the wiring

/// Everything an autosaving control writes to, bundled so a field
/// component takes one prop instead of seven.
///
/// All references, so it is `Copy` and travels down a nesting of
/// components unchanged. `pid` and `eid` identify what is being
/// edited; `saving` and `saved_text` hold the localized "Saving…" and
/// "Saved your changes." texts, because a message id cannot be
/// formatted in the browser and neither string depends on the answer;
/// `revision` is the counter the structure shard and the draft-state
/// shard both watch. That leaves `status` (the line the outcome is
/// written to) and `heading` (the detail card's title, which a rename
/// changes and which the procedure therefore answers with).
#[derive(Clone, Copy)]
pub(in crate::pages) struct Autosave<'a> {
    pub(in crate::pages) pid: &'a Signal<String>,
    pub(in crate::pages) eid: &'a Signal<String>,
    pub(in crate::pages) saving: &'a Signal<String>,
    pub(in crate::pages) revision: &'a Signal<f64>,
    pub(in crate::pages) status: &'a Signal<String>,
    pub(in crate::pages) saved_text: &'a Signal<String>,
    pub(in crate::pages) heading: &'a Signal<String>,
}

/// The line every detail form reports its saves on.
#[component]
pub(in crate::pages) async fn status_line(save: Autosave<'_>) -> Result {
    let Autosave { status, .. } = save;
    view! {
        <p
            role="status"
            class="min-h-5 text-sm text-muted-foreground"
            data-save-status=""
        >
            $(status.get())
        </p>
    }
}

/// The submit button for the script-less path only: with the runtime
/// every field saves itself, but without it a `<select>` cannot
/// submit on its own, so the whole form needs a button.
#[component]
pub(in crate::pages) async fn noscript_save(text: String) -> Result {
    view! {
        <noscript>
            card_footer(button(attrs: attributes! { type="submit" }, (text.as_str())))
        </noscript>
    }
}

/// One control's identity and current value, bundled so every field
/// component below takes it as a single prop.
///
/// `name` is the field [`edit::apply_update`] applies — the same name
/// the whole-form POST would carry — and it is `&'static str` because
/// a runtime expression captures an *owned* value into a temporary
/// its generated JS then outlives: only borrowed captures (`&str`,
/// `&Signal<_>`) survive the expression.
pub(in crate::pages) struct Field {
    pub(in crate::pages) id: String,
    pub(in crate::pages) name: &'static str,
    pub(in crate::pages) label: String,
    pub(in crate::pages) value: String,
}

/// One autosaving text input, with optional help text beneath it.
#[component]
pub(in crate::pages) async fn text_field(
    field: Field,
    required: bool,
    help: Option<String>,
    save: Autosave<'_>,
) -> Result {
    let Field {
        id,
        name,
        label: label_text,
        value,
    } = field;
    let Autosave {
        pid,
        eid,
        saving,
        revision,
        status,
        saved_text,
        heading,
    } = save;
    let help_id = help.as_ref().map(|_| format!("{id}-help"));
    view! {
        <div class="flex flex-col gap-2">
            label(attrs: attributes! { for=(id.as_str()) }, (label_text.as_str()))
            <input
                id=(id.as_str())
                class=(INPUT)
                type="text"
                name=(name)
                value=(value.as_str())
                required=(required.then_some(""))
                autocomplete="off"
                aria-describedby=(help_id.as_deref())
                @change=$(async |e: Event| {
                    status.set(saving.get());
                    let outcome = save_field(
                            pid.get(),
                            eid.get(),
                            name.to_owned(),
                            e.target.value,
                        )
                        .await;
                    if outcome.is_ok() {
                        status.set(saved_text.get());
                        heading.set(outcome.unwrap());
                        revision.increment();
                    } else {
                        status.set(outcome.unwrap_err());
                    }
                })
            >

            if let Some(help) = &help {
                <p id=(help_id.as_deref()) class="text-sm text-muted-foreground">
                    (help.as_str())
                </p>
            }
        </div>
    }
}

/// One autosaving `<textarea>` (a note's body).
#[component]
pub(in crate::pages) async fn textarea_field(
    field: Field,
    required: bool,
    save: Autosave<'_>,
) -> Result {
    let Field {
        id,
        name,
        label: label_text,
        value,
    } = field;
    let Autosave {
        pid,
        eid,
        saving,
        revision,
        status,
        saved_text,
        heading,
    } = save;
    view! {
        <div class="flex flex-col gap-2">
            label(attrs: attributes! { for=(id.as_str()) }, (label_text.as_str()))
            <textarea
                id=(id.as_str())
                class=(TEXTAREA)
                name=(name)
                rows="4"
                required=(required.then_some(""))
                @change=$(async |e: Event| {
                    status.set(saving.get());
                    let outcome = save_field(
                            pid.get(),
                            eid.get(),
                            name.to_owned(),
                            e.target.value,
                        )
                        .await;
                    if outcome.is_ok() {
                        status.set(saved_text.get());
                        heading.set(outcome.unwrap());
                        revision.increment();
                    } else {
                        status.set(outcome.unwrap_err());
                    }
                })
            >
                (value.as_str())
            </textarea>
        </div>
    }
}

/// One autosaving whole-number input (an attachment's size cap).
#[component]
pub(in crate::pages) async fn number_field(field: Field, save: Autosave<'_>) -> Result {
    let Field {
        id,
        name,
        label: label_text,
        value,
    } = field;
    let Autosave {
        pid,
        eid,
        saving,
        revision,
        status,
        saved_text,
        heading,
    } = save;
    view! {
        <div class="flex flex-col gap-2">
            label(attrs: attributes! { for=(id.as_str()) }, (label_text.as_str()))
            <input
                id=(id.as_str())
                class=(INPUT)
                type="number"
                name=(name)
                min="1"
                value=(value.as_str())
                @change=$(async |e: Event| {
                    status.set(saving.get());
                    let outcome = save_field(
                            pid.get(),
                            eid.get(),
                            name.to_owned(),
                            e.target.value,
                        )
                        .await;
                    if outcome.is_ok() {
                        status.set(saved_text.get());
                        heading.set(outcome.unwrap());
                        revision.increment();
                    } else {
                        status.set(outcome.unwrap_err());
                    }
                })
            >
        </div>
    }
}

/// One option of a [`select_field`]: the value posted, the text
/// shown, and whether it is the current one.
pub(in crate::pages) struct Choice {
    pub(in crate::pages) value: &'static str,
    pub(in crate::pages) text: String,
    pub(in crate::pages) current: bool,
}

/// One autosaving `<select>` over a fixed list of choices —
/// cardinality, arity, unit, audience. A select whose choice also
/// drives what the form *shows* (the kind, the text format) carries
/// its own handler where that fact lives, since it has a signal to
/// mirror into as well.
#[component]
pub(in crate::pages) async fn select_field(
    field: Field,
    choices: Vec<Choice>,
    save: Autosave<'_>,
) -> Result {
    let Field {
        id,
        name,
        label: label_text,
        ..
    } = field;
    let Autosave {
        pid,
        eid,
        saving,
        revision,
        status,
        saved_text,
        heading,
    } = save;
    view! {
        <div class="flex flex-col gap-2">
            label(attrs: attributes! { for=(id.as_str()) }, (label_text.as_str()))
            <select
                id=(id.as_str())
                class=(SELECT)
                name=(name)
                @change=$(async |e: Event| {
                    status.set(saving.get());
                    let outcome = save_field(
                            pid.get(),
                            eid.get(),
                            name.to_owned(),
                            e.target.value,
                        )
                        .await;
                    if outcome.is_ok() {
                        status.set(saved_text.get());
                        heading.set(outcome.unwrap());
                        revision.increment();
                    } else {
                        status.set(outcome.unwrap_err());
                    }
                })
            >
                for choice in &choices {
                    <option value=(choice.value) selected=(choice.current)>
                        (choice.text.as_str())
                    </option>
                }
            </select>
        </div>
    }
}

/// The audience select, on all four kinds. Absent inside an
/// effectively reviewer-only container: everything there is
/// reviewer-only whatever this says (the kernel clamps on add and
/// refuses a widening on update, P.4), so offering the choice would
/// mislead — the caller decides by not rendering it.
#[component]
pub(in crate::pages) async fn audience_field(
    cx: &Cx,
    current: Audience,
    save: Autosave<'_>,
) -> Result {
    let choices = vec![
        Choice {
            value: "ALL",
            text: t(cx, "schema.audience.all").await?,
            current: matches!(current, Audience::All),
        },
        Choice {
            value: "REVIEWER",
            text: t(cx, "schema.audience.reviewer").await?,
            current: matches!(current, Audience::Reviewer),
        },
    ];
    let audience_label = t(cx, "schema.audience").await?;
    view! {
        select_field(
            field: Field {
                id: "element-audience".to_owned(),
                name: "audience",
                label: audience_label,
                value: String::new(),
            },
            choices: choices,
            save: save
        )
    }
}

/// The *required* toggle: a checkbox with `role="switch"`, preceded
/// by a hidden `false` so the script-less POST carries an unchecked
/// box (`Fields::last` reads the later value).
#[component]
pub(in crate::pages) async fn switch_field(
    field: Field,
    checked: bool,
    save: Autosave<'_>,
) -> Result {
    let Field {
        id,
        name,
        label: label_text,
        ..
    } = field;
    let Autosave {
        pid,
        eid,
        saving,
        revision,
        status,
        saved_text,
        heading,
    } = save;
    view! {
        <div
            class="flex items-center gap-2"
            data-required=(if checked { "true" } else { "false" })
        >
            <input type="hidden" name=(name) value="false">
            <span class="relative inline-flex shrink-0">
                <input
                    id=(id.as_str())
                    type="checkbox"
                    role="switch"
                    class=(SWITCH_TRACK)
                    name=(name)
                    value="true"
                    checked=(checked.then_some(""))
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
                                name.to_owned(),
                                value,
                            )
                            .await;
                        if outcome.is_ok() {
                            status.set(saved_text.get());
                            heading.set(outcome.unwrap());
                            revision.increment();
                        } else {
                            status.set(outcome.unwrap_err());
                        }
                    })
                >

                <span class=(SWITCH_THUMB)></span>
            </span>
            label(attrs: attributes! { for=(id.as_str()) }, (label_text.as_str()))
        </div>
    }
}
