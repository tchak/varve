// Not a registry component: created for platform-app, following the
// topcoat-ui component conventions (see `components.toml` — this file
// is deliberately absent from it, and `tests/registry_sync.rs` lists
// it as ours).

use topcoat::{
    Result,
    view::{attributes, component, view},
};

use crate::components::breadcrumb::{
    breadcrumb_item, breadcrumb_link, breadcrumb_list, breadcrumb_page, breadcrumb_separator,
};

/// One step of a [`breadcrumbs`] trail.
pub struct Crumb {
    pub label: String,
    /// `None` marks the current page: rendered as plain text with
    /// `aria-current="page"`, never a link.
    pub href: Option<String>,
}

impl Crumb {
    /// An ancestor page: a link.
    pub fn link(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: Some(href.into()),
        }
    }

    /// The current page: the trail's last, unlinked item.
    pub fn here(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: None,
        }
    }
}

/// Breadcrumb navigation (platform P.4 *Breadcrumb navigation*):
/// the page's ancestry as data, rendered through the vendored
/// [`crate::components::breadcrumb`] family and placed directly
/// above the `<h1>`. Every ancestor is a link; the current page is
/// a plain `aria-current="page"` item; the separators are
/// `aria-hidden`. `label` is the localized landmark name
/// (`nav.breadcrumb`), passed in so the component stays
/// presentational — the `<nav>` is ours rather than the registry's
/// `breadcrumb`, whose `aria-label` is a hardcoded English
/// "breadcrumb" (upstream: topcoat-ui should take the label as a
/// prop).
#[component]
pub async fn breadcrumbs(label: String, crumbs: Vec<Crumb>) -> Result {
    view! {
        <nav aria-label=(label.as_str()) data-breadcrumbs="">
            breadcrumb_list(
                for (at, crumb) in crumbs.iter().enumerate() {
                    if at > 0 {
                        breadcrumb_separator()
                    }
                    breadcrumb_item(
                        match &crumb.href {
                            Some(href) => {
                                breadcrumb_link(
                                    attrs: attributes! { href=(href.as_str()) },
                                    (crumb.label.as_str())
                                )
                            }
                            None => {
                                breadcrumb_page((crumb.label.as_str()))
                            }
                        }
                    )
                }
            )
        </nav>
    }
}

#[cfg(test)]
mod tests {
    use topcoat::view::view;

    use super::{Crumb, breadcrumbs};
    use crate::components::testing::render;

    #[test]
    fn ancestors_link_and_the_current_page_carries_aria_current() {
        let html = render(async |cx| {
            view! {
                cx =>
                breadcrumbs(
                    label: "Breadcrumb".to_owned(),
                    crumbs: vec![
                        Crumb::link("Organizations", "/organizations"),
                        Crumb::link("Préfecture", "/organizations/abc"),
                        Crumb::here("Teams"),
                    ]
                )
            }
        });
        assert!(html.contains("aria-label=\"Breadcrumb\""), "{html}");
        assert!(html.contains("<ol"), "{html}");
        assert!(html.contains("href=\"/organizations\""), "{html}");
        assert!(html.contains(">Organizations</a>"), "{html}");
        assert!(html.contains("href=\"/organizations/abc\""), "{html}");
        assert!(html.contains(">Préfecture</a>"), "{html}");
        assert!(html.contains("aria-current=\"page\""), "{html}");
        assert!(!html.contains("href=\"Teams\""), "{html}");
        // Separators are decoration only (their chevron icon adds
        // its own aria-hidden, so count the separator items).
        assert_eq!(
            html.matches("<li aria-hidden=\"true\"").count(),
            2,
            "{html}"
        );
    }

    #[test]
    fn a_single_crumb_renders_without_a_separator() {
        let html = render(async |cx| {
            view! {
                cx =>
                breadcrumbs(
                    label: "Breadcrumb".to_owned(),
                    crumbs: vec![Crumb::here("Organizations")]
                )
            }
        });
        assert!(!html.contains("aria-hidden"), "{html}");
        assert!(html.contains("aria-current=\"page\""), "{html}");
    }
}
