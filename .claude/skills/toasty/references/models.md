# Models: derive, attributes, relations, types (toasty v0.10.0)

Sources: `docs/guide/src/{defining-models,field-options,keys-and-auto-generation,
indexes-and-unique-constraints,belongs-to,has-many,has-one,many-to-many,
embedded-types,json-encoding,deferred-fields,concurrency-control}.md` and
`examples/` at tag `toasty-v0.10.0`.

## One complete realistic model set

```rust
#[derive(Debug, toasty::Model)]
struct Account {
    #[key]
    #[auto]                     // uuid::Uuid + #[auto] = UUID v7 (time-ordered)
    id: uuid::Uuid,

    #[unique]                   // generates get_by_email / filter_by_email /
    email: String,              //   update_by_email / upsert_by_email / delete_by_email

    name: String,
    bio: Option<String>,        // nullable column

    #[index]                    // non-unique index; generates filter_by_status etc.
    status: AccountStatus,      // embedded enum → native PG ENUM type

    #[column(type = jsonb)]     // JSON fields REQUIRE an explicit column type (0.9+)
    settings: toasty::Json<Settings>,   // Settings: serde Serialize + Deserialize

    #[auto]                     // name+type heuristic: = #[default(jiff::Timestamp::now())]
    created_at: jiff::Timestamp,
    #[auto]                     // = #[update(jiff::Timestamp::now())] (create AND update)
    updated_at: jiff::Timestamp,

    #[version]                  // optimistic concurrency; u64, toasty-managed (starts at 1)
    version: u64,

    #[has_many]
    messages: toasty::Deferred<Vec<Message>>,
}

#[derive(Debug, PartialEq, toasty::Embed)]
enum AccountStatus { Active, Suspended }   // stored as "active"/"suspended"

#[derive(Debug, toasty::Model)]
struct Message {
    #[key]
    #[auto]
    id: uuid::Uuid,

    #[index]
    account_id: uuid::Uuid,
    #[belongs_to]               // key/references inferred: key = account_id, references = id
    account: toasty::Deferred<Account>,

    body: toasty::Deferred<String>,  // deferred column: omitted from default SELECT
}
```

## Attribute reference (exhaustive for 0.10)

| Attribute | Where | Effect |
|---|---|---|
| `#[key]` | field(s) | Primary key. Multiple fields → composite key, `get_by_a_and_b(...)`. |
| `#[key(a, b)]` / `#[key(partition = a, local = b)]` / `#[key(partition = [a, b], local = [c])]` | struct | Composite key; partition/local matters on DynamoDB, flat composite on SQL. Partition-only prefix gets `filter_by_<partition>()`. |
| `#[auto]` | key field | Auto-generate: integers → auto-increment, `uuid::Uuid` → UUID v7. Explicit: `#[auto(increment)]`, `#[auto(uuid(v7)))]`, `#[auto(uuid(v4))]`. |
| `#[auto]` | `created_at`/`updated_at` (`jiff::Timestamp`) | Timestamp shorthand (see model above). Requires the `jiff` feature. |
| `#[unique]` | field | Unique index; generates `get/filter/update/upsert/delete_by_*`. |
| `#[index]` | field | Non-unique index; same methods **except** `upsert_by_*` (its `get_by_*` errors if ≠1 match). |
| `#[index(a, b)]`, `#[unique(a, b)]` | struct | Composite index/unique; prefix `filter_by_*` methods generated per leftmost prefix; optional `name = "..."`, `partition =`/`local =` modes. |
| `#[column("name")]` | field | Column rename (Rust field name unchanged). |
| `#[column(type = ...)]` | field | Explicit DB type: `boolean`, `int`/`i8..i64`, `uint`/`u8..u64`, `text`, `varchar(N)`, `json`, `jsonb`, `numeric(P,S)`, `binary(N)`, `blob`, `timestamp(P)`, `date`, `time(P)`, `datetime(P)`, `cidr`, `inet`, `macaddr`, `macaddr8`. Validated against driver capability at `push_schema` (e.g. `varchar` rejected on SQLite). |
| `#[default(expr)]` | field | Any Rust expr, evaluated at insert time; applies on create + upsert-create branch. |
| `#[update(expr)]` | field | Evaluated on create AND every update (unless field set explicitly). |
| `#[version]` | `u64` field | OCC: instance update/delete condition on the loaded version and increment atomically; conflict → `Error::condition_failed` (`err.is_condition_failed()`). Query-based updates increment but never fail. All drivers. |
| `#[table = "name"]` | struct | Override auto-pluralized table name. |
| `#[belongs_to]` | relation field | FK relation. `key` defaults to `<field>_id`, `references` defaults to `id`. Explicit/composite: `#[belongs_to(key = [a, b], references = [id, rev])]`. |
| `#[has_many]` | `Deferred<Vec<T>>` or eager `Vec<T>` | Inverse of belongs_to. `pair = field` when the child's relation field name ≠ singular parent name. |
| `#[has_many(via = rel.path)]` | field | Multi-step derived relation (many-to-many traversal). Read-only, distinct targets, SQL-only for include/select. |
| `#[has_one]` | `Deferred<Option<T>>` | Child holds a `#[unique]` FK. |
| `#[document]` | embedded-struct field | One structured column, scalar leaves still filterable (no enum embeds inside). See `docs/guide/src/document-fields.md`. |

## Relations in practice

- Lazy (`Deferred<_>`): `post.user().exec(&mut db).await?` runs a query;
  `post.user.get()` is sync and panics if not preloaded (`try_get()` for Option).
- Eager (plain `T`/`Vec<T>`/`Option<T>`): loaded with every parent query; cycles of
  eager relations are rejected at schema build.
- Preload: `.include(User::fields().posts())` — chainable, mixes relation kinds,
  works on collection queries. A relation path converts into a typed `Include`,
  which accepts `.filter(expr)` and `.order_by(...)` to restrict/order the related
  rows loaded (`crates/toasty/src/stmt/include.rs`, added 0.9).
- Scoped accessor `user.posts()`: `.exec`, `.create()`, `.get_by_id`, `.filter`,
  `.filter_by_*().update()/.delete()`, `.insert(&mut db, &post)` (re-parent),
  `.remove(&mut db, &post)` (deletes child if FK required, NULLs it if optional).
- Many-to-many = explicit join model with two `#[belongs_to]` + `#[key(a_id, b_id)]`,
  endpoints declare `#[has_many]` to the join model and `#[has_many(via = joins.other)]`.
  Create/delete the join record to link/unlink; `via` is read-only.

## Type mapping (PostgreSQL column types)

`bool`→BOOL, `i8/i16`→SMALLINT, `i32`→INTEGER, `i64`→BIGINT, `u8`→SMALLINT,
`u16`→INTEGER, `u32/u64`→BIGINT (**u64 > i64::MAX rejects on insert**), `f32/f64`→
REAL/DOUBLE, `String`→TEXT, `Vec<u8>`→BYTEA, `uuid::Uuid`→UUID,
`rust_decimal::Decimal`→NUMERIC (native; `bigdecimal` falls back to TEXT),
`jiff::Timestamp`→TIMESTAMPTZ, `jiff::civil::{Date,Time,DateTime}`→DATE/TIME/TIMESTAMP,
`Vec<scalar>`→native arrays (`text[]`, …) with predicates `contains`, `is_superset`,
`intersects`, `len`, `is_empty` and update mutations `tags.push(x)`, `extend`, `pop`,
`clear`, `remove` (via `toasty::stmt` in `update!`). Embedded enums → named PG ENUM.

## Embeds, JSON, deferred — gotchas

- Newtype embed (`struct Email(String)`) = single column, no prefix; supports
  `#[key]`, `#[unique]`, `#[index]`, ordering ops, `#[auto]` proxying (`UserId(Uuid)`).
- Multi-field embeds flatten with `field_` prefixes; patch sub-fields with
  `toasty::stmt::patch(Address::fields().city(), "Portland")` or `update!` brace
  blocks; multi-field embeds can't carry `#[unique]`/`#[index]` on the parent field.
- Enum variants: rename via `#[column(rename_all = "SCREAMING_SNAKE_CASE")]` or
  per-variant `#[column(variant = "label")]` / integer `#[column(variant = 10)]`
  (no auto-numbering; no mixing string+int; ints default i64, narrow with
  `#[column(type = u8)]` on the enum or field). Filter with
  `.status().eq(Status::Active)`, `.status().is_active()`, data variants with
  `.contact().email().matches(|e| e.address().eq(...))`.
- `Json<T>` / `serde_json::Value` (feature `serde`): **no typed paths into the
  payload** — whole-value read/write only; `Option<Json<T>>` = SQL NULL vs
  `Json<Option<T>>` = JSON null.
- `Deferred<T>` on scalars/embeds defers the column from SELECT; load with
  `.include(Model::fields().body())`; `.get()` panics unloaded; filtering/sorting on
  a deferred field works without loading it. Does not wrap relation attrs — relations
  put `Deferred` in the field type instead.
