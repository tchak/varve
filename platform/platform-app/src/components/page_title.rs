// Not a registry component: created for platform-app, following the
// topcoat-ui component conventions (see `components.toml` — this file
// is deliberately absent from it, and `tests/registry_sync.rs` lists
// it as ours).

use topcoat::{
    Result,
    view::{Attributes, StaticClass, View, class, component, view},
};

/// The classes for the [`page_title`] heading.
const PAGE_TITLE: StaticClass = class!("text-2xl font-semibold tracking-tight text-foreground");

/// A page's main heading, rendered as an `<h1>`.
///
/// Every page renders exactly one, so assistive tech and tests can
/// anchor on the single `<h1>` inside `<main>`. The `attrs` (such as
/// `class`) are forwarded to the underlying `<h1>`; a `class` among
/// them is appended to the computed classes. Child nodes become the
/// heading's content.
///
/// ```ignore
/// view! {
///     page_title((title))
/// }
/// ```
#[component]
pub async fn page_title(#[default] mut attrs: Attributes, #[default] child: View) -> Result {
    view! { <h1 class=(class!(PAGE_TITLE, attrs.remove("class"))) (attrs)>(child)</h1> }
}
