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
}
