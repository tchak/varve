//! The impact report as localized lines — shared by the publish
//! confirmation ([`super::schema::publish`]) and the history diff
//! page ([`super::history`]). One line per changed column, named by
//! the server-resolved label (G.11.5): a removed column is named by
//! the base schema, so removals read like every other change.

use platform_client::procedure::{
    ChangeClass, ColumnChangeKind, ImpactReport, SurfaceChangeEntry, SurfaceChangeKind,
};
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
    // §3.1 surface lines, deduplicated across the compiled pair
    // (P.4): the same change on both surfaces is one line; a
    // one-surface change says which, and a `CHECKED` change carries
    // the admissibility-lapse sentence.
    let mut grouped: Vec<(&SurfaceChangeEntry, Vec<&str>)> = Vec::new();
    for entry in &report.surfaces {
        match grouped.iter_mut().find(|(seen, _)| {
            seen.class == entry.class
                && seen.change == entry.change
                && seen.label == entry.label
                && seen.from == entry.from
                && seen.to == entry.to
        }) {
            Some((_, surfaces)) => surfaces.push(entry.surface.inner()),
            None => grouped.push((entry, vec![entry.surface.inner()])),
        }
    }
    for (entry, surfaces) in grouped {
        let mut line = t_args(
            cx,
            surface_message(entry.change),
            &args([
                ("label", entry.label.clone().unwrap_or_default().into()),
                ("from", entry.from.clone().unwrap_or_default().into()),
                ("to", entry.to.clone().unwrap_or_default().into()),
                ("surface", surfaces[0].to_owned().into()),
            ]),
        )
        .await?;
        if entry.class == ChangeClass::Checked {
            line.push(' ');
            line.push_str(&t(cx, "surface.impact.lapse").await?);
        }
        if !matches!(
            entry.change,
            SurfaceChangeKind::SurfaceAdded | SurfaceChangeKind::SurfaceRemoved
        ) && surfaces.len() == 1
        {
            let only = match surfaces[0] {
                "reviewer" => t(cx, "surface.impact.reviewer-only").await?,
                _ => t(cx, "surface.impact.applicant-only").await?,
            };
            line.push(' ');
            line.push_str(&only);
        }
        lines.push(line);
    }
    Ok(lines)
}

/// The §3.1 line for a change kind, self-contained (no `{$class}`
/// composition — a `CHECKED` line gets the lapse sentence appended).
fn surface_message(kind: SurfaceChangeKind) -> &'static str {
    use SurfaceChangeKind as K;
    match kind {
        K::SurfaceAdded => "surface.impact.surface-added",
        K::SurfaceRemoved => "surface.impact.surface-removed",
        K::SectionAdded => "surface.impact.section-added",
        K::SectionRemoved => "surface.impact.section-removed",
        K::SectionRetitled => "surface.impact.section-retitled",
        K::SectionHelpChanged => "surface.impact.section-help-changed",
        K::NoteAdded => "surface.impact.note-added",
        K::NoteRemoved => "surface.impact.note-removed",
        K::NoteChanged => "surface.impact.note-changed",
        K::ColumnPresented => "surface.impact.column-presented",
        K::ColumnWithdrawn => "surface.impact.column-withdrawn",
        K::PromptChanged => "surface.impact.prompt-changed",
        K::HelpChanged => "surface.impact.help-changed",
        K::GroupPromptChanged => "surface.impact.group-prompt-changed",
        K::RequirednessTightened => "surface.impact.requiredness-tightened",
        K::RequirednessLoosened => "surface.impact.requiredness-loosened",
        K::RequirednessChanged => "surface.impact.requiredness-changed",
        K::VisibilityChanged => "surface.impact.visibility-changed",
        K::FormatTightened => "surface.impact.format-tightened",
        K::FormatLoosened => "surface.impact.format-loosened",
        K::FormatChanged => "surface.impact.format-changed",
        K::WritePolicyChanged => "surface.impact.write-policy-changed",
        K::IneligibilityAdded => "surface.impact.ineligibility-added",
        K::IneligibilityRemoved => "surface.impact.ineligibility-removed",
        K::IneligibilityRuleChanged => "surface.impact.ineligibility-rule-changed",
        K::IneligibilityMessageChanged => "surface.impact.ineligibility-message-changed",
    }
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
