# Routing (topcoat-router)

Sources: `crates/topcoat/docs/router.md`, `crates/topcoat-router/docs/`
(`module_router.md`, `error.md`, `tower.md`, `content.md`, `content/*.md`),
`crates/topcoat-router/macro/docs/` (one page per macro). All verified against
tag `topcoat-v0.6.2`.

Contents: [Basics](#basics) · [Paths](#path-syntax) · [Pages](#pages) ·
[Layouts](#layouts) · [Layers](#layers) · [API routes](#api-routes) ·
[module_router!](#module_router) · [Path params](#path-parameters) ·
[Query params](#query-parameters) · [href!](#building-urls-with-href) ·
[Errors](#errors) · [Origin policy](#cross-origin-requests-originpolicy) ·
[Bodies & responses](#request-bodies-and-responses) · [Uploads](#multipart-uploads) ·
[Tower/axum interop](#tower-interop-the-escape-hatch)

## Basics

Build a `Router` with `Router::builder()`, register handlers, `.build()`, then
serve with `topcoat::start(router)` (binds `HOST`/`PORT`, default
`127.0.0.1:3000`) or `topcoat::serve(listener, router)` (any `Listener`:
`TcpListener`, or Unix `UnixListener` behind a reverse proxy). Without the
`serve` feature, `Router::handle(request) -> Response` dispatches directly (used
for tests and serverless/wasm).

```rust
use topcoat::router::{Router, RouterBuilderDiscoverExt};

pub fn router() -> Router {
    Router::builder().discover().build()
}

#[tokio::main]
async fn main() {
    topcoat::start(router()).await.unwrap();
}
```

Registration is either **manual** (`.page(home).layout(shell).layer(timing)
.route(health)`) or **auto-discovery**: `.discover()` (feature `discover`, on by
default) collects every annotated item at link time — pages, layouts, layers,
routes, plus fonts, procedures, shards. Values are always registered by hand:
`.assets(AssetBundle::load().unwrap())`, `.app_context(db)`, `.cookies()`,
`.sessions(config)`, `.mail(config)`, `.origin_policy(policy)`, `.base_url(url)`.
A missing value registration surfaces as a **panic on first use** naming the
missing type, not a compile error.

## Path syntax

- `/users` static; `/users/{id}` one dynamic segment; `/docs/{*path}` catch-all
  tail (≥1 segment, must be last).
- `/(marketing)/pricing` — group `(name)`: participates in layout/layer
  matching but is stripped from the served URL (serves `/pricing`).

## Pages

```rust
use topcoat::{Result, router::page, view::view};

#[page("/")]
async fn home() -> Result {
    view! { <h1>"Home"</h1> }
}
```

- `GET` by default; override with `#[page(POST "/signup")]`, `[GET, POST]`, or `*`.
- Signature: async, returns `Result` (= `Result<View>`); may take `cx: &Cx`
  and/or **one** body param implementing `FromRequest` (e.g.
  `Form(input): Form<Signup>`), in either order.
- A page doubles as a component: `contact(body: Form(query))` renders it inline
  with the already-parsed body as a `body:` prop.

## Layouts

A layout wraps every page whose path starts with the layout's path; multiple
matches nest least-specific outermost. It receives the inner render as
`slot: Result` (i.e. `Result<View>`) — so it can inspect/replace errors before
they become responses:

```rust
use topcoat::{Result, router::layout, view::view};

#[layout("/")]
async fn root_layout(slot: Result) -> Result {
    view! {
        <!DOCTYPE html>
        <html><body>
            <nav><a href="/">"Home"</a></nav>
            (slot?)
        </body></html>
    }
}
```

Only `slot` and optional `cx: &Cx` params are accepted. A layout also doubles as
a component: `root_layout(slot: Ok(content))`.

## Layers

```rust
use topcoat::{Result, context::Cx, router::{Body, Next, layer, response::Response}};

#[layer("/api")]
async fn api_log(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
    let response = next.run(cx, body).await?;
    println!("API response: {}", response.status());
    Ok(response)
}
```

- Same prefix rule as layouts, matched against the handler's **registered path
  segment-by-segment at build time** (not the request URL): a layer at
  `/docs/admin` does NOT wrap a page at `/docs/{x}`; groups count, so a layer at
  `/dashboard` does not wrap `/(auth)/dashboard`.
- Returning without calling `next.run` short-circuits.
- Since 0.6.0, a 404/405 does **not** run path-scoped layers or layouts; only a
  layer whose `Layer::path` is `None` wraps every request (it sees the miss as
  the `Err` from `next.run`).
- Discovered layers must have unique paths; stack same-path layers by explicit
  `.layer(...)` registration (last registered = outermost).
- Layers that register request-scoped values do it via `cx.with(value)` and pass
  the child cx to `next.run`.

## API routes

```rust
use topcoat::{Result, router::{content::Json, route}};

#[route(GET "/api/health")]
async fn health() -> Result<&'static str> { Ok("ok") }

#[route(POST "/api/users")]
async fn create_user(Json(input): Json<CreateUser>) -> Result<Json<User>> { /* … */ }
```

- Method(s) first: `GET`, `[GET, POST]`, or `*` (specific method beats `*` at
  the same path). Path string optional (module-derived otherwise).
- Returns `Result<T>` where `T: IntoResponse`. **Values are not auto-JSON** —
  wrap in `Json<T>` to opt in. This is where the varve platform mounts
  `POST /graphql` and download handlers.

## module_router!

`module_router!()` (requires `discover`) derives paths from the Rust module
tree; call it in the route-root module — it returns a `RouterBuilder`:

```rust
pub fn router() -> topcoat::router::Router {
    topcoat::router::module_router!().discover()
        .assets(AssetBundle::load().unwrap())
        .build()
}
```

- Modules must be reachable via `mod` declarations (no filesystem scanning).
- Each module below the root contributes one kebab-cased segment
  (`app::blog_posts` → `/blog-posts`); function names don't matter — handlers
  without a path string in the same module share the module's path.
- `_`-prefixed modules are **groups** (no URL segment, still match
  layouts/layers). Override with `segment!(rename = "articles")`,
  `segment!(kind = Group | Static | Param | CatchAll)` — one per module,
  mutually exclusive with `path_param!` in that module.
- `module_router!` registers module-derived handlers only; add `.discover()`
  for explicit-path handlers, fonts, procedures, shards, or register by hand.
- Two module-derived layouts (or layers) at the same logical path are rejected.
- **Field note (verified 2026-08-21): `module_router!` takes no root
  argument** — it roots at the *calling module* (expands to
  `ModuleRouterBuilder::new(module_path!())`). Call it inside the
  route-root module (a one-line `pub(crate) fn builder() ->
  RouterBuilder` there; chain cookies/sessions/etc. in lib.rs), never
  from lib.rs — calling it in the crate root would make `pages` a
  `/pages` segment. Module-derived items outside the calling module's
  subtree panic at build. Bare `not_found!()` expands to a
  `/{*rest}`-style CatchAll module registered by `module_router!`
  itself; the macro's internal module inventory and a chained
  `.discover()`'s explicit-path inventory are separate — no double
  registration. Convention (coffee-shop demo + this repo): GET handler
  named `page`, POST named `submit`, shared helpers in the parent
  module via `super::`.

## Path parameters

`path_param!` declares a typed parameter and generates a Pascal-cased marker
type (`path_param!(post_id: u64)` → `struct PostId(u64)`); read it with the
`path_param::<T>(cx)` function. Inside a `module_router!` module the
declaration also turns that module's segment into the parameter.

```rust
use topcoat::{Result, context::Cx, router::{page, path_param}, view::view};

path_param!(post_id: u64, error = bad_request);

#[page("/posts/{post_id}")]
async fn post(cx: &Cx) -> Result {
    let post_id = path_param::<PostId>(cx)?;   // &u64; memoized per request
    view! { <h1>"Post " (post_id)</h1> }
}
```

- Untyped `path_param!(slug)` → `path_param::<Slug>(cx)` returns `&str`
  (percent-decoded, infallible).
- Typed without `error = …` returns `Result<&T, &<T as FromStr>::Err>`.
- `error =` forms: `bad_request` / `bad_request("msg")` / `not_found` /
  `unauthorized` / `forbidden` / `redirect("/p")` / `redirect_permanent("/p")`.
- Catch-all: `path_param!(*doc_path)` → `CatchAllSegments<'_>` (one decoded
  `&str` per segment); typed `path_param!(*ids: u32)` → `Result<&[u32], _>`.
- One `path_param!` per module; requirements: `FromStr`, `Display` (for
  `href!`), value and error `Send + Sync + 'static`.
- Reading a param the matched route didn't capture **panics**.

## Query parameters

```rust
use topcoat::{Result, context::Cx, router::{page, query_params}, view::view};

#[query_params(error = bad_request)]
struct PostsQuery {
    page: Option<u32>,
    q: Option<String>,
}

#[page("/posts")]
async fn posts(cx: &Cx) -> Result {
    let query = query_params::<PostsQuery>(cx)?;  // &PostsQuery; memoized
    view! { <p>"page: " (query.page.unwrap_or(1))</p> }
}
```

- Derives `serde::Deserialize`; parsed with `serde_urlencoded`. Use `Option<T>`
  for optional keys — `#[serde(default)]` is NOT applied, missing non-Option
  keys are parse errors.
- Not tied to a route: any handler with `cx` can read it. Same `error =` forms
  as `path_param!`; `error = redirect("?")` reloads the page with an empty
  query (only safe if every field is `Option`).

## Building URLs with href!

`href!` (0.6.0+) builds a URL from a handler, type-checked against its path:

```rust
use topcoat::router::href;

view! {
    <a href=(href!(post, PostId(1)))>"The first post"</a>              // /posts/1
    <a href=(href!(document, DocPath(["guides", "getting started"])))>"Guides"</a>
    <a href=(href!(menu::page))>"Menu"</a>
}
// Outside a view: a String, resolved against the request
Ok(see_other(href!(page).resolve(cx)))
```

Params match by declared type/name (wrong type panics, never a wrong URL);
segments are `Display`-formatted and percent-encoded. The `Href` value also has
`.query(item)`, `.fragment(f)`, `.relative()`/`.absolute()`/`.form(...)`; the
plain function `href(target, (params,))` exists too. Empty/`.`/`..` segments
panic.

## Errors

Module `topcoat::router::error` (`crates/topcoat-router/docs/error.md`):

- Constructors named after the response: `not_found()`, `unauthorized()`,
  `forbidden()`, `bad_request(desc)`, `redirect(uri)` (307),
  `redirect_permanent(uri)`, `see_other(uri)` (+ `SeeOther` type),
  `too_many_requests(secs)` (429 + Retry-After), `service_unavailable(secs)`
  (503), `internal_server_error(source)`. Error types: `NotFoundError`,
  `ForbiddenError`, `BadRequestError`, `MethodNotAllowedError`,
  `ContentTooLargeError`, `RedirectError`, `UnauthorizedError`.
- `RouterErrorExt` on `Option`/`Result`: `.ok_or_not_found()?`,
  `.ok_or_unauthorized()?`, `.ok_or_redirect("/login")?`, `.ok_or_forbidden()?`, …
- Any other error → 500, message never leaked.
- **Catching**: errors keep their type; an outer layout matches on
  `slot` and `error.downcast_ref::<NotFoundError>()`, replacing the view (add
  `(StatusCode::NOT_FOUND)` in the replacement view or it becomes a 200).
- **Branded 404s**: unmatched URLs skip layouts since 0.6.0. `not_found!("/")`
  registers a catch-all page (named `not_found`) resolving every unmatched URL
  under the prefix to `NotFoundError`, so layouts see it. Bare `not_found!()`
  is the module-derived form. The prefix URL itself is not covered.
- **Rewrites**: `Err(rewrite("/dashboard-beta", Body::empty()).into())`
  re-dispatches invisibly at another path (method/headers kept, all per-request
  state discarded, layers rerun). Handler sees the rewritten `uri(cx)`;
  `request::original_uri(cx)` is what the client asked. Max 8 rewrites, no
  revisiting a path.

## Cross-origin requests (OriginPolicy)

Applied before any layer/handler; by default state-changing cross-origin
browser requests and cross-origin WebSocket handshakes get 403. Safe methods
(GET) are deliberately unchecked — keep state changes on POST.

```rust
use topcoat::router::{OriginPolicy, Router};
let router = Router::builder()
    .origin_policy(OriginPolicy::new()
        .trust_origins(["https://app.example.com"])
        .exempt_paths(["/webhooks/stripe"]))
    .build();
// OriginPolicy::dangerous_disable() opts out entirely.
```

## Request bodies and responses

(`crates/topcoat-router/docs/content.md`)

- Extractors (`FromRequest`): `content::Json<T>`, `content::Form<T>`,
  `request::Bytes`, `String`, `Body` (raw stream), `Option<E>` for optional
  bodies. Unparseable body → 400.
- **Body limit**: buffering extractors read at most the limit (default 2 MiB),
  else 413. Raise per subtree:
  `.layer(BodyLimit::max(32 * 1024 * 1024).at("/upload"))`. Taking `Body`
  directly streams and is not limited.
- Responses: `T: IntoResponse` — strings, byte buffers, `StatusCode`,
  `Json<T>`, tuples `(StatusCode, Json<T>)` / `(headers, body)` (last element =
  body, leading `StatusCode` = status, middle = headers/extensions). `Js` and
  `Wasm` wrappers force the exact media types browsers verify. Implement
  `IntoResponse` for full control (downloads: set your own headers/body).
- Behind features: `multipart` (below), `websocket` (`WebSocketUpgrade`
  extractor, `content/websocket.md`), `sse` (`Sse` + `Event` stream,
  `content/sse.md`), `sitemap` (`Sitemap` response, `content/sitemap.md`).

## Multipart uploads

Feature `multipart`; `crates/topcoat-router/docs/content/multipart.md`:

```rust
use topcoat::{Result, router::{content::multipart::Multipart, route}};

#[route(POST "/api/upload")]
async fn upload(mut multipart: Multipart) -> Result<&'static str> {
    while let Some(field) = multipart.next_field().await? {
        let name = field.name().map(str::to_owned);
        let data = field.bytes().await?;           // or .text(), or .chunk() / Stream
        println!("field `{name:?}` is {} bytes", data.len());
    }
    Ok("received")
}
```

Field metadata: `name()`, `file_name()`, `content_type()`, `headers()`. Fields
arrive in request order, one at a time (mutable borrow). Malformed body → 400.

## Tower interop (the escape hatch)

Feature `tower`; `crates/topcoat-router/docs/tower.md`. Three bridges:

```rust
use topcoat::router::{Methods, Router, tower::{TowerLayer, TowerRoute, TowerService}};
use tower::timeout::TimeoutLayer;

// 1. Mount a tower service (axum router, hyper service, proxy) as a route.
//    Original URI is passed through untouched.
let router = Router::builder()
    .route(TowerRoute::new(Methods::Any, "/legacy/{*rest}", legacy_axum_router))
    // catch-all does not match the bare prefix; add a second TowerRoute for "/legacy"
    .layer(TowerLayer::new(TimeoutLayer::new(std::time::Duration::from_secs(5))).at("/api"))
    .build();

// 2. Serve a whole topcoat router inside a tower/axum app (0.6.2+).
let app = axum::Router::new()
    .fallback_service(TowerService::new(topcoat_router));
```

- `TowerLayer` wraps every route unless scoped with `.at(prefix)`.
- Topcoat errors pass through tower middleware unchanged (still catchable by
  type); errors the tower service itself produces surface as
  `TowerServiceError` (rendered 500 unless mapped).
- Mounted/wrapping services must be `Clone + Send + Sync` (wrap non-`Sync` in
  `tower::buffer`).
- `TowerService` is `Clone`/infallible; mount it where full request paths are
  forwarded (root fallback) — behind a prefix-stripping mount, generated URLs
  (hrefs, redirects, asset URLs) would point outside the mount.
