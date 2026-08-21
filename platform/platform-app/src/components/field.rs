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
