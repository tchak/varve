// Not a registry component: created for platform-app, following the
// topcoat-ui component conventions (see `components.toml` — this file
// is deliberately absent from it, and `tests/registry_sync.rs` lists
// it as ours).

use topcoat::{
    Result,
    context::Cx,
    view::{Attributes, StaticClass, attributes, class, component, view},
};

// The `label` component is aliased: `#[component]` defines a unit
// struct per component, and the bare name would collide with this
// component's `label` parameter binding.
use super::{input::input, label::label as field_label};

/// The classes for the [`field`] wrapper.
///
/// A field stacks its caption, control, and error message in a tight
/// column; rows of fields come from the surrounding form's layout, not
/// from the field itself.
const FIELD: StaticClass = class!("flex flex-col gap-2");

/// The classes for a [`field`]'s error message.
const FIELD_ERROR: StaticClass = class!("text-sm text-destructive");

/// A labeled form field: a [`label`](super::label::label), an
/// [`input`], and an error slot.
///
/// The `id` links the pieces together: it becomes the input's `id`, the
/// label's `for`, and (suffixed `-error`) the error paragraph's `id`,
/// which the input references via `aria-describedby` when an error is
/// present, along with `aria-invalid`. The `label` and `error` are
/// display strings — pass localized text, never message ids. The
/// `attrs` (such as `type`, `name`, `value`, `required`, or
/// `autocomplete`) are forwarded to the underlying `<input>`; a `class`
/// among them is handled by [`input`] as usual.
///
/// ```ignore
/// view! {
///     field(
///         id: "signin-email",
///         label: email_label,
///         attrs: attributes! { type="email" name="email" required="" }
///     )
/// }
/// ```
#[component]
pub async fn field(
    cx: &Cx,
    /// The control's id, shared with the label and the error slot.
    #[into]
    id: String,
    /// The visible caption, as display text.
    #[into]
    label: String,
    /// Extra attributes for the `<input>` element.
    #[default]
    mut attrs: Attributes,
    /// The field's error, as display text, rendered under the control.
    #[default]
    error: Option<String>,
) -> Result {
    let error_id = format!("{id}-error");
    attrs.insert(cx, "id", id.as_str());
    if error.is_some() {
        attrs.insert(cx, "aria-invalid", "true");
        attrs.insert(cx, "aria-describedby", error_id.as_str());
    }
    view! {
        <div class=(FIELD)>
            field_label(attrs: attributes! { for=(id.as_str()) }, (label))
            input(attrs: attrs)
            if let Some(error) = &error {
                <p id=(error_id.as_str()) class=(FIELD_ERROR)>(error)</p>
            }
        </div>
    }
}

#[cfg(test)]
mod tests {
    use topcoat::view::{attributes, view};

    use super::field;
    use crate::components::testing::render;

    /// The rendered `<input ...>` tag, extracted so attribute
    /// assertions cannot accidentally match the label or the error
    /// paragraph.
    fn input_tag(html: &str) -> &str {
        let start = html.find("<input").expect("an <input> rendered");
        let end = html[start..].find('>').expect("the <input> tag closes");
        &html[start..start + end + 1]
    }

    #[test]
    fn links_label_to_input_and_forwards_attrs() {
        let html = render(async |cx| {
            view! {
                cx =>
                field(
                    id: "signin-email",
                    label: "Email address",
                    attrs: attributes! { type="email" name="email" class="mt-4" }
                )
            }
        });

        // The label carries the caption and points at the control.
        assert!(html.contains(r#"for="signin-email""#), "{html}");
        assert!(html.contains("Email address"), "{html}");
        let tag = input_tag(&html);
        assert!(tag.contains(r#"id="signin-email""#), "{tag}");
        assert!(tag.contains(r#"type="email""#), "{tag}");
        assert!(tag.contains(r#"name="email""#), "{tag}");
        // The caller's class merged into the input's single class
        // attribute instead of replacing or duplicating it.
        assert_eq!(tag.matches("class=").count(), 1, "{tag}");
        assert!(tag.contains("mt-4"), "{tag}");
    }

    #[test]
    fn without_error_renders_no_error_wiring() {
        let html = render(async |cx| {
            view! { cx => field(id: "signin-email", label: "Email address") }
        });

        assert!(!html.contains("aria-invalid"), "{html}");
        assert!(!html.contains("aria-describedby"), "{html}");
        assert!(!html.contains("signin-email-error"), "{html}");
        assert!(!html.contains("<p"), "{html}");
    }

    #[test]
    fn with_error_wires_aria_and_renders_the_message() {
        let html = render(async |cx| {
            view! {
                cx =>
                field(
                    id: "signin-email",
                    label: "Email address",
                    error: Some("Required field.".to_owned())
                )
            }
        });

        let tag = input_tag(&html);
        assert!(tag.contains(r#"aria-invalid="true""#), "{tag}");
        assert!(
            tag.contains(r#"aria-describedby="signin-email-error""#),
            "{tag}"
        );
        // The error paragraph carries the referenced id and the text.
        assert!(html.contains(r#"<p id="signin-email-error""#), "{html}");
        assert!(html.contains("Required field."), "{html}");
    }
}
