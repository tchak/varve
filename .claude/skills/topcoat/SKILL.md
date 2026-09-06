---
name: topcoat
description: Distilled reference for the topcoat web framework (tokio-rs), pinned at v0.7.0. ALWAYS load this before writing, reviewing, or discussing ANY topcoat code — topcoat was announced 2026-07-22, after every Claude model's knowledge cutoff, so unassisted output WILL be hallucinated axum/Leptos-flavored APIs that do not exist. Covers routing, views/components, lazy views and streaming (live!/emit!/suspense/error_boundary), signals/reactivity, sessions, cookies, mail, tower interop, CLI, and testing.
---

# Topcoat v0.7.0 (tokio-rs) — distilled reference

- **Pinned version**: `topcoat` 0.7.0, distilled from the upstream repo at tag
  `v0.7.0` (github.com/tokio-rs/topcoat) on **2026-09-06**. Requires **Rust
  1.98** (`rust-version = "1.98"`).
- **Regenerate this skill whenever the dependency version bumps.** 0.7.0
  shipped 2026-09-04 (crates.io 2026-09-05) with one large breaking change
  over 0.6.x — lazy views and streaming SSR (see below) — on top of 0.6's
  breaking rework: any web tutorial, blog post, or LLM memory of "topcoat" is
  likely stale or invented.

## Hard rules

1. **Never write a topcoat API from memory.** Every macro, type, method, and
   feature-flag name must come from this skill's `references/` or from the real
   source. When unsure of a signature, read the vendored/downloaded crate source
   in `~/.cargo/registry/src/*/topcoat-*-0.7.0/` (or a checkout of the repo at
   `v0.7.0`) rather than guessing.
2. Topcoat is **not axum** (no extractor-style `State<T>`/`Path<T>` handler
   params; path/query/state are read from `cx: &Cx`) and **not Leptos** (no
   client wasm, no `#[server]`; reactivity is a compiled Rust→JS expression
   subset). Do not import patterns from either.
3. Upstream doc paths cited below (e.g. `crates/topcoat-router/docs/tower.md`)
   are paths inside the topcoat repo — each crate's `docs/` directory holds its
   guides, and `crates/*/macro/docs/` holds per-macro references.

## Crate / concept map

`topcoat` is the facade crate; app code depends on it only. Feature-gated
modules re-export the implementation crates:

| Module (feature) | What lives there |
|---|---|
| `topcoat::context` | `Cx`, `app_context`, `request_context`, `#[memoize]`, `CxTestBuilder` (topcoat-core) |
| `topcoat::view` (`view`) | `view!`, `attributes!`, `class!`, `#[component]`, **`live!`/`emit!`/`EmitToken`**, **`suspense`/`error_boundary`** components, `View` (a **trait**), `ViewExt` (`.boxed()`/`.first()`/`.single()`), `BoxView`, `Child`, `Attributes`/`Class`/`StaticClass`, `Props` derive |
| `topcoat::router` (`router`) | `Router`, `#[page]`/`#[layout]`/`#[layer]`/`#[route]`, `Slot<'a>` (= `Child<'a>`), `module_router!`, `path_param!`, `#[query_params]`, `not_found!`, `href!` (+ `Href::is_current`), `error`, `content` (Json/Form/Multipart/Sse/WebSocket/Sitemap), `response::{IntoResponse, AsyncIntoResponse}`, `tower` bridge, `OriginPolicy`, `BodyLimit` |
| `topcoat::start/serve/serve_until` (`serve`) | tokio+hyper serving; the only IO-dependent part |
| `topcoat::runtime` (`runtime`) | signals, `$()`/`expr!`, `@` event handlers, `:` binds, `#[procedure]`, `#[shard]`, browser script |
| `topcoat::cookie` (`cookie`) | jar via `cookies(cx)`, `cookie!`, signed/private jars, `CookieStore<T>` |
| `topcoat::session` (`session`) | bring-your-own-storage token/hash sessions: `start`/`stop`/`refresh`/`rotate`/`token_hash` |
| `topcoat::mail` (`mail`, `mail-smtp`) | `mail!`/`Mail`, `send`, SMTP/File/Memory transports |
| `topcoat::asset` (`asset`) | `asset!`, `AssetBundle`, content-hashed URLs under `/_topcoat/assets` |
| `topcoat::tailwind` (`tailwind`) | build-script wrapper over the standalone Tailwind CLI; `tailwind::stylesheet!()` |
| `topcoat::font` / `topcoat::icon` | `font!`/`fontsource_font!` + `font::link`; `icon` component + Iconify vendoring |
| htmx / alpine-ajax / datastar | request/response helpers for those client libs (not used by this project) |

Default features: `asset compression cookie discover font icon router runtime
serve session view`. Off by default: `mail mail-smtp multipart sse sitemap
tailwind tower ui websocket htmx alpine-ajax datastar font-fontsource
icon-iconify` (and `full`). For this repo's platform:

```sh
cargo add topcoat --features mail,mail-smtp,multipart,tower
cargo add tokio --features rt-multi-thread,macros
```

## Which reference to open

| Task | Open |
|---|---|
| Routes, pages, layouts (`Slot`), layers, module tree routing, path/query params, `href!`/`is_current`, errors/404/rewrites, **streaming & commit rules**, request/response bodies, uploads (`Multipart`), body limits, `OriginPolicy`, **tower/axum interop** | `references/routing.md` |
| `view!` syntax, **lazy views**, control flow, `#[component]`, props, `Child`, keys, `.boxed()`, `attributes!`/`class!`, status codes & headers from views, concurrent rendering, **`live!`/`emit!`/`suspense`/`error_boundary`**, rendering a view in a test | `references/views-and-components.md` |
| `Cx`, request helpers, `app_context`, `Cx::with`, `#[memoize]`, auth-as-functions pattern | `references/context-and-state.md` |
| Signals, `$()` expressions, `@click`/`:value`, `#[procedure]`, `#[shard]`, `raw!` | `references/reactivity.md` |
| Cookies (jar, `cookie!`, signed/private, `CookieStore<T>`) and sessions (token/hash model, lifecycle, `TokenStore`) — the platform's auth; **when writes are still legal** | `references/sessions-and-cookies.md` |
| Sending email (`mail!`, transports, testing mail) | `references/mail.md` |
| CLI (`topcoat dev/fmt/ui/asset`), project scaffolding, features, serving, assets/Tailwind/fonts/icons, **writing tests** | `references/project-setup.md` |

## What changed in 0.7.0 (2026-09-04)

The one breaking item is **streaming SSR** (PR #373), and it touches every
handler and component signature:

- **Views are lazy.** `view!` no longer renders where it is written: it
  evaluates to a value implementing the `View` **trait** (an `async move`-like
  value that *moves* every captured variable) and renders only when it becomes
  a response or is interpolated into another view. Consequently every
  component, page, layout, and shard returns **`Result<impl View>`** and wraps
  its template: `Ok(view! { … })`. `topcoat::Result<T, E = Error>` has **no
  default `T`** any more — a bare `-> Result` is a compile error, not a view.
- **Children**: `child: View` became **`#[default] child: Child<'_>`**
  (`topcoat::view::Child`). View nodes passed to a component desugar to
  `child: view! { … }.into()`; an explicit `child:` argument needs `.into()`
  or `Child::new(view)`.
- **Layouts** take `slot: Slot<'_>` (`topcoat::router::Slot`, an alias of
  `Child`) and interpolate `(slot)` — no `?`. A layout can no longer match on
  the slot's error: wrap `(slot)` in **`error_boundary`** to catch/brand
  errors (`NotFoundError`, `ForbiddenError`, `UnauthorizedError`…).
- **`#[component(boxed)]` is gone.** A recursive component (or a function
  returning different `view!`s from several `return` sites) erases its view
  type with `.boxed()` (`ViewExt`) on the returned view; one erased type per
  cycle suffices.
- **Live regions**: `live! { … }` bodies are async Rust; `emit! { … }` renders
  markup into the region (each emission replaces the previous, returns
  `Result<EmitToken>`). `suspense(fallback: …, child)` and
  `error_boundary(fallback: |error| …, child)` are the prepackaged shapes.
- **Commit rules**: the router awaits the page's *first* content inside the
  handler (status code, headers, cookie/session writes all happen then), then
  streams later emissions. After commit: status/headers cannot change, a
  cookie write **panics**, an error renders in place, and a redirect thrown
  from a streaming region becomes a **client-side navigation**.
- Routes return `T: AsyncIntoResponse` (every `IntoResponse` is one; views
  are the async case).
- `Href::is_current(cx)`, `Route::is_current`, `Page::is_current` (mark the
  current nav link); `HrefTarget` implemented for `&T` (dyn targets).
- Fixes: `#[memoize]` with a borrowed async arg no longer hits an
  `AsyncFnOnce` lifetime error; `#[procedure]` boolean results preserved.
- CLI 0.7: `topcoat dev` retries the port on init and no longer reloads over
  in-flight navigations; `topcoat fmt` also formats `live`/`emit`.
- topcoat-ui registry: icons switched from `feather:` to **`lucide:`**, the
  neutral theme's `--primary`/`--ring` darkened slightly. Re-vendor with
  `topcoat ui add <name> --overwrite`.
- Internals: `itoa` and `prettyplease` dropped; Rust 1.98.

### 0.6.x recap (0.6.0 2026-08-17, 0.6.2 2026-08-18)

`CxBuilder` removed (`cx.with(value)` scoping); `path_param!` replaces the
path-parameter attribute; `#[memoize]` keys are 128-bit hashes (`Hash` args,
explicit `#[memoize(as_ref)]`); global `OriginPolicy`, request body limits
(`BodyLimit`, `ContentTooLargeError`), unmatched requests skip layers/layouts
(`not_found!`); `href!`, `error::rewrite`, sitemaps, concurrent rendering,
stable component identity + `key:`, `TowerService` (0.6.2); asset bundle
written next to the scanned executable.
