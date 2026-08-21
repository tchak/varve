fn main() {
    // The theme stylesheet installed by `topcoat ui init` is the
    // Tailwind input: it carries the `@import "tailwindcss"`
    // directive, the theme's design tokens, and the `@source`
    // directive scanning `src/**/*.rs` for utility classes. The
    // standalone Tailwind CLI (pinned by topcoat) is downloaded on
    // first build and cached under `<target>/topcoat/cache/tailwind`;
    // the output lands at `$OUT_DIR/tailwind.css`, which
    // `tailwind::stylesheet!()` declares as an asset.
    topcoat::tailwind::BuildConfig::new()
        .input("styles.css")
        .render()
        .unwrap();

    // The vendored `select` component draws its chevron and checkmark
    // with `iconify_icon!("feather:...")`, which reads the staged
    // `feather` set at compile time. Staging downloads the set's
    // `@iconify-json/feather` package once and caches it under
    // `<target>/topcoat/cache/iconify`; builds stay offline after
    // that.
    topcoat::icon::iconify::BuildConfig::new()
        .icon_set("feather")
        .stage()
        .unwrap();
}
