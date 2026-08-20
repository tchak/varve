# Queries: CRUD, filters, dynamic construction, pagination (toasty v0.10.0)

Sources: `docs/guide/src/{querying-records,filtering-with-expressions,
sorting-limits-and-pagination,creating-records,updating-records,upserting-records,
deleting-records}.md`, `crates/toasty/src/stmt/{expr,path,query,include}.rs`.

## Create

```rust
// Macro (struct-literal syntax, shorthand fields work); returns a builder — exec runs it.
let user = toasty::create!(User { name: "Alice", email: "alice@example.com" })
    .exec(&mut db).await?;

// Builder directly, for conditional fields:
let mut b = User::create().name("Alice");
if cond { b = b.bio("Likes Rust"); }
let user = b.exec(&mut db).await?;

// Through a relation (FK auto-filled): toasty::create!(in user.todos() { title: "x" })
// Nested: toasty::create!(User { name: "A", todos: [{ title: "a" }, { title: "b" }] })
// Same-type batch: toasty::create!(User::[{...}, {...}]) → Vec<User> (atomic)
// Mixed batch: toasty::create!((User {...}, Post {...})) → tuple (atomic)
// Bulk: User::create_many().with_item(|c| c.name("A")).item(toasty::create!(User{...})).exec(...)
// Runtime N: collect builders into Vec, toasty::batch(vec).exec(&mut db).await?
```

Setters take `impl IntoExpr<T>`: `&str`/`String`/`&String` for strings, values or refs
for numerics — no `.clone()` gymnastics.

## Read

```rust
let u  = User::get_by_id(&mut db, &id).await?;          // immediate; Err if missing
let u  = User::get_by_email(&mut db, "a@x.com").await?; // from #[unique]
let q  = User::filter_by_email("a@x.com");              // builder, customize further
let vs = User::all().exec(&mut db).await?;              // Vec<User>
let o  = User::all().first().exec(&mut db).await?;      // Option<User>
let u  = User::filter_by_id(id).get(&mut db).await?;    // exactly one, else Err
let n  = User::all().filter(...).count().exec(&mut db).await?;  // u64, SELECT COUNT(*)

// Projection (SELECT subset): single path → Vec<Field>, tuple → Vec<(..)>
let names: Vec<String> = User::all().select(User::fields().name()).exec(&mut db).await?;
let pairs: Vec<(u64, String)> =
    User::all().select((User::fields().id(), User::fields().name())).exec(&mut db).await?;
```

**ABSENT: no row-streaming API.** Terminals are `exec` (Vec), `first`, `get`,
`count`, `paginate` — there is no `stream()`/`fetch()` cursor iterator on `Query`
(`crates/toasty/src/stmt/query.rs`). Walk large sets with cursor pagination.

## Filter expressions

`Model::fields()` → typed `Path`s; comparisons yield `Expr<bool>`:
`.eq .ne .gt .ge .lt .le .between(lo, hi) .in_list([..] or Vec) .is_none() .is_some()
.starts_with(prefix)` (all backends, case-sensitive), `.like(pat)` (SQL only,
backend-native case rules), `.ilike(pat)` (**PostgreSQL only**, else
`unsupported_feature`). Combine: `.and(e) .or(e) .not()` / `!e`. Chained
`.filter(...)` calls AND together. Precedence is left-to-right wrapping:
`a.or(b).and(c)` = `(a OR b) AND c`; group by nesting arguments: `a.or(b.and(c))`.

Relation paths traverse: `User::fields().profile().score().gt(50)` (subquery through
HasOne/BelongsTo chains, SQL-only). HasMany: `.todos().any(Todo::fields()...)` /
`.all(...)` (vacuously true when empty; SQL-only for `.all`).

## Dynamic / runtime query construction (spike Q1)

**Everything above is a first-class value — nothing requires macros or generated
methods beyond the per-field path accessors.** `Expr<T>` is `Clone` + `Debug` and
freely built at runtime (`crates/toasty/src/stmt/expr.rs`):

```rust
use toasty::stmt::Expr;

// Fold an arbitrary runtime list of predicates. and_all of [] == no filter (true).
let mut clauses: Vec<Expr<bool>> = Vec::new();
if let Some(kind) = kind_param   { clauses.push(Event::fields().kind().eq(kind)); }
if let Some(min) = seq_min       { clauses.push(Event::fields().seq().ge(min)); }
clauses.push(
    Event::fields().status().eq("open").or(Event::fields().status().eq("stale")),
);
let filter = Expr::and_all(clauses);            // nested and/or tree, built at runtime
let rows = Event::filter(filter).exec(&mut db).await?;

// Runtime IN-list from a Vec (IntoExpr<List<T>> for Vec<U>):
let ids: Vec<i64> = load_ids();
let e = Expr::in_list(Event::fields().id(), ids);   // or path.in_list(ids)

// Relation subquery predicates are runtime-composable too — .any()/.all() are plain
// methods returning Expr<bool>, lowered to `parent_key IN (SELECT ...)`:
let f = User::fields().todos().any(Todo::fields().complete().eq(false));
```

Supporting machinery, all public:
- `Expr::and_all(impl IntoIterator<Item: IntoExpr<bool>>)`, `.and`, `.or`, `.not`,
  `Expr::in_list(lhs, rhs)` — `crates/toasty/src/stmt/expr.rs`.
- `Query::filter(Expr<bool>)` / `set_filter`, `order_by`/`set_order_by`, `limit`,
  `offset`, `include`, `delete()`, `count()`, `select()` — `stmt/query.rs`.
- Escape hatch into the raw AST: `Expr::<T>::from_untyped(toasty_core::stmt::Expr)`
  and `Query::from_untyped` exist, and the core AST has `Exists`, `InSubquery`,
  `Func`, `Cast` variants (`toasty-core/src/stmt/expr.rs`).

**Limits (documented absences):**
- **No typed EXISTS/IN-subquery builder against an arbitrary model.** Correlated
  subqueries are only expressible through *declared relation paths*
  (`.any()`/`.all()`/path traversal). `Expr::Exists` exists in the AST but no public
  constructor produces it, and hand-building resolved column references via
  `from_untyped` is engine-internal territory — for an ad-hoc EXISTS use
  `toasty::sql::query` (raw SQL) instead.
- Field paths come from generated `fields()` accessors; there is no public
  string-keyed "column by name" path builder (paths are index-based:
  `Model::path_field::<T>(n)` exists but is codegen support).
- `toasty::query!` macro (0.10) is compile-time sugar only: `FILTER`/`ORDER BY`/
  `OFFSET`/`LIMIT` over one model's own fields, `#var`/`#(expr)` splices; no
  associations, no EXISTS (that syntax is a design proposal in
  `docs/dev/design/query-macro.md`, not shipped).

## Update

```rust
toasty::update!(user { name: "Alice Smith", email: "a@x.com" }).exec(&mut db).await?;
user.update().name("Bob").exec(&mut db).await?;              // builder (conditional sets)
toasty::update!(User::filter_by_id(id) { name: "Bob" })      // query-target: bulk UPDATE,
    .exec(&mut db).await?;                                    //   no rows loaded
User::update_by_id(id).name("Bob").exec(&mut db).await?;     // = filter_by_id(id).update()
// Atomic relative ops (server-side, race-free):
toasty::update!(acct { balance.add(100) });  // also subtract / increment / decrement
// Vec<scalar>: tags.push("x"), extend([..]), pop(), clear(), remove("x")
// Embedded: meta: { version: 2 } patches sub-fields; None: bio: Option::<String>::None
```

## Upsert (0.9+; PostgreSQL/SQLite/Turso yes, **MySQL: unsupported_feature**, DynamoDB PK-only)

```rust
let user = User::upsert_by_email("a@x.com")     // conflict target = PK or #[unique] only
    .on_create(|u| u.name("Alice").login_count(0))
    .on_update(|u| u.login_count(toasty::stmt::increment()))
    .exec(&mut db).await?;
// Plain setters apply to both branches; or_ignore() → Option<User> (None on conflict).
// Shared mutations (increment/push/...) need #[default] on the field for the create branch.
```

## Delete

`user.delete().exec(&mut db).await?` (consumes self; version-guarded if `#[version]`);
`User::delete_by_id(&mut db, id).await?`; any query `.delete().exec(...)`.

## Sort, limit, pagination

```rust
Post::all().order_by(Post::fields().created_at().desc())        // or .asc(); tuple for
    .order_by(Post::fields().id().asc())                        //   multi-key (appends)
    .limit(20).offset(40)                                       // offset REQUIRES limit
// .latest_by(field) = order_by(field.desc())

// Cursor pagination (requires order_by; toasty appends PK tie-breakers itself):
let page: toasty::stmt::Page<_> = Post::all()
    .order_by(Post::fields().id().desc())
    .paginate(50)              // .after(cursor) / .before(cursor) to resume
    .exec(&mut db).await?;
// page derefs to slice; page.next(&mut db).await? / .prev / .has_next() / .has_prev()
// prev/backward pagination: SQL backends yes, DynamoDB no.
```

`limit(n)`/`per_page` are upper bounds, not guarantees — post-filtering can shrink a
page; detect the end with `has_next()`, never by page size.
