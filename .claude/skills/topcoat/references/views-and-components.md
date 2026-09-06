# Views and components (topcoat-view)

Sources: `crates/topcoat-view/macro/docs/` (`view.md`, `component.md`,
`live.md`, `attributes.md`, `class.md`, `props.md`),
`crates/topcoat/src/view/{suspense,error_boundary}.rs`,
`crates/topcoat-view/src/{view,child}.rs`. Verified against `v0.7.0`.

## view! syntax

HTML-like, close to real HTML — with one big exception: **text nodes must be
quoted** (`"Home"`, not `Home`).

```rust
use topcoat::{Result, view::*};

#[component]
async fn example(user: &User) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html>
            <head>
                <meta charset="utf-8">          // void elements: no closing tag
                <link rel="stylesheet" href="/app.css">
            </head>
            <body>
                <label for="email">"Email"</label>   // keywords ok as attr names
                <input type="email" id="email" aria-label="Email address">
                <h1>"Hello, " (user.name) "!"</h1>   // (expr) interpolates
                <my-widget data-widget-id="profile"></my-widget>
            </body>
        </html>
    })
}
```

- `(expr)` in child position → node; in attribute value position → value;
  also works for dynamic attribute names `(attr)="v"` and dynamic element
  names `<(tag)>…</(tag)>`.
- Non-void elements need matching closing tags. Attribute names may contain
  `-`, `:`, `.` (`data-post-id`, `aria-label`, `hx-get`, `class.active`).

## Views are lazy (0.7.0)

A `view!` expression **does not render where it is written**. It evaluates to
a value implementing the `View` trait; the expressions inside run when that
view renders — when it becomes a response, or when the view it is interpolated
into does. A view that is never rendered never runs them, like a future that
is never awaited. The macro expands to (the equivalent of) an `async move`
block, so it **captures every variable the template mentions by moving it**:

```rust
let title = String::from("Hello");
let header = view! { <h1>(title)</h1> };
// `title` has moved into `header` and cannot be used here anymore.
Ok(view! {
    (header)
    <p>"Welcome!"</p>
})
```

- Need a value both inside the view and after it? Interpolate a clone.
- A view capturing a reference borrows what it points at and cannot outlive
  it. In practice fine: component props and anything borrowed from the request
  context live until the render is over, so a `&str` prop is safe.
- Every `view!` has its own anonymous type: a function returning different
  `view!`s from several `return` sites must give them one type with
  `.boxed()` (`ViewExt`, → `BoxView<'a>` = `Pin<Box<dyn View + 'a>>`).
- `()` is the empty view. `Child<'_>`/`Slot<'_>` are views too.

### Control flow (Rust, with markup bodies)

`if`/`else if`/`else`, `if let`, `for pat in expr { … }`, `match` (arms are one
node — wrap multiple siblings in `{ … }`; guards allowed), and `let pat = expr;`
statements. All of these also work **inside an element's attribute list**,
emitting attributes instead of nodes:

```rust
Ok(view! {
    <a href="/posts"
        if current { aria-current="page" class="active" }
    >"Posts"</a>
    <ul>
        for post in posts {
            <li><a href=(post.url)>(post.title)</a></li>
        }
    </ul>
    match status {
        Status::Draft => <span>"Draft"</span>,
        Status::Published { title } => <a href="/posts">(title)</a>,
        _ => "",
    }
})
```

### Boolean / conditional attributes

- Static: prefer the literal `disabled=""` (folded into the template).
- Expression attributes self-remove: `false` or `None` omits the whole
  attribute; `true` renders it empty; `Some(v)` renders `v`.
  `aria-current=(is_current.then_some("page"))`, `title=(maybe_title)`.
- `disabled="false"` is still disabled (literal attributes always render).
  Enumerated attrs (`aria-expanded`, `contenteditable`) need string
  `"true"`/`"false"`, not bools.

### Status codes and response headers from a view

A `StatusCode` in node position sets the response status; a `HeaderMap` or a
single `(HeaderName, HeaderValue)` pair adds headers. First rendered wins per
status / per header name — so a declaration **before** a layout's `(slot)`
overrides the page, **after** it is a fallback the page can override:

```rust
use topcoat::router::{StatusCode, HeaderValue, header};
Ok(view! {
    (StatusCode::NOT_FOUND)
    ((header::CACHE_CONTROL, HeaderValue::from_static("no-store")))
    <h1>"Page not found"</h1>
})
```

Requires the `router` feature; discarded when a view is rendered to a plain
string. **Only the first phase of a render can declare them** (0.7): a status
code inside a `live!` region counts when it is part of the region's first
emission (e.g. an `error_boundary` fallback that replaces a page's first
content); after the response committed, declarations are ignored.

## Components

```rust
use topcoat::{Result, view::{Child, View, component, view}};

#[component]
async fn panel(title: &str, #[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <section class="panel">
            <h2>(title)</h2>
            <div class="panel-body">(child)</div>
        </section>
    })
}
```

Called inside `view!` with **named-parameter call syntax**; trailing view nodes
become the `child` parameter (no commas between them):

```rust
Ok(view! {
    panel(
        title: "Profile",
        <p>"Account details"</p>
        badge(label: "Active", tone: "success")
    )
})
// desugars to: panel(title: "Profile", child: view! { … }.into())
```

- A parameter literally named `child` of type `Child<'_>` receives the extra
  nodes. Give it `#[default]` so the component can also be called without
  children (`Child::default()` renders nothing). Passing `child:` explicitly
  needs `.into()` (a `view!`, `live!`, or `BoxView` converts) or
  `Child::new(view)`. The component decides where the children render by
  interpolating `(child)`; children never interpolated never run.
- Return type is `Result<impl View>` (import the `View` trait); the body ends
  with `Ok(view! { … })`.
- Parameter attributes: `#[default]` / `#[default(expr)]` (optional param),
  `#[into]` (caller passes `impl Into<T>`; preferred over `impl Into<T>`
  params to avoid monomorphization; a `#[into] fallback: Child<'_>` prop
  accepts a bare `view! { … }`).
- Generics work (`T: Send + Sync` often needed); `impl Trait` params work.
- `cx: &Cx` parameter is filled automatically (request context).
- **Recursive components**: a component returns an anonymous view type, so a
  cycle describes a type containing itself. Erase one component's view in the
  cycle with `.boxed()` — `Ok(view! { … }.boxed())` — and the others keep
  `impl View`. (`#[component(boxed)]` no longer exists.)
- **Keys**: `key:` is reserved — inside a `for` loop, key repeated invocations
  with a stable item id (`post_card(key: post.id, title: …)`); any
  `IdentityKey` works. An unkeyed repeated invocation renders but consuming its
  identity (state) errors.

### Async + concurrent rendering

Components are async and render **concurrently**: siblings, loop iterations,
taken branches, nested components all start at once (no request waterfalls —
components query data directly, dedupe with `#[memoize]`). Markup output stays
in source order, but body *execution order* is unspecified: treat components as
side-effect-free functions of (props, cx); never communicate between components
through shared mutable state. Plain Rust in the view (interpolations, `let`,
conditions) still runs in source order.

### Rendering outside a component

Inside `#[component]`/`#[page]`/`#[layout]`/`#[shard]` the cx is implicitly in
scope. In a plain function, pass it: `view! { cx => greeting(name: "World") }`
(same for `emit! { cx => … }`). Whether it names its context or not, a view
is self-contained as the outermost view of a render and takes part in the
enclosing render when interpolated.

### Custom values in markup

Traits (each takes a `PartsWriter` whose `push_*` methods escape for the
position; `push_*_unescaped` is the only opt-out): `NodeViewParts` (child),
`AttributeValueViewParts` (value; `attribute_present()` controls omission),
`AttributeKeyViewParts`, `AttributeViewParts` (whole attribute fragments),
`ElementNameViewParts`.

## Live regions: live!, emit!, suspense, error_boundary (0.7.0)

Source: `crates/topcoat-view/macro/docs/live.md`, `examples/live`,
`examples/suspense`. A page normally renders in full before the browser sees
any of it. `live!` marks a region whose content can still change while the
response streams; inside its body, `emit!` renders markup into the region and
**every emission replaces the previous one** in the browser — over the same
response, no client library, no fetching.

```rust
use topcoat::{Result, view::{View, component, emit, live, view}};

#[component]
async fn daily_quote() -> Result<impl View> {
    Ok(live! {
        emit! { <p>"Loading..."</p> }?;
        let quote = fetch_quote().await;
        emit! { <blockquote>(quote)</blockquote> }
    })
}
```

- The body is ordinary async Rust: await, loop, branch between emissions
  (`for percent in 0..100 { emit! { <p>(percent) "%"</p> }?; step().await; }`).
- **The page waits for a region's first emission** and renders it with the
  rest of the document — start with something that is ready right away.
- `emit!` accepts everything `view!` does and evaluates to
  `Result<EmitToken>`; the body returns one, so ending with an emission is the
  natural shape (intermediate emissions use `?`). When control flow does not
  end with an emission, return `Ok(EmitToken)` yourself (compile-time
  reminder only — the body still has to emit at least once).
- Concurrent emissions (`join!` of async blocks) all reach the browser; the
  last to arrive stays visible. Sequence when each should be seen.
- **Errors**: a failed emission (a component inside returned `Err`) comes back
  as `emit!`'s `Err` instead of ending the stream — propagate with `?` or
  `match emit! { forecast() } { Err(e) => emit! { <p>(e.to_string())</p> }, ok => ok }`.
  An error the body returns propagates like any rendering error; after the
  page started streaming the status code can no longer change (see
  routing.md § Streaming and commit).
- A region **is a view**: interpolate it, return it from a component, pass it
  as a child; regions nest; several on a page stream independently.
- In a plain function: `emit! { cx => … }`.

Two prepackaged shapes (both `#[component]`s built on `live!`, in
`topcoat::view`):

```rust
use topcoat::view::{error_boundary, suspense};

Ok(view! {
    // shows the fallback until the child content is ready, then swaps it in
    suspense(fallback: view! { <p>"Loading..."</p> }, quote())

    // renders the child; on any error inside, swaps in the fallback's view
    error_boundary(
        fallback: |error| Ok(view! {
            <p>"The stats are unavailable: " (error.to_string())</p>
        }),
        stats()
    )
})
```

Exact signatures: `suspense(#[into] fallback: Child<'_>, #[default] child:
Child<'_>)`; `error_boundary<V: View, F>(fallback: F, #[default] child:
Child<'_>)` with `F: FnOnce(topcoat::Error) -> Result<V> + Send` — the
fallback closure is **sync** (return a lazy view that awaits inside it if you
need async work). Returning `Err(error)` from the fallback **rethrows** so an
unhandled error still reaches the enclosing handler. `suspense` does not catch
errors — nest it inside an `error_boundary`. A `(StatusCode::…)` in a fallback
sets the response status when the fallback is the region's first emission
(the child failed before the page committed), which is the layout-brands-a-404
case in routing.md.

## attributes!

Builds a reusable `topcoat::view::Attributes` (map-like, unique keys, insertion
replaces) with the same attribute syntax as `view!` — including control flow,
binds, and event handlers:

```rust
use topcoat::view::{attributes, view};

let attrs = attributes! { class="button" type="submit" aria-label="Save changes" };
Ok(view! { <button (attrs)>"Save"</button> })     // parenthesized attribute fragment
```

Runtime API: `attrs.insert(cx, "data-state", "loading")`,
`attrs.contains_key("class")`, `attrs.remove("class")`. Inserting into an
element **consumes** the value (clone to reuse). Components take `Attributes`
as ordinary params to forward caller-controlled attributes
(`panel(attrs: attributes! { … }, …)`).

## class!

Builds `topcoat::view::Class` — space-joined entries, attribute omitted when
all entries are absent (`None`, empty string, false condition):

```rust
use topcoat::view::{class, view, StaticClass};

Ok(view! {
    <button class=(class!(
        "btn",
        variant,                                  // Option<&str>
        sizes,                                    // Vec<String>
        "cursor-pointer" if enabled else "opacity-50",
    ))>"Save"</button>
})

const BUTTON: StaticClass = class!("btn btn-lg rounded");  // faster than &'static str
```

Entry forms: `expr`, `expr if cond`, `expr if cond else alt`. Entries implement
`ClassViewParts` (strings, Options, Vec/arrays, another `Class`,
`AttributeValue`).

## Props derive

`#[derive(Props)]` on `FooProps` generates a typestate `FooPropsBuilder`:
`build()` only exists once every required property is set (compile error, not
panic). Fields accept `#[default]`/`#[default(expr)]` and `#[into]`.

## Rendering a view in a test

`ViewExt::single(self)` resolves a non-live view's content (panics if the
first content is live — use `.first()` for live views, which keeps only the
first emission); the `ViewHandle` renders with `.render(&cx) -> String`
(status/headers discarded). Upstream pattern
(`crates/topcoat-view/macro/tests/render.rs`):

```rust
use topcoat::{context::Cx, view::{View, ViewExt, view}};

async fn r(v: impl View) -> String {
    let cx = Cx::default();
    v.single().await.unwrap().render(&cx)
}
// let cx = &Cx::default(); assert_eq!(r(view! { cx => <p>"hi"</p> }).await, "<p>hi</p>");
```

Component tests without a runtime: build a `Cx` with `CxTestBuilder`, build
the view synchronously (`|cx| view! { cx => … }`), and drive `.single()` with
a noop-waker `block_on` loop (see `references/project-setup.md` § Testing).

## Field notes: migrating a 0.6 codebase to lazy views (verified 2026-09-06)

What the compiler actually demanded when this repo's ~130 handlers and
components moved to `Result<impl View>`:

- **A lazy view moves its captures, so nothing it uses may borrow a local.**
  `E0515: cannot return value referencing local` points at the culprit:
  a helper returning `impl AttributeValueViewParts` from a `&str` argument
  (Rust 2024 makes the opaque type capture that lifetime — add `+ use<>`
  when the value is owned), a closure called inside the view (`let href =
  || href!(…, ElementId(id.clone()))` → `{ let id = id.clone(); move || … }`),
  or `Href::query(&item)`, which borrows `item` lazily (call `.resolve(cx)`
  to a `String` when the query data is a local). Clone strings out of a
  borrowed struct (`secret.as_str()` → `secret.clone()`) instead of holding
  `&form.field` across the view.
- **Several return sites need one type.** A `submit` handler that either
  re-renders the form or redirects boxes each arm: `Ok(view! { … }.boxed())`
  and a redirect helper typed `fn redirect_to(cx: &Cx, location: String) ->
  Result<BoxView<'static>>` (a `view! { cx => … }` owns its context, so
  `'static` is legal). Async helpers that end in that redirect return
  `Result<BoxView<'static>>` too, so match arms mixing them unify.
- **Gates and branded errors** are `error_boundary(fallback: |error| { if
  error.downcast_ref::<UnauthorizedError>().is_none() { return Err(error); }
  return_to::remember(cx)?; redirect_to(cx, …) }, (slot))`. The fallback is
  sync: anything it needs from an async lookup goes into a small
  `#[component]` it renders (`not_found_title()`), never awaited in the
  closure. A cookie written there is fine — it runs in the first phase.
- **Stack budget in debug builds: box page-level views, not just recursive
  ones.** A component's returned view is moved *by value* through the
  parent's poll frame (`Poll::Ready(Ok(view))`, then into `MoveView`,
  `ScopeView`, `Child::new`…), and a debug build gives every move its own
  stack slot for the frame's lifetime — which is the whole time the
  children render, since a lazy parent's poll frame stays on the stack
  under its descendants. Measured here: a 28 KB editor-page view cost
  ~1.1 MB of stack between the layout and its first child, and a 5-level
  recursive tree then overflowed the 2 MB test-thread stack. `.boxed()` on
  the returned view of every component on the ancestor chain (page,
  chrome, detail column) brought that prefix to 0.45 MB; the recursive pair
  (`tree_list`/`tree_row`) already boxed cost ~90 KB per level. Template
  *size* and view *state size* were not the problem (`size_of_val` of the
  views was 1–28 KB). Diagnose with a probe node — `({ let p = 0u8;
  eprintln!("STACK name {:p}", &p); "" })` — at each boundary and diff the
  addresses; a regression test rendering a deeply nested page at the
  default stack pins the budget.
- **`_cx` is rejected** ("the request context parameter must be named
  `cx`"); drop the parameter when it is unused.
- Component tests became `render(|cx| view! { cx => … })` — no `async`,
  no `?` after `view!`.

## Formatting

`topcoat fmt` formats macro bodies (`view!`, `live!`, `emit!`, `attributes!`,
`class!`, …) alongside `rustfmt`; see `references/project-setup.md`.
