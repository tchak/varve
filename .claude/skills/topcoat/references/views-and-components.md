# Views and components (topcoat-view)

Sources: `crates/topcoat-view/macro/docs/` (`view.md`, `component.md`,
`attributes.md`, `class.md`, `props.md`). Verified against `topcoat-v0.6.2`.

## view! syntax

HTML-like, close to real HTML — with one big exception: **text nodes must be
quoted** (`"Home"`, not `Home`).

```rust
use topcoat::{Result, view::*};

#[component]
async fn example(user: &User) -> Result {
    view! {
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
    }
}
```

- `(expr)` in child position → node; in attribute value position → value;
  also works for dynamic attribute names `(attr)="v"` and dynamic element
  names `<(tag)>…</(tag)>`.
- Non-void elements need matching closing tags. Attribute names may contain
  `-`, `:`, `.` (`data-post-id`, `aria-label`, `hx-get`, `class.active`).

### Control flow (Rust, with markup bodies)

`if`/`else if`/`else`, `if let`, `for pat in expr { … }`, `match` (arms are one
node — wrap multiple siblings in `{ … }`; guards allowed), and `let pat = expr;`
statements. All of these also work **inside an element's attribute list**,
emitting attributes instead of nodes:

```rust
view! {
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
}
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
status / per header name — so a declaration **before** a layout's `(slot?)`
overrides the page, **after** it is a fallback the page can override:

```rust
use topcoat::router::{StatusCode, HeaderValue, header};
view! {
    (StatusCode::NOT_FOUND)
    ((header::CACHE_CONTROL, HeaderValue::from_static("no-store")))
    <h1>"Page not found"</h1>
}
```

Requires the `router` feature; discarded when a view is rendered to a plain
string.

## Components

```rust
use topcoat::{Result, view::{View, component, view}};

#[component]
async fn panel(title: &str, child: View) -> Result {
    view! {
        <section class="panel">
            <h2>(title)</h2>
            <div class="panel-body">(child)</div>
        </section>
    }
}
```

Called inside `view!` with **named-parameter call syntax**; trailing view nodes
become the `child: View` parameter (no commas between them):

```rust
view! {
    panel(
        title: "Profile",
        <p>"Account details"</p>
        badge(label: "Active", tone: "success")
    )
}
```

- Parameter attributes: `#[default]` / `#[default(expr)]` (optional param),
  `#[into]` (caller passes `impl Into<T>`; preferred over `impl Into<T>`
  params to avoid monomorphization).
- Generics work (`T: Send + Sync` often needed); `impl Trait` params work.
- `cx: &Cx` parameter is filled automatically (request context).
- Recursive components: mark one component in the cycle `#[component(boxed)]`.
- **Keys**: `key:` is reserved — inside a `for` loop, key repeated invocations
  with a stable item id (`post_card(key: post.id, title: …)`); any
  `IdentityKey` works. An unkeyed repeated invocation renders but consuming its
  identity (state) errors. (Identity system is new in 0.6.0.)

### Async + concurrent rendering (0.6.0)

Components are async and render **concurrently**: siblings, loop iterations,
taken branches, nested components all start at once (no request waterfalls —
components query data directly, dedupe with `#[memoize]`). Markup output stays
in source order, but body *execution order* is unspecified: treat components as
side-effect-free functions of (props, cx); never communicate between components
through shared mutable state. Plain Rust in the view (interpolations, `let`,
conditions) still runs in source order.

### Rendering outside a component

Inside `#[component]`/`#[page]`/`#[layout]`/`#[shard]` the cx is implicitly in
scope. In a plain function, pass it: `view! { cx => greeting(name: "World") }`.

### Custom values in markup

Traits (each takes a `PartsWriter` whose `push_*` methods escape for the
position; `push_*_unescaped` is the only opt-out): `NodeViewParts` (child),
`AttributeValueViewParts` (value; `attribute_present()` controls omission),
`AttributeKeyViewParts`, `AttributeViewParts` (whole attribute fragments),
`ElementNameViewParts`.

## attributes!

Builds a reusable `topcoat::view::Attributes` (map-like, unique keys, insertion
replaces) with the same attribute syntax as `view!` — including control flow,
binds, and event handlers:

```rust
use topcoat::view::{attributes, view};

let attrs = attributes! { class="button" type="submit" aria-label="Save changes" };
view! { <button (attrs)>"Save"</button> }        // parenthesized attribute fragment
```

Runtime API: `attrs.insert(cx, "data-state", "loading")`,
`attrs.contains_key("class")`. Inserting into an element **consumes** the
value (clone to reuse). Components take `Attributes` as ordinary params to
forward caller-controlled attributes (`panel(attrs: attributes! { … }, …)`).

## class!

Builds `topcoat::view::Class` — space-joined entries, attribute omitted when
all entries are absent (`None`, empty string, false condition):

```rust
use topcoat::view::{class, view, StaticClass};

view! {
    <button class=(class!(
        "btn",
        variant,                                  // Option<&str>
        sizes,                                    // Vec<String>
        "cursor-pointer" if enabled else "opacity-50",
    ))>"Save"</button>
}

const BUTTON: StaticClass = class!("btn btn-lg rounded");  // faster than &'static str
```

Entry forms: `expr`, `expr if cond`, `expr if cond else alt`. Entries implement
`ClassViewParts` (strings, Options, Vec/arrays, another `Class`,
`AttributeValue`).

## Props derive

`#[derive(Props)]` on `FooProps` generates a typestate `FooPropsBuilder`:
`build()` only exists once every required property is set (compile error, not
panic). Fields accept `#[default]`/`#[default(expr)]` and `#[into]`.

## Formatting

`topcoat fmt` formats macro bodies (`view!` etc.) alongside `rustfmt`; see
`references/project-setup.md`.
