//! History (platform P.4 *Procedure history*): the event log as the
//! procedure page's history section — rendered newest-first, one
//! localized line per event — and, per publication, the way into the
//! diff page ([`show`]). The trail is bounded by design (G.9), so
//! the whole list renders without paging.

pub(super) mod show;

use platform_client::procedure::ProcedureEvent;
use topcoat::{
    Result,
    context::Cx,
    router::href,
    view::{View, component, view},
};

use crate::components::card::{card, card_content, card_header};
use crate::i18n::{t, t_args};
use crate::pages::{args, utc_date_arg};

use crate::pages::organizations::show::OrganizationId;

use super::ProcedureId;

/// One event as its localized line: what happened, who acted, when.
pub(super) async fn event_line(cx: &Cx, event: &ProcedureEvent) -> Result<String> {
    use platform_client::procedure::ProcedureEventKind;
    let message = match event.kind() {
        ProcedureEventKind::Created => "procedure.history.created",
        ProcedureEventKind::Published => "procedure.history.published",
        ProcedureEventKind::Closed => "procedure.history.closed",
        ProcedureEventKind::Reopened => "procedure.history.reopened",
    };
    let actor = match event.actor() {
        Some(actor) => actor.name.clone(),
        None => t(cx, "procedure.history.actor.system").await?,
    };
    t_args(
        cx,
        message,
        &args([
            ("actor", actor.into()),
            ("date", utc_date_arg(event.created_at())),
        ]),
    )
    .await
}

/// The history card: the trail newest-first, each publication
/// linking to its diff. The link's visible text repeats per row, so
/// an `aria-label` names each one by its date.
#[component]
pub(super) async fn section(
    cx: &Cx,
    organization_id: uuid::Uuid,
    procedure_id: uuid::Uuid,
    events: Vec<ProcedureEvent>,
) -> Result<impl View> {
    let heading = t(cx, "procedure.history.title").await?;
    let diff_label = t(cx, "procedure.history.diff").await?;
    let mut rows = Vec::new();
    for event in events.iter().rev() {
        let line = event_line(cx, event).await?;
        let diff = match event {
            ProcedureEvent::Published(published) => {
                let event_id: uuid::Uuid = published.id.inner().parse()?;
                let name = t_args(
                    cx,
                    "procedure.history.diff.label",
                    &args([("date", utc_date_arg(published.created_at))]),
                )
                .await?;
                Some((event_id, name))
            }
            ProcedureEvent::Other(_) => None,
        };
        rows.push((line, diff));
    }
    Ok(view! {
        card(
            card_header(<h2 class="leading-none font-semibold">(heading)</h2>)
            card_content(
                <ul class="flex flex-col gap-2 text-sm" data-history="">
                    for (line, diff) in &rows {
                        <li class="flex flex-wrap items-baseline gap-x-2">
                            <span>(line.as_str())</span>
                            if let Some((event_id, name)) = diff {
                                <a
                                    href=(href!(
                                        show::page,
                                        OrganizationId(organization_id),
                                        ProcedureId(procedure_id),
                                        show::EventId(*event_id),
                                    ))
                                    aria-label=(name.as_str())
                                    class="font-medium underline-offset-4 hover:underline"
                                >
                                    (diff_label.as_str())
                                </a>
                            }
                        </li>
                    }
                </ul>
            )
        )
    })
}
