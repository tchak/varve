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
//!
//! The draft-state line is a `#[shard]` ([`state`]) rather than a
//! string, because an autosave moves it — the draft was saved just
//! now — and the message is MF2 (plural categories, a CLDR date), so
//! only the server can format it. It watches the same `revision`
//! counter the structure panel does; on the preview, where nothing
//! saves, that counter never moves and the shard never re-fetches.
//! Like every shard it is a public endpoint the page guard does not
//! cover, so it authorizes itself.

use topcoat::{
    Result,
    context::Cx,
    router::{error::RouterErrorExt, href},
    runtime::{Signal, shard},
    view::{attributes, component, view},
};

use crate::{
    client,
    components::{
        badge::{BadgeVariant, badge},
        breadcrumbs::{Crumb, breadcrumbs},
        button::{ButtonSize, ButtonVariant, button, button_variants},
        page_title::page_title,
        tabs::{tabs, tabs_list, tabs_trigger},
    },
    i18n::{t, t_args},
    pages::{args, one_arg, utc_date_arg},
};

use platform_client::revision_draft::ProcedureRevisionDraft;

use super::super::super::super::OrganizationId;
use super::super::{ProcedureId, counts};
use super::autosave::draft_of;
use super::{page, preview, publish};

/// Which tab is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::pages) enum Tab {
    Editor,
    Preview,
}

/// The header. `offer_discard` (the discard link) and
/// `offer_publish` (the publish button, posting the free phase of
/// `publishRevision` — [`publish::submit`]) are the editor's actions
/// — absent on the preview, and absent on the editor while a
/// confirmation is open (the confirmation replaces them).
///
/// Boxed: the editor page's `view!` is deep enough that an unboxed
/// header frame overflows the stack in debug builds, the same reason
/// `structure::tree_row` and `detail::panel` are boxed.
#[component(boxed)]
pub(in crate::pages) async fn header(
    cx: &Cx,
    procedure: ProcedureRevisionDraft,
    tab: Tab,
    offer_discard: bool,
    offer_publish: bool,
    revision: &Signal<f64>,
) -> Result {
    let organization_id: uuid::Uuid = procedure.organization.id.inner().parse()?;
    let procedure_id: uuid::Uuid = procedure.id.inner().parse()?;
    let title = t_args(
        cx,
        "schema.title",
        &one_arg("procedure", procedure.title.clone()),
    )
    .await?;
    let crumb_label = t(cx, "nav.breadcrumb").await?;
    let draft_badge = t(cx, "procedure.draft.badge").await?;
    let discard_label = t(cx, "schema.discard").await?;
    let publish_label = t(cx, "schema.publish").await?;
    let tab_editor = t(cx, "schema.tab.editor").await?;
    let tab_preview = t(cx, "schema.tab.preview").await?;
    let in_progress = procedure.revision_draft.in_progress;
    let procedure_id_string = procedure_id.to_string();
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
    let crumbs = vec![
        crate::pages::organizations_crumb(cx).await?,
        Crumb::link(
            procedure.organization.name.clone(),
            href!(
                crate::pages::organizations::show::page,
                OrganizationId(organization_id)
            )
            .resolve(cx),
        ),
        Crumb::link(
            t(cx, "procedures.title").await?,
            href!(
                crate::pages::organizations::show::procedures::page,
                OrganizationId(organization_id)
            )
            .resolve(cx),
        ),
        Crumb::link(procedure.title.clone(), procedure_href.resolve(cx)),
        Crumb::here(t(cx, "schema.crumb").await?),
    ];
    let discard_href = href!(
        page,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    )
    .query(&[("discard", "confirm")]);
    let publish_href = href!(
        publish::submit,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    );
    view! {
        signal pid = procedure_id_string.clone();

        <div class="flex flex-col gap-2">
            breadcrumbs(label: crumb_label, crumbs: crumbs)
            page_title((title))
            <div
                class="flex flex-wrap items-center gap-3 text-sm text-muted-foreground"
            >
                if in_progress {
                    badge(variant: BadgeVariant::Secondary, (draft_badge))
                }
                state(procedure_id: $(pid.get()), revision: $(revision.get()))
                if in_progress && offer_publish {
                    <form method="post" action=(publish_href)>
                        button(
                            variant: ButtonVariant::Primary,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" },
                            (publish_label)
                        )
                    </form>
                }
                if in_progress && offer_discard {
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

/// The draft-state line as a shard: how much the draft holds and
/// when it was last saved; with nothing in progress, the published
/// schema (head until touched) or that there is no draft yet.
/// Re-rendered when an autosave bumps `revision`, since the saved-on
/// date it carries is *now* afterwards. Authorizes itself through
/// the client; an unreadable procedure is the 404.
#[shard]
async fn state(cx: &Cx, procedure_id: String, revision: f64) -> Result {
    let _ = revision;
    let client = client(cx).await?;
    let procedure = draft_of(cx, &client, &procedure_id)
        .await?
        .ok_or_not_found()?;
    let line = state_line(cx, &procedure).await?;
    view! { <span data-schema-state="">(line.as_str())</span> }
}

/// The draft-state line's text.
async fn state_line(cx: &Cx, procedure: &ProcedureRevisionDraft) -> Result<String> {
    let draft = &procedure.revision_draft;
    if !draft.in_progress {
        // Pristine: the published head (base names it), or nothing
        // at all on a never-published procedure.
        return if draft.base.is_some() {
            t(cx, "schema.state.published").await
        } else {
            t(cx, "schema.state.none").await
        };
    }
    let (columns, groups) = counts(&draft.elements);
    t_args(
        cx,
        "schema.state.draft",
        &args([
            ("columns", (columns as i64).into()),
            ("groups", (groups as i64).into()),
            ("date", utc_date_arg(procedure.updated_at)),
        ]),
    )
    .await
}
