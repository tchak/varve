// Not a registry component: created for platform-app, following the
// topcoat-ui component conventions (see `components.toml` — this file
// is deliberately absent from it, and `tests/registry_sync.rs` lists
// it as ours).

use topcoat::{
    Result,
    view::{Attributes, StaticClass, View, class, component, view},
};

/// The classes for the [`site_header`] bar.
const HEADER: StaticClass = class!("border-b border-border bg-background");

/// The classes for the navigation row inside the bar.
const NAV: StaticClass = class!("mx-auto flex w-full max-w-3xl items-center gap-4 px-6 py-3");

/// The site-wide top bar: a brand link home and a right-aligned
/// navigation area, inside `<header>`/`<nav>` landmarks.
///
/// The `brand_label` is display text (the site name) linked to
/// `brand_href`. Child nodes become the right-aligned navigation items
/// — links, buttons, or the signed-in state. The `attrs` (such as
/// `class`) are forwarded to the underlying `<header>`; a `class`
/// among them is appended to the computed classes.
///
/// ```ignore
/// view! {
///     site_header(
///         brand_label: title,
///         brand_href: href!(home).resolve(cx),
///         <a href=(href!(signin))>(sign_in_label)</a>
///     )
/// }
/// ```
#[component]
pub async fn site_header(
    /// The brand text, linked to `brand_href`.
    #[into]
    brand_label: String,
    /// Where the brand links to, typically the home page.
    #[into]
    brand_href: String,
    /// Extra attributes for the `<header>` element.
    #[default]
    mut attrs: Attributes,
    /// The navigation items, laid out right-aligned.
    #[default]
    child: View,
) -> Result {
    view! {
        <header class=(class!(HEADER, attrs.remove("class"))) (attrs)>
            <nav class=(NAV)>
                <a href=(brand_href) class="font-semibold text-foreground">
                    (brand_label)
                </a>
                <div class="ml-auto flex flex-wrap items-center gap-2">(child)</div>
            </nav>
        </header>
    }
}
