//! The impact report as localized lines — shared by the publish
//! confirmation ([`super::schema::publish`]) and the history diff
//! page ([`super::history`]). One line per changed column, named by
//! the server-resolved label (G.11.5): a removed column is named by
//! the base schema, so removals read like every other change.

use platform_client::procedure::{ChangeClass, ColumnChangeKind, ImpactReport};
use topcoat::{Result, context::Cx};

use crate::i18n::{t, t_args};
use crate::pages::{args, one_arg};

/// The report, localized: one line per changed column with what the
/// change does to existing answers.
pub(crate) async fn report_lines(cx: &Cx, report: &ImpactReport) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for entry in &report.columns {
        let mut line = match entry.change {
            ColumnChangeKind::Removed => {
                t_args(
                    cx,
                    "schema.impact.removed",
                    &one_arg("label", entry.label.clone()),
                )
                .await?
            }
            // §3.1: a rename is safe and *reported* — this line is
            // what an empty diff used to swallow.
            ColumnChangeKind::Relabeled => {
                let from = entry.renamed_from.clone().unwrap_or_default();
                t_args(
                    cx,
                    "schema.impact.relabeled",
                    &args([("from", from.into()), ("label", entry.label.clone().into())]),
                )
                .await?
            }
            change => {
                let message = match change {
                    ColumnChangeKind::Added => "schema.impact.added",
                    ColumnChangeKind::Cast => "schema.impact.cast",
                    ColumnChangeKind::ScopeMoved => "schema.impact.scope-moved",
                    ColumnChangeKind::Forbidden => "schema.impact.forbidden",
                    ColumnChangeKind::Removed | ColumnChangeKind::Relabeled => {
                        unreachable!("matched above")
                    }
                };
                let class = t(cx, class_id(entry.class)).await?;
                t_args(
                    cx,
                    message,
                    &args([
                        ("label", entry.label.clone().into()),
                        ("class", class.into()),
                    ]),
                )
                .await?
            }
        };
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
    for group in &report.relabeled_groups {
        lines.push(
            t_args(
                cx,
                "schema.impact.group-relabeled",
                &args([
                    ("from", group.from.clone().into()),
                    ("to", group.to.clone().into()),
                ]),
            )
            .await?,
        );
    }
    Ok(lines)
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
