//! `…/schema/publish`: **publication** of the revision draft
//! (design/platform.md P.4 *Publication*, `design/graphql.md` G.10)
//! — the POST that runs the two-phase `publishRevision`, and the
//! confirmation state that carries the impact report to the
//! administrator.
//!
//! The header's publish button posts without `confirm`: a `SAFE`
//! report publishes immediately (G.10 — the walking-skeleton path
//! never sees a confirmation). A worse report answers `published:
//! false` with nothing written, and the POST lands on
//! `?publish=confirm`, where [`confirmation`] shows the report — the
//! *read-time* `RevisionDraft.report`, the same classification the
//! mutation gates on — above a form that re-sends with
//! `confirm=true`. The state is a URL, so it is also reachable
//! directly; the report is recomputed on every GET and never has to
//! survive a redirect.

use cynic::MutationBuilder;
use platform_client::procedure::{
    ChangeClass, ColumnChangeKind, ImpactReport, PublishRevision, PublishRevisionInput,
    PublishRevisionVariables,
};
use platform_client::revision_draft::Element;
use platform_client::{Code, Error};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::not_found, href, page, path_param},
    view::{attributes, component, view},
};

use crate::{
    client,
    components::{
        alert::{AlertVariant, alert, alert_description},
        button::{ButtonSize, ButtonVariant, button, button_variants},
    },
    i18n::{t, t_args},
    pages::{args, one_arg},
};

use super::super::super::super::OrganizationId;
use super::super::ProcedureId;
use super::element::{id_of, label_of};
use super::{Notice, NoticeKind, back_to_editor, done};

/// What the publish forms post: the header's button sends nothing,
/// the confirmation's re-send carries `confirm=true`.
#[derive(Deserialize)]
pub(super) struct Confirmation {
    #[serde(default)]
    confirm: String,
}

/// `publishRevision`, both phases. Published lands on the editor
/// with a status notice; a report worse than `SAFE` without confirm
/// lands on `?publish=confirm`, which shows it.
#[page(POST)]
pub(super) async fn submit(cx: &Cx, Form(input): Form<Confirmation>) -> Result {
    let client = client(cx).await?;
    let organization = path_param::<OrganizationId>(cx)?;
    let procedure = path_param::<ProcedureId>(cx)?;
    let result = platform_client::run(
        &client,
        PublishRevision::build(PublishRevisionVariables {
            input: PublishRevisionInput {
                procedure_id: cynic::Id::new(procedure.to_string()),
                confirm: input.confirm == "true",
            },
        }),
    )
    .await;
    match result {
        Ok(result) if result.publish_revision.published => {
            let notice = done(cx, "schema.notice.published").await?;
            back_to_editor(cx, None, Some(notice)).await
        }
        Ok(_) => {
            let location = href!(
                super::page,
                OrganizationId(*organization),
                ProcedureId(*procedure)
            )
            .query(&[("publish", "confirm")])
            .resolve(cx);
            crate::pages::redirect_to(cx, location).await
        }
        Err(error) => {
            let notice = refused_publication(cx, error).await?;
            back_to_editor(cx, None, Some(notice)).await
        }
    }
}

/// A refused publication, mapped the editor's way (G.10):
/// `FORBIDDEN` is the 404; `INVALID_DRAFT` (nothing to publish, or a
/// choice with no options) carries the server's reason; `CONFLICT`
/// is the stale fork — another revision was published since this
/// draft was started, so fixing it means discarding.
async fn refused_publication(cx: &Cx, error: Error) -> Result<Notice> {
    match error.code() {
        Some(Code::Forbidden) => Err(not_found().into()),
        Some(Code::Conflict) => Ok(Notice {
            kind: NoticeKind::Alert,
            text: t(cx, "schema.publish.error.conflict").await?,
        }),
        Some(Code::InvalidDraft | Code::InvalidInput) => Ok(Notice {
            kind: NoticeKind::Alert,
            text: t_args(
                cx,
                "schema.publish.error.refused",
                &one_arg("reason", error.to_string()),
            )
            .await?,
        }),
        _ => Err(error.into()),
    }
}

/// The confirmation state (`?publish=confirm`): the report as a list
/// of localized lines, then the re-send form and the way back.
#[component]
pub(super) async fn confirmation(
    cx: &Cx,
    organization_id: uuid::Uuid,
    procedure_id: uuid::Uuid,
    elements: Vec<Element>,
    report: ImpactReport,
) -> Result {
    let question = t(cx, "schema.publish.question").await?;
    let confirm_label = t(cx, "schema.publish.confirm").await?;
    let keep = t(cx, "schema.publish.keep").await?;
    let no_changes = t(cx, "schema.impact.none").await?;
    let lines = report_lines(cx, &elements, &report).await?;
    let publish_href = href!(
        submit,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    );
    let editor_href = href!(
        super::page,
        OrganizationId(organization_id),
        ProcedureId(procedure_id)
    );
    let variant = match report.worst {
        ChangeClass::Safe => AlertVariant::Neutral,
        _ => AlertVariant::Destructive,
    };
    view! {
        alert(
            variant: variant,
            attrs: attributes! { role="alertdialog" aria-labelledby="publish-question" },
            alert_description(
                <p id="publish-question" class="mb-3">(question)</p>
                if lines.is_empty() {
                    <p class="mb-3">(no_changes)</p>
                } else {
                    <ul class="mb-3 list-disc pl-5" data-impact-report="">
                        for line in &lines {
                            <li>(line.as_str())</li>
                        }
                    </ul>
                }
                <div class="flex gap-2">
                    <form method="post" action=(publish_href)>
                        <input type="hidden" name="confirm" value="true" />
                        button(
                            variant: ButtonVariant::Primary,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" },
                            (confirm_label)
                        )
                    </form>
                    <a
                        href=(editor_href)
                        class=(button_variants(
                            ButtonVariant::Outline,
                            ButtonSize::Sm,
                        ))
                    >
                        (keep)
                    </a>
                </div>
            )
        )
    }
}

/// The report, localized: one line per changed column — named
/// through the draft's elements — with what the change does to
/// existing answers. Removals are aggregated into one count line:
/// a removed column is no longer in the draft, so it has no label
/// to name it by (and removal is `SAFE` — hidden never deletes).
async fn report_lines(cx: &Cx, elements: &[Element], report: &ImpactReport) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    let mut removed: i64 = 0;
    for entry in &report.columns {
        let message = match entry.change {
            ColumnChangeKind::Removed => {
                removed += 1;
                continue;
            }
            ColumnChangeKind::Added => "schema.impact.added",
            ColumnChangeKind::Cast => "schema.impact.cast",
            ColumnChangeKind::ScopeMoved => "schema.impact.scope-moved",
            ColumnChangeKind::Forbidden => "schema.impact.forbidden",
        };
        let label = label_for(elements, entry.column_id.inner());
        let class = t(cx, class_id(entry.class)).await?;
        let mut line = t_args(
            cx,
            message,
            &args([("label", label.into()), ("class", class.into())]),
        )
        .await?;
        if !entry.removed_options.is_empty() {
            let suffix = t_args(
                cx,
                "schema.impact.options-removed",
                &one_arg("n", entry.removed_options.len() as i64),
            )
            .await?;
            line.push(' ');
            line.push_str(&suffix);
        }
        lines.push(line);
    }
    if removed > 0 {
        lines.push(t_args(cx, "schema.impact.removed", &one_arg("n", removed)).await?);
    }
    Ok(lines)
}

/// The column's label in the draft; the raw id only if the server
/// reports a change on a column the draft does not hold (total,
/// like every `element` helper).
fn label_for(elements: &[Element], id: &str) -> String {
    elements
        .iter()
        .find(|e| id_of(e) == id)
        .map(|e| label_of(e).to_owned())
        .unwrap_or_else(|| id.to_owned())
}

/// What the class does to existing answers, as a message id.
fn class_id(class: ChangeClass) -> &'static str {
    match class {
        ChangeClass::Safe => "schema.impact.class.safe",
        ChangeClass::Lossy => "schema.impact.class.lossy",
        ChangeClass::Checked => "schema.impact.class.checked",
        ChangeClass::Breaking => "schema.impact.class.breaking",
    }
}
