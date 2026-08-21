# Project setup, CLI, assets, testing

Sources: `crates/topcoat/docs/{getting_started,asset,tailwind,font,icon,ui}.md`,
`crates/topcoat-cli/docs/fmt.md`, `demos/coffee-shop/`, framework test code.
Verified at `topcoat-v0.6.2`.

## New project

```sh
cargo new my-app && cd my-app
cargo add topcoat                                   # 0.6.2
cargo add tokio --features rt-multi-thread,macros
# platform extras for this repo:
cargo add topcoat --features mail,mail-smtp,multipart,tower
```

```rust
use topcoat::{Result, router::{Router, RouterBuilderDiscoverExt, page}, view::{component, view}};

#[tokio::main]
async fn main() {
    topcoat::start(Router::builder().discover().build()).await.unwrap();
}

#[page("/")]
async fn home() -> Result {
    view! {
        <!DOCTYPE html>
        <html>
            <head>
                <title>"Hello world"</title>
                topcoat::dev::script()          // hot-reload script (dev server)
            </head>
            <body>hello(name: "World")</body>
        </html>
    }
}

#[component]
async fn hello(name: &str) -> Result {
    view! { <h1>"Hello, " (name) "!"</h1> }
}
```

## The topcoat CLI

`cargo install topcoat-cli` → one `topcoat` binary (also `cargo topcoat …`).

- **`topcoat dev`** — builds, bundles assets, serves; watches sources and
  rebuilds/rebundles/restarts. Pages including `topcoat::dev::script()` reload
  automatically; press `r` for a manual rebuild. `HOST=0.0.0.0 PORT=8080
  topcoat dev` overrides the bind (defaults `127.0.0.1:3000`). Plain
  `cargo run` also works, minus assets/reload.
- **`topcoat fmt`** — formats topcoat macro bodies (`view!` etc.) in place,
  complementing `rustfmt` (which leaves macro bodies alone). Args: files/dirs;
  `--stdin` for editors; `--macros view,class` to restrict. A repo-root
  **`Topcoat.toml` is just a marker file for `topcoat fmt` editor
  integration** — it is not an app config file.
- **`topcoat asset list | bundle | clean`** — manual bundling; `--bin`/
  `--package` to pick a target, same profile flags as `cargo build`
  (`--release`, `--profile p`), `--out dist/assets` for a custom location.
- **`topcoat ui init | add | update | remove`** — vendors themeable components
  (source copied into `src/components/`, tracked in `components.toml`; theme
  CSS in `styles.css` which doubles as the Tailwind input). Needs features
  `ui` + `tailwind` (+ `font-fontsource` for the themes' fonts). Feature `ui`
  exposes `topcoat_ui::Registry` for tests that pin vendored components to the
  registry (see `demos/coffee-shop/tests/registry_sync.rs`).
- 0.6.0+: the CLI warns on a version mismatch between `topcoat` and the CLI.

## Feature flags (facade crate `topcoat`)

Default: `asset compression cookie discover font icon router runtime serve
session view`. Opt-in: `mail`, `mail-smtp`, `multipart`, `sse`, `sitemap`,
`tailwind`, `tower`, `ui`, `websocket`, `htmx`, `alpine-ajax`, `datastar`,
`font-fontsource`, `icon-iconify`, `full`. Only `serve` pulls tokio/hyper;
build with `default-features = false` (keeping e.g. `router`, `view`) and call
`Router::handle` on serverless/wasm targets.

## Assets

`asset!` declares a content-addressed static file; the bundler scans the
compiled binary for declarations (an unused handle can be optimized out and is
then skipped):

```rust
use topcoat::asset::{Asset, asset};

const FERRIS: Asset = asset!("./ferris.png");       // relative to this source file
// "assets/logo.png" → relative to CARGO_MANIFEST_DIR; absolute paths as-is;
// "https://…" → downloaded and cached at build time.
// Options: rename:, extension:, checksum: "sha256:…", content_type:.

view! { <img src=(FERRIS)> }   // renders /_topcoat/assets/ferris-<hash>.png
```

Load the bundle on the router (or rendering an `Asset` panics — treat as
build/deploy mismatch):

```rust
use topcoat::asset::{AssetBundle, RouterBuilderAssetExt};
Router::builder().discover().assets(AssetBundle::load().unwrap()).build();
// custom location: AssetBundle::load_dir("dist/assets")
```

- Bundle location (0.6.0): `<cargo-target>/<profile>/assets`, next to the
  executable it was scanned from; **bundle and binary must come from the same
  build** (asset IDs embed OUT_DIR paths).
- CDN hosting: `.assets(AssetConfig::hosted_at("https://cdn.example.com/assets",
  AssetBundle::load().unwrap()))` — no asset routes; you upload the bundle. On
  wasm, embed `manifest.toml` via `Manifest::parse(include_str!(…))`.

## Tailwind

Thin wrapper over the standalone Tailwind CLI (pinned 4.3.2, downloaded and
cached; no Node). Enable `tailwind` on both the dependency and the
build-dependency:

```toml
[dependencies]
topcoat = { version = "0.6.2", features = ["tailwind"] }
[build-dependencies]
topcoat = { version = "0.6.2", default-features = false, features = ["tailwind"] }
```

```rust
// build.rs
fn main() { topcoat::tailwind::BuildConfig::new().render().unwrap(); }
// options: .input("styles.css"), .cwd("src"), .version_checksum(…),
// .executable("tailwindcss") / .executable_env("TAILWIND_CLI")

// layout:
view! { <link rel="stylesheet" href=(tailwind::stylesheet!())> }
// stylesheet!() == asset!(concat!(env!("OUT_DIR"), "/tailwind.css"))
```

Class scanning is Tailwind's own, from the package root, respecting
`.gitignore` — literal `class="…"` in `view!` is found; dynamically assembled
class strings are invisible.

## Fonts and icons (brief)

- `font!` declares `@font-face` blocks in Rust (`const ORBITRON: Font =
  font! { "Orbitron", @font-face { src: url("…") format("woff2"); … } };`);
  registered by `.discover()` (or `.font(F)`); rendered in `<head>` with
  `topcoat::font::link(font: ORBITRON)`; family name via `.family()`.
  `url(...)` accepts an `Asset` to self-host. Feature `font-fontsource` adds
  `fontsource_font!(GEIST, host: Asset)` pulling a family from the Fontsource
  catalog (see `demos/coffee-shop/src/app.rs`).
- `icon`: `IconData::unescaped_unchecked(ViewBox::new(0.0,0.0,24.0,24.0),
  r#"<path …/>"#)` rendered via the `icon` component:
  `icon(data: TRASH, label: "trash")` (inline `<svg>`, 1em, currentColor;
  `size:` fixes dimensions, `attrs:` forwards attributes; no `label` = hidden
  from assistive tech). Feature `icon-iconify` vendors sets from Iconify at
  build time.

## Testing

Two levels, both used by upstream:

**1. Router-level** — `Router::handle` needs no listener. Topcoat re-exports
the `http` types it uses (`topcoat::router::{Method, StatusCode, HeaderMap,
HeaderName, HeaderValue, Uri, header}`; `request::Request<T = Body>` is
`http::Request<T>`), so tests need no direct `http` dependency:

```rust
use topcoat::router::{Body, Method, Router, StatusCode, request::Request, to_bytes};

fn request(method: Method, path: &str) -> Request {
    Request::builder().method(method).uri(path).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn health_works() {
    let router = Router::builder().route(health).build();
    let response = router.handle(request(Method::GET, "/api/health")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let (_parts, body) = response.into_parts();
    let bytes = to_bytes(body, usize::MAX).await.unwrap();   // Result<Bytes>
    assert_eq!(&bytes[..], b"ok");
}
```

**2. Cx-level** — unit-test `cx` functions with `CxTestBuilder`
(`topcoat::context::CxTestBuilder`): `.app_context(v)`, `.request_context(v)`,
`.build()` → `Cx`. Pair with `MemoryTransport` for mail assertions (see
`references/mail.md`) and in-memory app-context fakes for storage.

Upstream layout: unit tests live in `#[cfg(test)] mod tests` inside crate
sources; `demos/coffee-shop/tests/` holds integration tests. Examples in
`examples/` are one-feature binaries (`examples/session`, `examples/mail`,
`examples/module-router`, …) — good templates.

## Serving recap

- `topcoat::start(router)` — `HOST`/`PORT` env, default `127.0.0.1:3000`.
- `topcoat::serve(listener, router)` — bring your own `TcpListener`/
  `UnixListener` (remove a stale socket file before binding).
- `topcoat::serve_until` — like `serve` with a shutdown future.
- `Router::handle(request)` — no `serve` feature needed (tests, wasm,
  serverless).
- Embedding under axum/tower: `TowerService::new(router)` — see
  `references/routing.md` § Tower interop.


## Field notes: tailwind + `topcoat ui` mechanics (verified 2026-08-21)

The skill's original pass summarized these to command names; here is
how they actually work at 0.6.2:

- **CLI**: `cargo install topcoat-cli --version 0.6.2 --locked` —
  pin to the runtime dep's version (the CLI warns on mismatch).
- **`topcoat ui` is shadcn-style vendoring, not a crate**:
  `topcoat ui init --package <pkg> --theme neutral` writes
  `components.toml` (install state: theme + per-component sha256) and
  `styles.css` (theme tokens + `@import "tailwindcss"` +
  `@source "./src/**/*.rs"`). `topcoat ui add button ...` copies
  sources from the `topcoat-ui-registry` crate (pulled transitively by
  the `ui` cargo feature — that feature does nothing else) into
  `src/components/`, appending `pub mod x;` to `src/components.rs`.
  The CLI finds the registry via `cargo metadata` →
  `[package.metadata.topcoat-ui].registry`. Pin vendored files with a
  registry-sync test (see the coffee-shop demo; in a non-member
  workspace locate the registry via `cargo metadata`).
- **Catalog conventions** (match these for hand-written components):
  `#[component] pub async fn`, file = snake_case component name,
  `StaticClass` consts via `class!`, `#[default] mut attrs:
  Attributes` forwarded with caller `class` merged, `#[into]` string
  props carrying display text only. There is NO Props derive in the
  catalog. Gotcha: `#[component]` defines a unit struct per component,
  so a parameter named like an imported component collides — alias the
  import (`label as field_label`).
- **Tailwind**: runtime dep features `["tailwind", "ui"]` plus a
  build-dep `topcoat { default-features = false, features =
  ["tailwind"] }`; `build.rs` is
  `BuildConfig::new().input("styles.css").render()`. First build
  downloads the standalone Tailwind CLI (~76MB) into
  `<target>/topcoat/cache/tailwind/` (file-locked, workspace-shared,
  gone on `cargo clean`; offline once warm). Escape hatches:
  `.executable("tailwindcss")` / `.executable_env("TAILWIND_CLI")`.
- **Assets**: `tailwind::stylesheet!()` ≡
  `asset!(concat!(env!("OUT_DIR"), "/tailwind.css"))`. `topcoat asset
  bundle --package <bin-pkg>` writes `assets/` NEXT TO the scanned
  executable; `AssetBundle::load()` reads `<exe_dir>/assets`; bundle
  and binary must come from the same build. Rendering an `Asset` with
  no `AssetConfig` in app context PANICS by design — for routers that
  must also run bundle-less (tests, plain `cargo run`), resolve via
  `try_app_context::<AssetConfig>` + `config.get(asset)` and skip the
  `<link>` when absent.
- `Topcoat.toml` is only a marker for `topcoat fmt` editor
  integration. `topcoat fmt` formats `view!` bodies; rustfmt leaves
  them alone — run both.
