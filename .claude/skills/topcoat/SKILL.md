---
name: topcoat
description: Distilled reference for the topcoat web framework (tokio-rs), pinned at v0.6.2. ALWAYS load this before writing, reviewing, or discussing ANY topcoat code — topcoat was announced 2026-07-22, after every Claude model's knowledge cutoff, so unassisted output WILL be hallucinated axum/Leptos-flavored APIs that do not exist. Covers routing, views/components, signals/reactivity, sessions, cookies, mail, tower interop, CLI, and testing.
---

# Topcoat v0.6.2 (tokio-rs) — distilled reference

- **Pinned version**: `topcoat` 0.6.2, distilled from the upstream repo at tag
  `topcoat-v0.6.2` (github.com/tokio-rs/topcoat) on **2026-08-20**.
- **Regenerate this skill whenever the dependency version bumps.** 0.6.2 shipped
  2026-08-18 with major breaking changes over 0.5.x (see below): any web
  tutorial, blog post, or LLM memory of "topcoat" is likely stale or invented.

## Hard rules

1. **Never write a topcoat API from memory.** Every macro, type, method, and
   feature-flag name must come from this skill's `references/` or from the real
   source. When unsure of a signature, read the vendored/downloaded crate source
   in `~/.cargo/registry/src/*/topcoat-*-0.6.2/` (or a checkout of the repo at
   `topcoat-v0.6.2`) rather than guessing.
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
| `topcoat::view` (`view`) | `view!`, `attributes!`, `class!`, `#[component]`, `View`/`Attributes`/`Class`, `Props` derive |
| `topcoat::router` (`router`) | `Router`, `#[page]`/`#[layout]`/`#[layer]`/`#[route]`, `module_router!`, `path_param!`, `#[query_params]`, `not_found!`, `href!`, `error`, `content` (Json/Form/Multipart/Sse/WebSocket/Sitemap), `tower` bridge, `OriginPolicy`, `BodyLimit` |
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
| Routes, pages, layouts, layers, module tree routing, path/query params, `href!`, errors/404/rewrites, request/response bodies, uploads (`Multipart`), body limits, `OriginPolicy`, **tower/axum interop** | `references/routing.md` |
| `view!` syntax, control flow, `#[component]`, props, keys, `attributes!`/`class!`, status codes & headers from views, concurrent rendering | `references/views-and-components.md` |
| `Cx`, request helpers, `app_context`, `Cx::with`, `#[memoize]`, auth-as-functions pattern | `references/context-and-state.md` |
| Signals, `$()` expressions, `@click`/`:value`, `#[procedure]`, `#[shard]`, `raw!` | `references/reactivity.md` |
| Cookies (jar, `cookie!`, signed/private, `CookieStore<T>`) and sessions (token/hash model, lifecycle, `TokenStore`) — the platform's auth | `references/sessions-and-cookies.md` |
| Sending email (`mail!`, transports, testing mail) | `references/mail.md` |
| CLI (`topcoat dev/fmt/ui/asset`), project scaffolding, features, serving, assets/Tailwind/fonts/icons, **writing tests** | `references/project-setup.md` |

## What changed in 0.6.x (0.6.0 2026-08-17, 0.6.2 2026-08-18)

Breaking vs 0.5.x — tutorials predating 2026-08-17 will show stale APIs:

- **`Cx` rework**: `CxBuilder` removed; `Cx` is detachable/clonable; request
  context is registered by *scoping* with `cx.with(value)` /
  `cx.with_many((a, b))` returning a child `Cx`.
- **Path params**: the old path-parameter *attribute* macro was replaced by the
  `path_param!` declaration macro (generates a Pascal-cased type read via
  `path_param::<T>(cx)`).
- **`#[memoize]`**: keys are 128-bit hashes (args need `Hash`, not `Clone+Eq`);
  borrowing `Option`/`Result` contents now requires explicit `#[memoize(as_ref)]`.
- **Router hardening**: global `OriginPolicy` (cross-origin state-changing
  requests 403 by default); request body limits (2 MiB default, `BodyLimit`
  layer); unmatched requests no longer run layers/layouts (use `not_found!`);
  unused-layer sanity check; `ContentTooLargeError` replaces `LengthLimitError`.
- **New in 0.6.x**: `href!` typed URL building (handler fn structs became
  traits to enable it), request rewriting (`error::rewrite`), sitemaps,
  concurrent view rendering, stable component identity + `key:`, and (0.6.2)
  mounting a topcoat router as an axum/tower service via `TowerService`.
- **Assets**: the bundle is now written *next to the executable it was scanned
  from* (`<target>/<profile>/assets`); bundle and binary must come from the
  same build.
