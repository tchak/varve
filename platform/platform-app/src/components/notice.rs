//! A one-shot notice after an action: a light tint (soft green for a
//! confirmation, soft red for a refusal), never a solid block. Ours,
//! not vendored: the registry's `alert` carries neutral and
//! destructive tones only, and a confirmation deserves a colour of
//! its own.
//!
//! **Theme note.** The neutral theme has no success token, so the
//! tints come from Tailwind's palette with explicit dark-mode
//! values — the one place outside the theme tokens that sets a
//! colour (design/platform.md P.4, the schema editor; the a11y
//! contract asks for such overrides to be a recorded decision).
//! Both pairs clear WCAG AA contrast on their tint.
//!
//! The caller decides the live-region semantics (`role="status"` or
//! `role="alert"`) through `attrs`, as with `alert`.

use topcoat::{
    Result,
    view::{Attributes, Child, StaticClass, View, class, component, view},
};

/// What the notice reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoticeTone {
    /// An action that went through.
    #[default]
    Success,
    /// An action that was refused.
    Error,
}

impl NoticeTone {
    fn classes(self) -> StaticClass {
        match self {
            Self::Success => class!(
                "border-emerald-200 bg-emerald-50 text-emerald-900 \
                 dark:border-emerald-900 dark:bg-emerald-950 dark:text-emerald-100"
            ),
            Self::Error => class!(
                "border-rose-200 bg-rose-50 text-rose-900 \
                 dark:border-rose-900 dark:bg-rose-950 dark:text-rose-100"
            ),
        }
    }
}

const BASE: StaticClass = class!("w-full rounded-lg border px-4 py-3 text-sm");

/// The notice: tone classes, forwarded `attrs` (a `class` is appended),
/// the text as children.
#[component]
pub async fn notice(
    #[default] tone: NoticeTone,
    #[default] mut attrs: Attributes,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    Ok(view! {
        <div class=(class!(BASE, tone.classes(), attrs.remove("class"))) (attrs)>
            (child)
        </div>
    })
}

#[cfg(test)]
mod tests {
    use topcoat::view::{attributes, view};

    use super::{NoticeTone, notice};
    use crate::components::testing::render;

    #[test]
    fn tones_and_forwarded_role() {
        let html = render(|cx| {
            view! {
                cx =>
                notice(
                    tone: NoticeTone::Success,
                    attrs: attributes! { role="status" class="mt-2" },
                    "Saved your changes."
                )
            }
        });
        assert!(html.contains("role=\"status\""), "{html}");
        assert!(html.contains("bg-emerald-50"), "{html}");
        assert!(html.contains("mt-2"), "{html}");
        assert!(html.contains("Saved your changes."), "{html}");

        let html = render(|cx| {
            view! {
                cx =>
                notice(
                    tone: NoticeTone::Error,
                    attrs: attributes! { role="alert" },
                    "Refused."
                )
            }
        });
        assert!(html.contains("role=\"alert\""), "{html}");
        assert!(html.contains("bg-rose-50"), "{html}");
        assert!(!html.contains("emerald"), "{html}");
    }
}
