# Client reactivity (topcoat-runtime)

Sources: `crates/topcoat/docs/runtime.md`,
`crates/topcoat-runtime/macro/docs/{expr,procedure,shard}.md`. Verified at
`topcoat-v0.6.2`.

**Upstream calls the runtime "highly experimental and fairly limited"** —
expect breaking changes; the expression vocabulary is small. No wasm bundle, no
client build step: reactive expressions are type-checked Rust compiled twice
(server Rust for the initial HTML + equivalent JavaScript shipped with the
page).

## Setup

Interactive pages need the browser script in the head, and the script is a
topcoat asset, so the asset bundle must be on the router:

```rust
view! {
    <html>
        <head>
            topcoat::runtime::script()
        </head>
        <body></body>
    </html>
}

// router: .discover() also registers procedure/shard endpoints
Router::builder()
    .discover()
    .assets(AssetBundle::load().unwrap())
    .build();
```

## Runtime expressions: `$(...)`

`$(...)` wherever a view node can stand; sugar for the `expr!` macro. Server
evaluates once for initial HTML; the JS re-runs in the browser whenever a
signal it read changes.

## Signals

Declared with a `signal` **statement inside a `view!` body**; initial value is
ordinary Rust, evaluated at server render and serialized into the page:

```rust
view! {
    signal count = 0.0;

    <button @click=$(|_e| count.increment())>"+1"</button>
    <p>"Count: " $(count.get())</p>
}
```

Signal methods (inside expressions): `.get()`, `.set(v)`; shorthands:
`.toggle()` on `bool`, `.increment()`/`.decrement()` on `f64`, `.push_str(s)`
on `String`.

## Event handlers: `@event`

`@click`, `@input`, any DOM event. Value is a runtime expression evaluating to
a closure, run in the browser; the closure receives an `Event`
(`topcoat::runtime::Event`) mirroring the DOM event: `e.target.value`, `e.key`,
`e.client_x`, `e.prevent_default()`, …

```rust
view! {
    signal query = String::new();
    <input @input=$(|e: Event| query.set(e.target.value))>
}
```

Escape hatch: the value may be a raw-JS string literal: `@click="alert('hi')"`.

## Bind attributes: `:attr`

`:hidden=$(!open.get())`, `:value=$(name.get())` — server renders the initial
value; browser re-applies whenever a read signal changes. `:value` + `@input`
gives two-way binding.

## The expr! vocabulary (shared Rust/JS subset)

Types: `f64` (ALL numbers — integer literals rejected, write `1.0`), `bool`,
`String`/`&str` (`len` [UTF-8 bytes], `is_empty`, `trim*`, `starts_with`,
`ends_with`, `contains`, `to_owned`, comparisons [by code point]),
`Option<T>` (`is_some`, `is_none`, `unwrap`, `expect`),
`Result<T, E>` (`is_ok`, `is_err`, `ok`, `err`, `unwrap*`, `expect*`), tuples,
`Signal`. Rust semantics are the definition; the JS matches (Display-style f64
text: `inf`, `-0`).

Syntax: literals, listed operators, method calls / field access / indexing,
blocks with `let` of plain identifiers, `if`/`else` as expression, closures
(optionally `async`) and `.await`, `loop`/`while`/`break`/`continue`/`return`.
**Rejected**: `match`, integer literals, struct expressions, multi-segment
paths.

Free identifiers are **captured**: serialized at render time, constant in the
JS — a snapshot, never updated from the server. Captures must be vocabulary
types.

### raw! — embedded JavaScript

```rust
$({
    let n = name.get();
    raw!("${n}.toUpperCase()", n.to_uppercase())   // (js, equivalent rust)
})
```

`${ident}` interpolates a scope binding. Omitting the second (Rust) argument
means the expression cannot be server-evaluated — only usable where it runs
purely in the browser. Equivalence of the two sides is on you.

## Procedures — async server functions callable from the browser

```rust
use topcoat::{Result, context::Cx, runtime::procedure};

#[procedure]
async fn search(cx: &Cx, query: String) -> Result<String> {
    // database, app context, session — full server Rust
    Ok(query)
}
```

- Called from a runtime expression like a normal async fn, in async position:
  `@click=$(async |_e| { let r = search(q.get()).await; results.set(r); })`.
- HTTP under the hood: **every procedure is a public endpoint; arguments can
  be spoofed and must not be trusted** — authorize inside the procedure.
- Args and `Ok` type must be vocabulary types. `cx: &Cx` is filled server-side,
  invisible to the client.
- Calls only run in the browser; calling during server render panics — keep
  calls inside closures that never run server-side.
- An `Err` fails the awaiting expression without an observable error value;
  return outcome-as-data (`Result<String, String>` as the `Ok` type) if the
  client must react.
- Registration: `.discover()`, or
  `Router::builder().procedure(double)` (`RouterBuilderProcedureExt`).

## Shards — server-re-rendered components

A `#[shard]` is a component whose arguments are runtime expressions; when a
signal they read changes, the browser posts current values to a per-shard
endpoint, the function re-runs on the server, and the returned HTML is swapped
in place:

```rust
use topcoat::{Result, context::Cx, runtime::shard, view::view};

#[shard]
async fn search_results(cx: &Cx, query: String) -> Result {
    let products = search_products(cx, &query).await?;
    view! { for product in products { <div>(product)</div> } }
}

// usage inside a view:
view! {
    signal query = String::new();
    <input :value=$(query.get()) @input=$(|e: Event| query.set(e.target.value))>
    search_results(query: $(query.get()))
}
```

- Initial page render runs the shard inline (no extra request). Changes
  coalesce per tick; a new request aborts the in-flight one.
- Re-render replaces content wholesale: signals declared inside the shard
  reset. Persistent state lives outside, flows in through arguments.
- **Guards on the page/layout do NOT cover the shard endpoint** — a shard
  rendering private content must authorize itself (`require_auth(cx).await?`)
  and validate its arguments (caller-controlled).
- Args must be vocabulary types; return is `Result` (a view); `cx` special as
  usual. Registration: `.discover()` or `.shard(search_results)`
  (`RouterBuilderShardExt`).
