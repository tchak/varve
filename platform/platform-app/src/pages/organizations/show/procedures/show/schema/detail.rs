//! The **detail panel**: the form for whichever element `?selected`
//! names — one component per kind (column, group, section, note),
//! each built from the shared autosaving controls in
//! [`super::autosave`].
//!
//! **The panel owns the editing state.** Every signal the form needs
//! is declared here in [`panel`]'s own `view!` — the status line, the
//! chosen kind and text format that decide which fieldsets show, the
//! ids the procedures are called with — and handed down as props.
//! Only `revision` comes from the page, because the page's structure
//! shard is what reads it.
//!
//! **Two kinds of control.** Most fields just save themselves, and
//! are the generic components from [`super::autosave`]. Two carry
//! their own handler because saving is not all they do: the *type*
//! select mirrors into a signal the facets are bound to, and the
//! *format* select saves through [`super::autosave::save_text_format`]
//! with the pattern beside it — a text column's format is derived
//! from the pair, so neither half can be sent alone.
//!
//! Below a selected **container** (a group or a section) the panel
//! also shows the add form pointed into it; below a column it shows
//! the enum options card when the kind is a choice.

use platform_client::revision_draft::{
    AttachmentType, Cardinality, Column, ColumnType, Element, Group, Note, Section,
};
use topcoat::{
    Result,
    context::Cx,
    icon::{icon, iconify::iconify_icon},
    router::href,
    runtime::{Event, Signal},
    view::{attributes, component, view},
};

use crate::{
    components::{
        button::{ButtonSize, ButtonVariant, button},
        card::{card, card_content, card_header},
        label::label,
    },
    i18n::{t, t_args},
    pages::one_arg,
};

use super::super::super::super::OrganizationId;
use super::super::ProcedureId;
use super::autosave::{
    Autosave, Choice, Field, audience_field, noscript_save, number_field, save_field, save_option,
    save_text_format, select_field, status_line, switch_field, text_field, textarea_field,
};
use super::controls::{INPUT, SELECT};
use super::element::{
    KINDS, UNITS, kind_message_id_of, kind_of, multiple_of, text_format_of, unit_name, unit_of,
};
use super::elements::element::ElementId;
use super::{add_form, elements};

/// The procedure whose draft is being edited: what every form in
/// the panel needs to build its action, and nothing more.
#[derive(Clone, Copy)]
pub(in crate::pages) struct At {
    pub(in crate::pages) organization_id: uuid::Uuid,
    pub(in crate::pages) procedure_id: uuid::Uuid,
}

/// What the panel edits and where its forms post.
pub(in crate::pages) struct Editing {
    pub(in crate::pages) at: At,
    pub(in crate::pages) element: Element,
}

/// The two audience choices the panel may or may not offer, both
/// decided by P.4 inheritance and both computed from the tree by the
/// page (this module sees the answers, not the tree).
pub(in crate::pages) struct Offers {
    /// The element's own audience select. Absent when an ancestor is
    /// reviewer-only: wider is refused by the kernel and narrower is
    /// moot, so the control would only mislead.
    pub(in crate::pages) audience: bool,
    /// The nested add form's audience field. Absent when the selected
    /// container is itself effectively reviewer-only — everything
    /// added inside it is reviewer-only whatever the field says.
    pub(in crate::pages) audience_below: bool,
}

/// The signals that decide which of a column's fieldsets show, and
/// carry the halves of a paired edit.
#[derive(Clone, Copy)]
struct Facets<'a> {
    /// The chosen type: the unit, arity, attachment and format
    /// fieldsets are bound to it.
    kind: &'a Signal<String>,
    /// The chosen text format: the pattern field is bound to it, and
    /// the pattern's save sends it.
    format: &'a Signal<String>,
    /// The current pattern, so the format select's save can send it.
    pattern: &'a Signal<String>,
    /// The options card's own status line, separate from the form's.
    option_status: &'a Signal<String>,
}

/// The panel: the selected element's form, and what belongs under it.
/// Declares the editing signals and delegates to the component for
/// the element's kind.
///
/// Boxed: the page's `view!` already nests deeply, and the four kind
/// components below it each nest a dozen more, so leaving this future
/// unboxed overflows the compiler's type-depth limit (the same reason
/// `structure::tree_row` is boxed).
#[component(boxed)]
pub(in crate::pages) async fn panel(
    cx: &Cx,
    editing: Editing,
    offers: Offers,
    revision: &Signal<f64>,
) -> Result {
    let Editing { at, element } = editing;
    let element_id = super::element::id_of(&element).to_owned();
    let saving_text = t(cx, "schema.status.saving").await?;
    let saved_text_value = t(cx, "schema.status.saved").await?;
    let initial_heading = heading_for(cx, &element).await?;
    let procedure_id_string = at.procedure_id.to_string();
    let (initial_kind, initial_format, initial_pattern) = match &element {
        Element::Column(column) => {
            let (format, pattern) = text_format_of(&column.ty);
            (
                kind_of(&column.ty).to_owned(),
                format.to_owned(),
                pattern.to_owned(),
            )
        }
        _ => (String::new(), String::new(), String::new()),
    };
    view! {
        signal status = String::new();
        signal heading = initial_heading.clone();
        signal option_status = String::new();
        signal kind = initial_kind.clone();
        signal format = initial_format.clone();
        signal pattern = initial_pattern.clone();
        signal pid = procedure_id_string.clone();
        signal eid = element_id.clone();
        signal saving = saving_text.clone();
        signal saved_text = saved_text_value.clone();

        match &element {
            Element::Column(column) => {
                column_detail(
                    at: at,
                    column: column.clone(),
                    offers_audience: offers.audience,
                    save: Autosave {
                        pid,
                        eid,
                        saving,
                        revision,
                        status,
                        saved_text,
                        heading,
                    },
                    facets: Facets {
                        kind,
                        format,
                        pattern,
                        option_status,
                    }
                )
            }
            Element::Group(group) => {
                group_detail(
                    at: at,
                    group: group.clone(),
                    offers: Offers {
                        audience: offers.audience,
                        audience_below: offers.audience_below,
                    },
                    save: Autosave {
                        pid,
                        eid,
                        saving,
                        revision,
                        status,
                        saved_text,
                        heading,
                    }
                )
            }
            Element::Section(section) => {
                section_detail(
                    at: at,
                    section: section.clone(),
                    offers: Offers {
                        audience: offers.audience,
                        audience_below: offers.audience_below,
                    },
                    save: Autosave {
                        pid,
                        eid,
                        saving,
                        revision,
                        status,
                        saved_text,
                        heading,
                    }
                )
            }
            Element::Note(note) => {
                note_detail(
                    at: at,
                    note: note.clone(),
                    offers_audience: offers.audience,
                    save: Autosave {
                        pid,
                        eid,
                        saving,
                        revision,
                        status,
                        saved_text,
                        heading,
                    }
                )
            }
            Element::Unknown => {
                ""
            }
        }
    }
}

/// The heading the selected element's card carries: which kind it is
/// and what it is called (the `<section>` around the panel is
/// labelled by it). Renaming the element changes it, so the autosave
/// formats it again with [`heading_for`] and writes it back into the
/// signal this reads.
#[component]
async fn detail_heading(heading: &Signal<String>) -> Result {
    view! {
        <h2 id="schema-detail-heading" class="leading-none font-semibold">
            $(heading.get())
        </h2>
    }
}

/// An element's detail heading. Shared with [`super::autosave`],
/// which formats it again after every save.
pub(in crate::pages) async fn heading_for(cx: &Cx, element: &Element) -> Result<String> {
    Ok(match element {
        Element::Column(column) => {
            t_args(
                cx,
                "schema.detail.column",
                &one_arg("label", column.label.clone()),
            )
            .await?
        }
        Element::Group(group) => {
            t_args(
                cx,
                "schema.detail.group",
                &one_arg("label", group.label.clone()),
            )
            .await?
        }
        Element::Section(section) => {
            t_args(
                cx,
                "schema.detail.section",
                &one_arg("label", section.title.clone()),
            )
            .await?
        }
        Element::Note(_) => t(cx, "schema.detail.note").await?,
        Element::Unknown => String::new(),
    })
}

/// A group: its label, how many rows it holds, and its audience.
#[component]
async fn group_detail(cx: &Cx, at: At, group: Group, offers: Offers, save: Autosave<'_>) -> Result {
    let action = update_action(at, group.id.inner());
    let cardinality = vec![
        Choice {
            value: "ONE",
            text: t(cx, "schema.cardinality.one").await?,
            current: matches!(group.cardinality, Cardinality::One),
        },
        Choice {
            value: "MANY",
            text: t(cx, "schema.cardinality.many").await?,
            current: matches!(group.cardinality, Cardinality::Many),
        },
    ];
    let cardinality_label = t(cx, "schema.cardinality").await?;
    let label_label = t(cx, "form.label").await?;
    let save_text = t(cx, "schema.save").await?;
    let id = group.id.inner().to_owned();
    view! {
        card(
            card_header(detail_heading(heading: save.heading))
            <form
                method="post"
                action=(action)
                class="contents"
                data-element-form=(id.as_str())
            >
                card_content(
                    <div class="flex flex-col gap-4">
                        text_field(
                            field: Field {
                                id: "element-label".to_owned(),
                                name: "label",
                                label: label_label,
                                value: group.label.clone(),
                            },
                            required: true,
                            help: None,
                            save: save
                        )
                        select_field(
                            field: Field {
                                id: "element-cardinality".to_owned(),
                                name: "cardinality",
                                label: cardinality_label,
                                value: String::new(),
                            },
                            choices: cardinality,
                            save: save
                        )
                        if offers.audience {
                            audience_field(current: group.audience, save: save)
                        }
                        status_line(save: save)
                    </div>
                )
                noscript_save(text: save_text)
            </form>
        )
        <div class="mt-6">
            add_form::form(
                procedure_id: at.procedure_id,
                organization_id: at.organization_id,
                facts: add_form::AddFacts {
                    parent: Some(id.clone()),
                    section_parent: false,
                    audience_offered: offers.audience_below,
                    heading_level_top: false,
                }
            )
        </div>
    }
}

/// A section: its title, its help text, and its audience.
#[component]
async fn section_detail(
    cx: &Cx,
    at: At,
    section: Section,
    offers: Offers,
    save: Autosave<'_>,
) -> Result {
    let action = update_action(at, section.id.inner());
    let title_label = t(cx, "form.title").await?;
    let help_label = t(cx, "schema.section.help").await?;
    let save_text = t(cx, "schema.save").await?;
    let id = section.id.inner().to_owned();
    view! {
        card(
            card_header(detail_heading(heading: save.heading))
            <form
                method="post"
                action=(action)
                class="contents"
                data-element-form=(id.as_str())
            >
                card_content(
                    <div class="flex flex-col gap-4">
                        text_field(
                            field: Field {
                                id: "element-title".to_owned(),
                                name: "title",
                                label: title_label,
                                value: section.title.clone(),
                            },
                            required: true,
                            help: None,
                            save: save
                        )
                        text_field(
                            field: Field {
                                id: "element-help".to_owned(),
                                name: "help",
                                label: help_label,
                                value: section.help.clone().unwrap_or_default(),
                            },
                            required: false,
                            help: None,
                            save: save
                        )
                        if offers.audience {
                            audience_field(current: section.audience, save: save)
                        }
                        status_line(save: save)
                    </div>
                )
                noscript_save(text: save_text)
            </form>
        )
        <div class="mt-6">
            add_form::form(
                procedure_id: at.procedure_id,
                organization_id: at.organization_id,
                facts: add_form::AddFacts {
                    parent: Some(id.clone()),
                    section_parent: true,
                    audience_offered: offers.audience_below,
                    heading_level_top: false,
                }
            )
        </div>
    }
}

/// A note: an optional title, the body it exists for, its audience.
#[component]
async fn note_detail(
    cx: &Cx,
    at: At,
    note: Note,
    offers_audience: bool,
    save: Autosave<'_>,
) -> Result {
    let action = update_action(at, note.id.inner());
    let title_label = t(cx, "form.title").await?;
    let body_label = t(cx, "schema.note.body").await?;
    let save_text = t(cx, "schema.save").await?;
    let id = note.id.inner().to_owned();
    view! {
        card(
            card_header(detail_heading(heading: save.heading))
            <form
                method="post"
                action=(action)
                class="contents"
                data-element-form=(id.as_str())
            >
                card_content(
                    <div class="flex flex-col gap-4">
                        text_field(
                            field: Field {
                                id: "element-title".to_owned(),
                                name: "title",
                                label: title_label,
                                value: note.title.clone().unwrap_or_default(),
                            },
                            required: false,
                            help: None,
                            save: save
                        )
                        textarea_field(
                            field: Field {
                                id: "element-body".to_owned(),
                                name: "body",
                                label: body_label,
                                value: note.body.clone(),
                            },
                            required: true,
                            save: save
                        )
                        if offers_audience {
                            audience_field(current: note.audience, save: save)
                        }
                        status_line(save: save)
                    </div>
                )
                noscript_save(text: save_text)
            </form>
        )
    }
}

/// A column: its label and type, then the fieldsets that type can
/// use (bound to the `kind` signal, so choosing a type shows and
/// hides them without a round trip), whether it is required, and its
/// audience. The enum options are a card of their own beneath.
#[component]
async fn column_detail(
    cx: &Cx,
    at: At,
    column: Column,
    offers_audience: bool,
    save: Autosave<'_>,
    facets: Facets<'_>,
) -> Result {
    let id = column.id.inner().to_owned();
    let action = update_action(at, &id);
    let label_label = t(cx, "form.label").await?;
    let required_label = t(cx, "schema.required").await?;
    let save_text = t(cx, "schema.save").await?;
    let unit_label = t(cx, "schema.unit").await?;
    let arity_label = t(cx, "schema.arity").await?;
    let accept_label = t(cx, "schema.attachment.accept").await?;
    let accept_help = t(cx, "schema.attachment.accept.help").await?;
    let max_bytes_label = t(cx, "schema.attachment.max-bytes").await?;
    let mut units = vec![Choice {
        value: "",
        text: t(cx, "schema.unit.none").await?,
        current: unit_of(&column.ty).is_none(),
    }];
    for unit in UNITS {
        units.push(Choice {
            value: unit_name(*unit),
            text: unit_name(*unit).to_owned(),
            current: unit_of(&column.ty) == Some(*unit),
        });
    }
    let arity = vec![
        Choice {
            value: "ONE",
            text: t(cx, "schema.arity.one").await?,
            current: !multiple_of(&column.ty),
        },
        Choice {
            value: "MANY",
            text: t(cx, "schema.arity.many").await?,
            current: multiple_of(&column.ty),
        },
    ];
    let (accept, max_bytes) = match &column.ty {
        ColumnType::Attachment(AttachmentType {
            accept, max_bytes, ..
        }) => (
            accept.join(", "),
            max_bytes.map(|n| n.to_string()).unwrap_or_default(),
        ),
        _ => (String::new(), String::new()),
    };
    let Facets { kind, .. } = facets;
    view! {
        card(
            card_header(detail_heading(heading: save.heading))
            <form
                method="post"
                action=(action)
                class="contents"
                data-element-form=(id.as_str())
            >
                card_content(
                    <div class="flex flex-col gap-4">
                        text_field(
                            field: Field {
                                id: "element-label".to_owned(),
                                name: "label",
                                label: label_label,
                                value: column.label.clone(),
                            },
                            required: true,
                            help: None,
                            save: save
                        )
                        kind_field(
                            current: kind_of(&column.ty).to_owned(),
                            save: save,
                            facets: facets
                        )
                        <div
                            class="flex flex-col gap-2"
                            :hidden=$(if kind.get() == "INTEGER" {
                                false
                            } else {
                                kind.get() != "DECIMAL"
                            })
                            data-facet="unit"
                        >
                            select_field(
                                field: Field {
                                    id: "element-unit".to_owned(),
                                    name: "unit",
                                    label: unit_label,
                                    value: String::new(),
                                },
                                choices: units,
                                save: save
                            )
                        </div>
                        <div
                            class="flex flex-col gap-4"
                            :hidden=$(kind.get() != "ATTACHMENT")
                            data-facet="attachment"
                        >
                            text_field(
                                field: Field {
                                    id: "element-accept".to_owned(),
                                    name: "accept",
                                    label: accept_label,
                                    value: accept,
                                },
                                required: false,
                                help: Some(accept_help),
                                save: save
                            )
                            number_field(
                                field: Field {
                                    id: "element-max-bytes".to_owned(),
                                    name: "max_bytes",
                                    label: max_bytes_label,
                                    value: max_bytes,
                                },
                                save: save
                            )
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
                            select_field(
                                field: Field {
                                    id: "element-arity".to_owned(),
                                    name: "arity",
                                    label: arity_label,
                                    value: String::new(),
                                },
                                choices: arity,
                                save: save
                            )
                        </div>
                        format_fields(
                            column: column.clone(),
                            save: save,
                            facets: facets
                        )
                        switch_field(
                            field: Field {
                                id: "element-required".to_owned(),
                                name: "required",
                                label: required_label,
                                value: String::new(),
                            },
                            checked: column.required,
                            save: save
                        )
                        if offers_audience {
                            audience_field(current: column.audience, save: save)
                        }
                        status_line(save: save)
                    </div>
                )
                noscript_save(text: save_text)
            </form>
        )
        <div class="mt-6" :hidden=$(kind.get() != "ENUM") data-facet="options">
            options_card(at: at, column: column.clone(), save: save, facets: facets)
        </div>
    }
}

/// The type select. Its own handler, because the choice does two
/// things: it saves, and it mirrors into the `kind` signal the
/// fieldsets above are bound to.
#[component]
async fn kind_field(cx: &Cx, current: String, save: Autosave<'_>, facets: Facets<'_>) -> Result {
    let Autosave {
        pid,
        eid,
        saving,
        revision,
        status,
        saved_text,
        heading,
    } = save;
    let Facets { kind, .. } = facets;
    let kind_label = t(cx, "schema.type").await?;
    let mut names = Vec::new();
    for name in KINDS {
        names.push((*name, t(cx, kind_message_id_of(name)).await?));
    }
    view! {
        <div class="flex flex-col gap-2">
            label(attrs: attributes! { for="element-kind" }, (kind_label.as_str()))
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
                        status.set(saved_text.get());
                        heading.set(outcome.unwrap());
                        revision.increment();
                    } else {
                        status.set(outcome.unwrap_err());
                    }
                })
            >
                for (value, text) in &names {
                    <option value=(*value) selected=(*value == current)>
                        (text.as_str())
                    </option>
                }
            </select>
        </div>
    }
}

/// A text column's format: the choice, and the custom pattern it may
/// need. Both controls save through
/// [`super::autosave::save_text_format`] with **both** values,
/// because the stored format is derived from the pair — sending one
/// at a time can never reach a custom pattern.
#[component]
async fn format_fields(cx: &Cx, column: Column, save: Autosave<'_>, facets: Facets<'_>) -> Result {
    let Autosave {
        pid,
        eid,
        saving,
        revision,
        status,
        saved_text,
        heading,
    } = save;
    let Facets {
        kind,
        format,
        pattern,
        ..
    } = facets;
    let (current_format, current_pattern) = text_format_of(&column.ty);
    let current_pattern = current_pattern.to_owned();
    let format_label = t(cx, "schema.format").await?;
    let pattern_label = t(cx, "schema.format.pattern").await?;
    let pattern_help = t(cx, "schema.format.pattern.help").await?;
    let choices = [
        ("", t(cx, "schema.format.none").await?),
        ("EMAIL", t(cx, "schema.format.email").await?),
        ("PHONE", t(cx, "schema.format.phone").await?),
        ("IBAN", t(cx, "schema.format.iban").await?),
        ("REGEX", t(cx, "schema.format.regex").await?),
    ];
    view! {
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
                            status.set(saved_text.get());
                            heading.set(outcome.unwrap());
                            revision.increment();
                        } else {
                            status.set(outcome.unwrap_err());
                        }
                    })
                >
                    for (value, text) in &choices {
                        <option value=(*value) selected=(*value == current_format)>
                            (text.as_str())
                        </option>
                    }
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
                    value=(current_pattern.as_str())
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
                            status.set(saved_text.get());
                            heading.set(outcome.unwrap());
                            revision.increment();
                        } else {
                            status.set(outcome.unwrap_err());
                        }
                    })
                >

                <p id="element-pattern-help" class="text-sm text-muted-foreground">
                    (pattern_help.as_str())
                </p>
            </div>
        </div>
    }
}

/// The enum column's options: one row per option (its label
/// autosaving, a remove button), and an explicit *Add option* form.
/// An empty choice is a legitimate draft state — publication is
/// what refuses it, not this form.
#[component]
async fn options_card(
    cx: &Cx,
    at: At,
    column: Column,
    save: Autosave<'_>,
    facets: Facets<'_>,
) -> Result {
    let Autosave {
        pid,
        eid,
        saving,
        revision,
        saved_text,
        ..
    } = save;
    let Facets { option_status, .. } = facets;
    let element = column.id.inner().to_owned();
    let options_label = t(cx, "schema.options").await?;
    let options_help = t(cx, "schema.options.help").await?;
    let option_label = t(cx, "schema.options.label").await?;
    let options_empty = t(cx, "schema.options.empty").await?;
    let option_new_label = t(cx, "schema.options.new").await?;
    let options_add = t(cx, "schema.options.add").await?;
    // `(id, label, the accessible name of its remove button)`.
    let mut options: Vec<(String, String, String)> = match &column.ty {
        ColumnType::Enum(choice) => choice
            .options
            .iter()
            .map(|o| (o.id.inner().to_owned(), o.label.clone(), String::new()))
            .collect(),
        _ => Vec::new(),
    };
    for (_, text, remove_name) in &mut options {
        *remove_name = t_args(cx, "schema.options.remove", &one_arg("label", text.clone())).await?;
    }
    let add_action = href!(
        elements::element::options::add::submit,
        OrganizationId(at.organization_id),
        ProcedureId(at.procedure_id),
        ElementId(element.clone())
    );
    let update_action = || {
        href!(
            elements::element::options::update::submit,
            OrganizationId(at.organization_id),
            ProcedureId(at.procedure_id),
            ElementId(element.clone())
        )
    };
    let remove_action = || {
        href!(
            elements::element::options::remove::submit,
            OrganizationId(at.organization_id),
            ProcedureId(at.procedure_id),
            ElementId(element.clone())
        )
    };
    view! {
        card(
            card_header(
                <h3 class="leading-none font-semibold">(options_label.as_str())</h3>
            )
            card_content(
                <div class="flex flex-col gap-4">
                    <p class="text-sm text-muted-foreground">(options_help.as_str())</p>
                    if options.is_empty() {
                        <p class="text-sm text-muted-foreground" data-options-empty="">
                            (options_empty.as_str())
                        </p>
                    } else {
                        <ul class="flex flex-col gap-2">
                            for (index, (option_id, text, remove_name)) in options.iter().enumerate() {
                                <li
                                    class="flex items-center gap-2"
                                    data-option-id=(option_id.as_str())
                                >
                                    <form
                                        method="post"
                                        action=(update_action())
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
                                            value=(text.as_str())
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
                                                    option_status.set(saved_text.get());
                                                    revision.increment();
                                                } else {
                                                    option_status.set(outcome.unwrap_err());
                                                }
                                            })
                                        >
                                    </form>
                                    <form method="post" action=(remove_action())>
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
                        action=(add_action)
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
                            attrs: attributes! { type="submit" },
                            (options_add.as_str())
                        )
                    </form>
                </div>
            )
        )
    }
}

/// Where an element's detail form posts.
fn update_action(at: At, element_id: &str) -> impl topcoat::view::AttributeValueViewParts {
    href!(
        elements::element::update::submit,
        OrganizationId(at.organization_id),
        ProcedureId(at.procedure_id),
        ElementId(element_id.to_owned())
    )
}
