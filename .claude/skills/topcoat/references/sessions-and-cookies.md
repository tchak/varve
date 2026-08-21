# Sessions and cookies — the platform's auth layer

Sources: `crates/topcoat/docs/session.md`, `crates/topcoat/docs/cookie.md`,
`examples/session/src/main.rs`, `demos/coffee-shop/src/customer.rs`. Verified
at `topcoat-v0.6.2`.

## Cookies (topcoat-cookie, feature `cookie`, on by default)

Install with `.cookies()` on the builder (`RouterBuilderCookieExt`); then
`cookies(cx)` returns the request-scoped jar. Incoming `Cookie` header parsed
once and memoized; queued changes become `Set-Cookie` automatically after the
handler returns (never touch headers yourself). Built on the `cookie` crate:
`Cookie`, `Key`.

```rust
use topcoat::{
    Result, context::Cx,
    cookie::{Cookie, Cookies, RouterBuilderCookieExt, cookies},
    router::{Router, route},
};

let router = Router::builder().cookies().build();

#[route(POST "/api/theme")]
async fn toggle_theme(cx: &Cx) -> Result<String> {
    let jar = cookies(cx);                       // bring trait `Cookies` into scope
    let next = match jar.get("theme") {
        Some(t) if t.value() == "dark" => "light",
        _ => "dark",
    };
    jar.add(Cookie::build(("theme", next)).path("/").build());
    Ok(next.to_owned())
}
```

`get(name)` / `add(cookie)` / `remove(cookie)` (removal must carry the same
`Path`/`Domain` it was set with). `add`/`remove` accept `Into<Cookie>`, so
`jar.add(("theme", "dark"))` works.

### cookie! macro

Mirrors the `Set-Cookie` header — pair first, then `;`-separated attributes:

```rust
use topcoat::cookie::{Cookie, SameSite, cookie, time::Duration};

let session: Cookie = cookie! {
    "session" = "abc123";
    Path = "/";
    Secure;
    HttpOnly;
    SameSite = Lax;
    MaxAge = Duration::hours(1)
};
```

### Jar combinators, prefixes, signed/private

Iterator-adapter-style combinators wrap the jar; each attribute has
`default_*` (fill if unset) and `override_*` (force) flavors — for `secure`,
`http_only`, `same_site`, `path`, `domain`, `max_age`, plus name prefixes
`prefix_host` (`__Host-`: forces Secure+Path=/+no Domain) and `prefix_secure`
(`__Secure-`); prefixes are stripped on read so code uses the bare name.
`.map(|c| …)` is the escape hatch. Idiom — one app-wide helper:

```rust
use topcoat::{context::Cx, cookie::{Cookies, SameSite}};

fn cookies(cx: &Cx) -> impl Cookies {
    topcoat::cookie::cookies(cx)
        .default_secure(true)
        .default_http_only(true)
        .default_same_site(SameSite::Lax)
        .default_path("/")
}
```

- **Signed** (tamper-proof, readable): `cookies(cx).signed(&key)`.
- **Private** (AES-256-GCM, tamper-proof + unreadable; name bound into
  ciphertext): `cookies(cx).private(&key)`.
- Register one `Key` as app context (`.app_context(Key::generate())`) and use
  `signed_cookies(cx)` / `private_cookies(cx)` — they panic without a
  registered `Key`. **Persist the key**; regenerating on boot invalidates all
  existing signed/encrypted cookies.
- 0.6.0: the jar is protected from writes after the response is sent.

### Typed cookies: CookieStore<T>

JSON-serialized structured value over any jar (`T: Serialize +
DeserializeOwned`); composes with signing/encryption/prefixes through the jar
passed in:

```rust
use topcoat::cookie::{CookieStore, Cookies, cookie_store, private_cookies};

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Cart { items: Vec<String> }

let cart = cookie_store::<Cart, _>(private_cookies(cx), "cart")
    .parse_or_default()                      // or parse() / parse_or(v) / parse_or_else(f)
    .update(|cart| cart.items.push("widget".to_owned()))
    .commit()?;                              // NOTHING is written until commit()
```

- `parse()` distinguishes absent (`Ok(None)`) from malformed (`Err`);
  `parse_or*` treat malformed as missing (schema changes reset stale cookies).
- Parsed store: `read()` borrows, `get()` clones, `set(v)`, `update(f)`,
  `commit()`, `rollback()`, `remove()`. Unparsed store also has `set` (skip
  reading) and `remove` (e.g. logout).
- Idiom: `fn cart(cx: &Cx) -> CookieStore<Cart, impl Cookies> {
  cookie_store(signed_cookies(cx), "cart").parse_or_default() }`.

## Sessions (topcoat-session, feature `session`, on by default)

Topcoat implements the **mechanics** (token generation, transport, lifecycle);
**you own the storage** — it hands you a SHA-256 `TokenHash` + expiry to
persist next to your user id; no session table or user model is dictated. The
client holds a 32-byte random token, by default in a hardened cookie
(`__Host-` prefix, `Secure`, `HttpOnly`, `SameSite=Lax`, `Path=/`). The raw
token is never stored server-side.

### Setup

```rust
use topcoat::{
    cookie::RouterBuilderCookieExt,
    router::Router,
    session::{RouterBuilderSessionExt, SessionConfig},
};

let router = Router::builder()
    .cookies()                                // the default token store needs the jar
    .sessions(SessionConfig::default())
    .build();
```

### Lifecycle (all take `cx: &Cx`, all async)

- `session::start(cx).await?` → `Session { token_hash, expires_at }` — mints a
  fresh token (never reuses a presented one → fixation-safe), issues it to the
  client. Call on login; persist hash + expiry.
- `session::token_hash(cx).await?` → `Option<TokenHash>` — the presented
  token's hash; look it up in your storage (treat unknown/expired as
  unauthenticated). Read once per request and cached.
- `session::stop(cx).await?` → `Option<TokenHash>` — client discards token;
  delete the record. ("Sign out everywhere" = delete the other records.)
- `session::refresh(cx).await?` → `Option<Session>` — re-issues with a full
  lifetime (sliding expiration); push your record's expiry forward.
- `session::rotate(cx).await?` → `Option<Rotation>` (`rotation.revoked` hash to
  delete, `rotation.session` to record) — fresh token, same session; use on
  privilege change.

`start`/`stop`/`rotate` update the request's cached view, so a page rendered
after login sees the new session immediately.

### Login/logout/current-user (from `examples/session`)

```rust
use topcoat::{
    Result, context::{Cx, app_context},
    router::{content::Form, error::{SeeOther, see_other}, href, route},
    session::{self, TokenHash},
};

#[route(POST "/login")]
async fn login(cx: &Cx, Form(form): Form<LoginForm>) -> Result<SeeOther> {
    // verify credentials first, then:
    let session = session::start(cx).await?;
    db(cx).create(session, User { name: form.name });
    Ok(see_other(href!(page).resolve(cx)))
}

#[route(POST "/logout")]
async fn logout(cx: &Cx) -> Result<SeeOther> {
    if let Some(token_hash) = session::stop(cx).await? {
        db(cx).delete(&token_hash);
    }
    Ok(see_other(href!(page).resolve(cx)))
}

async fn current_user(cx: &Cx) -> Result<Option<User>> {
    let Some(hash) = session::token_hash(cx).await? else { return Ok(None); };
    Ok(db(cx).read(&hash))       // only while unexpired; wrap in #[memoize] if hot
}

// guarding a page:
let user = current_user(cx).await?.ok_or_redirect("/login")?;
```

### Configuration

```rust
use std::time::Duration;
use topcoat::session::{SessionConfig, cookie::CookieTokenStore};

let config = SessionConfig::builder()
    .token_store(CookieTokenStore::new().name("id"))   // default cookie name: "session"
    .lifetime(Duration::from_hours(24 * 14))           // default lifetime: 30 days
    .build();
```

The lifetime is both the cookie `Max-Age` and the `expires_at` you receive.

### Custom token stores

`TokenStore` is the client-side **transport** (not the session DB). Implement
`read`/`write`/`delete` (each returns `TokenStoreFuture`) to carry the token
elsewhere, e.g. a `Bearer` header for API clients; `Token::encode()` /
`Token::decode()` are URL-safe base64. Full example in
`crates/topcoat/docs/session.md` ("Custom token stores").

### Security notes (upstream)

- Keep every state-changing route on POST (or other non-GET): `SameSite=Lax`
  still sends the cookie on top-level cross-site navigations, and the router's
  `OriginPolicy` (see routing.md) rejects cross-origin non-safe methods but
  deliberately not GET.
- Compare sessions by hash lookup; never store or log the raw token.


## Field notes (verified in production use, 2026-08-21)

- **Cookie values are percent-encoded on `Set-Cookie`**: base64 `=`
  padding arrives as `%3D`. Decode before parsing a token out of a
  test's `Set-Cookie` header.
- **Ext-layer ordering is part of the layer stack**: "last registered
  = outermost" includes the `.cookies()`/`.sessions()` ext layers, and
  `RouterBuilderCookieExt` wants cookies registered *after* same-path
  layers that need the jar. Concretely: `.discover()` (your own root
  layer) must precede `.cookies().sessions(...)` in the builder chain
  or your layer runs outside the session machinery and
  `session::token_hash(cx)` sees nothing.
- Minor: `Token::decode` rejects both bad base64 and wrong length;
  `session::Session.expires_at` is `web_time::SystemTime`.
