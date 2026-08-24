//! The **schema header**: the title, the way back to the procedure,
//! the draft state, and the tab rail — everything above the panels,
//! shared by the editor and the preview.
//!
//! The two tabs are two pages over one draft (the settings-shell
//! `tabs_trigger` pattern: links, not client state), so the rail has
//! to be identical on both and know which one is showing. Keeping it
//! here is what makes that true by construction rather than by two
//! copies staying in step.
//!
//! The optional discard link is the editor's alone: the preview
//! shows the draft, it does not act on it.

use topcoat::{
    Result,
    context::Cx,
    router::href,
    view::{attributes, component, view},
};

use crate::{
    components::{
        badge::{BadgeVariant, badge},
        button::{ButtonSize, ButtonVariant, button_variants},
        page_title::page_title,
        tabs::{tabs, tabs_list, tabs_trigger},
    },
    i18n::{t, t_args},
    pages::{args, one_arg, utc_date_arg},
};

use platform_client::revision_draft::ProcedureRevisionDraft;

use super::super::super::super::OrganizationId;
use super::super::{ProcedureId, counts};
use super::{page, preview};

/// Which tab is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::pages) enum Tab {
    Editor,
    Preview,
}

/// The header. `offer_discard` is the editor's discard link — absent
/// on the preview, and absent on the editor while the confirmation
/// is open (the confirmation replaces it).
#[component]
pub(in crate::pages) async fn header(
    cx: &Cx,
    procedure: ProcedureRevisionDraft,
    tab: Tab,
    offer_discard: bool,
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
    let discard_label = t(cx, "schema.discard").await?;
    let tab_editor = t(cx, "schema.tab.editor").await?;
    let tab_preview = t(cx, "schema.tab.preview").await?;
    let has_draft = procedure.revision_draft.is_some();
    let elements = procedure
        .revision_draft
        .as_ref()
        .map(|d| d.elements.as_slice())
        .unwrap_or_default();
    let (columns, groups) = counts(elements);
    let state = if has_draft {
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
    } else {
        t(cx, "schema.state.none").await?
    };
    let editor_href = href!(
        page,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    );
    let preview_href = href!(
        preview::page,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    );
    let procedure_href = href!(
        super::super::page,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    );
    let discard_href = href!(
        page,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    )
    .query(&[("discard", "confirm")]);
    view! {
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
                if has_draft {
                    badge(variant: BadgeVariant::Secondary, (draft_badge))
                }
                <span data-schema-state="">(state)</span>
                if has_draft && offer_discard {
                    <a
                        href=(discard_href)
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
                    active: tab == Tab::Editor,
                    attrs: attributes! { href=(editor_href) },
                    (tab_editor)
                )
                tabs_trigger(
                    active: tab == Tab::Preview,
                    attrs: attributes! { href=(preview_href) },
                    (tab_preview)
                )
            )
        )
    }
}
