# Request context and state (topcoat-core)

Sources: `crates/topcoat/docs/context.md`, `crates/topcoat/docs/app_context.md`,
`crates/topcoat-core/macro/docs/memoize.md`,
`crates/topcoat/docs/functions_not_middlewares.md`. Verified at `topcoat-v0.6.2`.

## Cx

`topcoat::context::Cx` is the request context. Handlers, layouts, components,
procedures, and shards take it as an **optional parameter literally named
`cx`** (`cx: &Cx`); topcoat fills it automatically. There are no axum-style
extractors for state — everything request-scoped is read *from* `cx`.

### Router request helpers (`topcoat::router::request`)

`parts(cx)` (http `Parts`), `method(cx)`, `uri(cx)`, `original_uri(cx)` (the
pre-rewrite client URL), `version(cx)`, `headers(cx)`, `content_type(cx)`,
`extensions(cx)`.

```rust
use topcoat::{context::Cx, router::request::{headers, method, uri}};

fn request_summary(cx: &Cx) -> String {
    let ua = headers(cx).get("user-agent")
        .and_then(|v| v.to_str().ok()).unwrap_or("unknown");
    format!("{} {} from {ua}", method(cx), uri(cx).path())
}
```

Path/query values: `path_param::<T>(cx)` and `query_params::<T>(cx)` — see
`references/routing.md`. Both parse lazily, once per request, memoized.

## App context (long-lived values)

Register on the router, read anywhere; keyed by concrete `TypeId`
(`T: Any + Send + Sync`; register duplicates of a type → panic; newtype-wrap
for two values of the same underlying type):

```rust
Router::builder()
    .discover()
    .app_context(Database::connect())   // e.g. a toasty Db, an HTTP client
    .app_context(HttpClient::new())
    .build();

// reading — panics if unregistered (startup bug), so wrap in helpers:
fn db(cx: &Cx) -> &Database { app_context(cx) }
// optional flavor:
fn feature_config(cx: &Cx) -> Option<&FeatureConfig> { try_app_context(cx) }
```

Imports: `topcoat::context::{app_context, try_app_context, request_context,
try_request_context}`.

## Request context (per-request values) — `Cx::with` (0.6.0)

`CxBuilder` no longer exists. Request context is registered by **scoping**:
`cx.with(value)` returns a child `Cx` that additionally holds the value
(`cx.with_many((a, b))` for several). The child shares app context, memoize
cache, etc.; re-registering a type shadows it for the child scope only. This is
how layers expose values (cookie jar, current tenant…) to everything below.

```rust
use topcoat::context::{Cx, request_context};

struct Customer { name: String }

fn greet(cx: &Cx) -> String {
    let cx = cx.with(Customer { name: "Ada".to_owned() });
    let customer: &Customer = request_context(&cx);
    format!("Hello, {}", customer.name)
}
```

### Work that outlives the handler

`Cx` is clonable; a spawned task or streaming body must own its handle
(`let cx = cx.clone(); tokio::spawn(async move { … })`). After the response is
sent the clone still reads context but response-directed writes (cookies) are
dropped.

## #[memoize]

Per-request cache (like React `cache`) keyed by a 128-bit hash of every
argument except `cx`. Empty at the start of each request.

```rust
use topcoat::context::{Cx, memoize};

#[memoize]
async fn get_user(cx: &Cx, id: i64) -> User { db::load_user(id).await }
// return type is rewritten to &User (borrowed from the request cache)

#[memoize(as_ref)]
async fn find_user(cx: &Cx, id: i64) -> Option<User> { … }
// as_ref borrows the CONTENTS: Option<&User> (Result<T,E> -> Result<&T,&E>)
```

- Works on sync and async fns. Concurrent async callers share one in-flight
  future (concurrent view rendering hits the DB once).
- Requirements: a param literally named `cx: &Cx`; no `self`; every other arg
  `Hash` (hashed, never cloned — hand-written partial `Hash` impls cause
  silent collisions); return type `Send + Sync + 'static`.
- Recursion with identical args panics (deadlock guard); different args fine.
- **Request-context dependencies are tracked**: the cache records which
  request-context values the body read, and only hands a cached result to
  callers whose scope resolves those reads to the same values — so
  `cx.with(...)`-scoped values never leak across scopes. App context is not
  tracked. Dependencies propagate through nested memoized calls.
- Not a cross-request cache — layer Redis/LRU behind your data functions.

## The idiom: functions, not middlewares

(`crates/topcoat/docs/functions_not_middlewares.md`) Do not build auth as
middleware or extractors. Write small composable `cx` functions and call them
from whatever needs them — pages, layouts, deeply nested components, shards:

```rust
use topcoat::{
    Result,
    context::{Cx, app_context, memoize},
    router::error::{RouterErrorExt, UnauthorizedError},
};

fn db(cx: &Cx) -> Db { app_context::<Db>(cx).clone() }

#[memoize(as_ref)]
async fn fetch_user(cx: &Cx, user_id: &str) -> Option<User> { /* db lookup */ }

fn session_cookie(cx: &Cx) -> Option<&str> { /* read cookie/header */ }

async fn fetch_current_user(cx: &Cx) -> Option<&User> {
    let user_id = session_cookie(cx)?;
    fetch_user(cx, user_id).await
}

async fn require_auth(cx: &Cx) -> Result<&User, UnauthorizedError> {
    fetch_current_user(cx).await.ok_or_unauthorized()
}

async fn require_admin(cx: &Cx) -> Result<&User> {
    let user = require_auth(cx).await?;
    Ok(user.is_admin().then_some(user).ok_or_forbidden()?)
}
```

A component that calls `require_auth(cx).await?` is guarded wherever it
renders; `#[memoize]` dedupes the lookup across layout + page + components.
Reserve router **layers** for transport concerns (compression, tracing,
body limits); reserve `cx` functions for application data.

## Testing contexts

`topcoat::context::CxTestBuilder` assembles a `Cx` from scratch:

```rust
use topcoat::context::CxTestBuilder;

let cx = CxTestBuilder::new()
    .app_context(config)            // any Any + Send + Sync value
    .request_context(Marker(7))
    .build();
```
