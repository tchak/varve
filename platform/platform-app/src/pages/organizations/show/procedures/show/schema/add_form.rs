//! The **add form**: what to add (column, group, section or note),
//! its label, and — for a column — its type, posted to
//! [`super::add::submit`].
//!
//! It appears in two places, which is why it is a component and not
//! part of the detail panel: with nothing selected it *is* the detail
//! panel (adding at the top level), and inside a selected container
//! it sits under that container's form (adding into it). The ids are
//! prefixed by the parent so the two never collide on one page.
//!
//! The type select shows for a column only, driven by a signal this
//! form declares itself — no autosave here: an add is one submission,
//! and the form has nothing to save until it is submitted.

use topcoat::{
    Result,
    context::Cx,
    router::href,
    runtime::Event,
    view::{View, attributes, component, view},
};

use crate::{
    components::{
        button::button,
        card::{card, card_content, card_footer, card_header},
        field::field,
        label::label,
    },
    i18n::t,
};

use super::super::super::super::OrganizationId;
use super::super::ProcedureId;
use super::add;
use super::controls::SELECT;
use super::element::{KINDS, kind_message_id_of};

/// Where the form sits and what it offers, bundled for [`form`]
/// (the props stay under the component-argument limit).
pub(in crate::pages) struct AddFacts {
    pub(in crate::pages) parent: Option<String>,
    pub(in crate::pages) section_parent: bool,
    /// `false` inside a reviewer-only container: everything added
    /// there is reviewer-only regardless (the server clamps), so the
    /// field would mislead.
    pub(in crate::pages) audience_offered: bool,
    pub(in crate::pages) heading_level_top: bool,
}

/// The form itself: what to add, its label, into `parent`.
#[component]
pub(in crate::pages) async fn form(
    cx: &Cx,
    procedure_id: uuid::Uuid,
    organization_id: uuid::Uuid,
    facts: AddFacts,
) -> Result<impl View> {
    let AddFacts {
        parent,
        section_parent,
        audience_offered,
        heading_level_top,
    } = facts;
    let heading = match (&parent, section_parent) {
        (Some(_), true) => t(cx, "schema.add.inside-section").await?,
        (Some(_), false) => t(cx, "schema.add.inside").await?,
        (None, _) => t(cx, "schema.add.title").await?,
    };
    let lead = t(cx, "schema.add.lead").await?;
    let what_label = t(cx, "schema.add.what").await?;
    let column_label = t(cx, "schema.kind.column").await?;
    let group_label = t(cx, "schema.kind.group").await?;
    let section_label = t(cx, "schema.kind.section").await?;
    let note_label = t(cx, "schema.kind.note").await?;
    let label_label = t(cx, "form.label").await?;
    let type_label = t(cx, "schema.type").await?;
    let submit = t(cx, "schema.add.submit").await?;
    let audience_label = t(cx, "schema.audience").await?;
    let audience_all = t(cx, "schema.audience.all").await?;
    let audience_reviewer = t(cx, "schema.audience.reviewer").await?;
    let mut kind_names = Vec::new();
    for kind in KINDS {
        kind_names.push((*kind, t(cx, kind_message_id_of(kind)).await?));
    }
    let prefix = match &parent {
        Some(id) => format!("add-{id}"),
        None => "add".to_owned(),
    };
    let what_id = format!("{prefix}-what");
    let type_id = format!("{prefix}-type");
    let label_id = format!("{prefix}-label");
    let audience_id = format!("{prefix}-audience");
    Ok(view! {
        // The type select shows for a column only; the signal is this
        // form's own (a handler reaches its own `view!`'s signals).
        signal what = "column".to_owned();

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
                action=(href!(
                    add::submit,
                    OrganizationId(organization_id),
                    ProcedureId(procedure_id),
                ))
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
                            <select
                                id=(what_id.as_str())
                                class=(SELECT)
                                name="what"
                                @change=$(|e: Event| what.set(e.target.value))
                            >
                                <option value="column">(column_label)</option>
                                <option value="group">(group_label)</option>
                                <option value="section">(section_label)</option>
                                <option value="note">(note_label)</option>
                            </select>
                        </div>
                        <div
                            class="flex flex-col gap-2"
                            :hidden=$(what.get() != "column")
                            data-facet="add-type"
                        >
                            label(
                                attrs: attributes! { for=(type_id.as_str()) },
                                (type_label)
                            )
                            <select id=(type_id.as_str()) class=(SELECT) name="kind">
                                for (kind_value, name) in &kind_names {
                                    <option value=(*kind_value)>(name.as_str())</option>
                                }
                            </select>
                        </div>
                        field(
                            id: label_id.as_str(),
                            label: label_label,
                            attrs: attributes! { type="text" name="label" required="" autocomplete="off" }
                        )
                        if audience_offered {
                            <div class="flex flex-col gap-2">
                                label(
                                    attrs: attributes! { for=(audience_id.as_str()) },
                                    (audience_label)
                                )
                                <select
                                    id=(audience_id.as_str())
                                    class=(SELECT)
                                    name="audience"
                                >
                                    <option value="ALL">(audience_all)</option>
                                    <option value="REVIEWER">(audience_reviewer)</option>
                                </select>
                            </div>
                        }
                    </div>
                )
                card_footer(button(attrs: attributes! { type="submit" }, (submit)))
            </form>
        )
    })
}
