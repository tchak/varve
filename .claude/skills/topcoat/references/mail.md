# Mail (topcoat-mail)

Sources: `crates/topcoat/docs/mail.md`, `crates/topcoat-mail/macro/docs/mail.md`.
Verified at `topcoat-v0.6.2`. Features: `mail` (+ `mail-smtp` for SMTP).

```toml
[dependencies]
topcoat = { version = "0.6.2", features = ["mail", "mail-smtp"] }
```

## Setup

Register a `MailConfig` wrapping a transport (`RouterBuilderMailExt::mail`);
call sites are transport-agnostic:

```rust
use topcoat::{
    mail::{FileTransport, MailConfig, RouterBuilderMailExt},
    router::{Router, RouterBuilderDiscoverExt},
};

pub fn router() -> Router {
    Router::builder()
        .discover()
        .mail(MailConfig::builder()
            .transport(FileTransport::new("target/mail"))
            .build())
        .build()
}
```

## Declaring and sending

`mail!` is an expression producing `Result<Mail>`; it expands to an awaited
async block (must be inside an async fn), so field values may use `.await` and
`?` directly. `send(cx, mail).await?` delivers through the registered
transport and returns a `Receipt` (carries the `Message-ID`; a receipt means
accepted-for-delivery, not delivered-to-inbox).

```rust
use topcoat::{Result, context::Cx, mail::{mail, send}, router::route};

#[route(POST "/api/welcome")]
async fn welcome(cx: &Cx) -> Result<&'static str> {
    let mail = mail! {
        from: ("Topcoat", "welcome@example.com"),
        to: "ada@example.com",
        subject: "Welcome, Ada!",
        html: {
            <h1>"Welcome!"</h1>
            <p>"Your account is ready."</p>
        },
    }?;
    send(cx, mail).await?;
    Ok("sent")
}
```

Fields (any order, each at most once): `from` (single address), `to`/`cc`/
`bcc`/`reply_to` (address or collection), `subject`, `html` (braced `view!`
body or a `View` expression), `text` (default: derived from HTML; declare your
own, or `TextBody::None` for HTML-only), `attachments`, `headers`
(`(name, value)` pair or collection), `in_reply_to`/`references` (threading;
feed them a stored `Receipt`'s message id), `date`/`message_id` (generated at
send time unless declared).

- Addresses: `Mailbox`, bare string `"ada@example.com"`, display form
  `"Ada Lovelace <ada@example.com>"`, `(name, address)` pairs; flavors mix in
  collections (`TryIntoMailboxes`). Invalid addresses surface as the macro's
  `Err`.
- An HTML body that renders components / needs request context starts with
  `cx =>` like a plain-function `view!`:
  `html: { cx => <p>"Hello, " (name) "!"</p> }`. Keep mail markup plain with
  inline styles — mail clients understand little CSS.
- Attachments: `Attachment::new("invoice.pdf", "application/pdf", b"%PDF-")`
  (downloadable) and `Attachment::inline("logo", "image/png", bytes)`
  referenced from HTML via `src="cid:logo"`.
- `SendError` when the mail is incomplete (no From, no recipients, no body) or
  delivery fails. `MailBuilder` assembles a `Mail` without the macro;
  `Mail::formatted()` renders RFC 5322 wire form (for HTTP mail APIs).

## Transports

- **`SmtpTransport`** (feature `mail-smtp`): pooled connections.
  `SmtpTransport::relay("smtp.example.com")?` (implicit TLS :465),
  `.starttls(...)` (:587), `.credentials("user", "pass")`, `.build()`; or
  `SmtpTransport::from_url("smtps://user:pass@smtp.example.com:465")?.build()`
  (fits one env var).
- **`FileTransport::new(dir)`**: writes each mail as an `.eml` (dev).
- **`MemoryTransport::new()`**: captures in memory for tests; clones share the
  capture; assembles the mail fully, so a mail that would fail to send fails in
  the test too.

```rust
use topcoat::{
    context::CxTestBuilder,
    mail::{MailConfig, MemoryTransport, Mailbox, mail, send},
};

let transport = MemoryTransport::new();
let config = MailConfig::builder().transport(transport.clone()).build();
let cx = CxTestBuilder::new().app_context(config).build();

send(&cx, mail! { from: "ada@example.com", to: "bob@example.com", text: "Hi" }?).await?;
assert_eq!(transport.sent().len(), 1);
assert_eq!(transport.sent()[0].to(), [Mailbox::new("bob@example.com")?]);
```

Custom transports: implement the `Transport` trait (e.g. a provider's HTTP
API).
