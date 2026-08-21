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

#[cfg(test)]
mod tests {
    use topcoat::view::{attributes, view};

    use super::site_header;
    use crate::components::testing::render;

    // The signed-in/signed-out navigation is the `shell` layout's
    // contract (it builds the child nodes), covered at router level
    // in `tests/app/`; the component's own contract is the landmarks,
    // the brand link, the child slot, and attrs forwarding.

    #[test]
    fn renders_landmarks_brand_link_and_children() {
        let html = render(async |cx| {
            view! {
                cx =>
                site_header(
                    brand_label: "Varve",
                    brand_href: "/",
                    <a href="/signin">"Sign in"</a>
                )
            }
        });

        assert!(html.contains("<header"), "{html}");
        assert!(html.contains("<nav"), "{html}");
        // The brand link points home and carries the label.
        assert!(html.contains(r#"<a href="/""#), "{html}");
        assert!(html.contains("Varve"), "{html}");
        // The child landed in the navigation area.
        assert!(html.contains(r#"<a href="/signin""#), "{html}");
        assert!(html.contains("Sign in"), "{html}");
    }

    #[test]
    fn forwards_attrs_and_merges_class_onto_the_header() {
        let html = render(async |cx| {
            view! {
                cx =>
                site_header(
                    brand_label: "Varve",
                    brand_href: "/",
                    attrs: attributes! { data-test="header" class="sticky" }
                )
            }
        });

        let start = html.find("<header").expect("a <header> rendered");
        let end = html[start..].find('>').expect("the <header> tag closes");
        let tag = &html[start..start + end + 1];
        assert!(tag.contains(r#"data-test="header""#), "{tag}");
        // The caller's class merged into the header's single class
        // attribute instead of replacing or duplicating it.
        assert_eq!(tag.matches("class=").count(), 1, "{tag}");
        assert!(tag.contains("sticky"), "{tag}");
    }
}
