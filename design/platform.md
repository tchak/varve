# platform.md — the platform above the kernel

Design document for the platform: the DN-successor web application and
HTTP API built on the varve kernel. `design/kernel.md` (DESIGN) remains
the single source of truth for the kernel (tiers 0–5); this document
owns `platform/`; `design/graphql.md` owns the public schema (split out
2026-08-22 — the platform document is the umbrella, and sections leave
it for their own file as they grow). DESIGN §13 fixes the boundary
between kernel and platform and is authoritative where they overlap.

**Conventions** — the same as `design/kernel.md`: open questions are never
deleted (struck through with a **Resolved** note and a pointer);
decisions record *how* they were settled; unknowns that touch the
kernel route to DESIGN §10 (open questions) or §12 (corpus questions),
platform-only unknowns to P.9 here. The second-system risk applies with
full force on this side of the boundary too: platform features earn
their place by existing in DN.

## P.1 Principles

1. **API-first, one schema, no private data paths.** The public GraphQL
   schema is the only way to read or write domain data — the app
   included. An app need the API cannot serve is an API gap, never an
   internal route; this is the discipline that makes "we dogfood it"
   true. Two declared carve-outs: session/authentication machinery and
   pure UI assets.
2. **The kernel narrow waist.** All kernel state flows through
   `varve-service` (DESIGN §13.2). Platform-owned tables (accounts,
   catalog, messages, …) are ordinary Toasty models; kernel objects are
   never touched at the store level from platform code.
3. **Authorization is surface assignment** (DESIGN §2.9). The platform
   maps principals to parties and surface assignments; there is no
   second permission model over kernel data. Platform-only resources
   (messages, tokens, catalog admin) get ordinary platform
   authorization.
4. **Dogfooding is the design method.** The app is integrator #1: it
   executes the same schema in-process that integrators call over HTTP,
   through the same principal context. The wire format is dogfooded at
   the fidelity edges (exports, signed logs — DESIGN §13.4).
5. **Accessible by construction.** The platform is the successor of a
   French public-service site, so accessibility is a legal duty
   (RGAA 4.1 — loi 2005-102, décret 2019-768), not a polish item.
   Target: **RGAA 4.1 / WCAG 2.2 AA** on every page. The means are
   structural, not retrofitted: server-rendered HTML that is complete
   without JavaScript; semantic landmarks and a single `<h1>` per
   page; every control labelled, every error linked to its control
   (`aria-describedby` / `aria-invalid`), every flash announced
   (`role="alert"`); keyboard-complete with visible focus; `lang` on
   every document. Each test level owns its share of the proof
   (CLAUDE.md, platform test policy): component tests own aria
   wiring, router tests run the static baseline lint over every HTML
   response they fetch, e2e owns keyboard journeys and the rule
   engine (axe-core) in a real browser. Vendored registry components
   are pinned byte-for-byte (`registry_sync`); an accessibility defect
   inside one is an upstream fix, never a local patch. Automated
   checks catch roughly a third of WCAG failures: they stop
   regressions, they are not the audit (P.9 Q12).

## P.2 Stack

- **topcoat** (tokio-rs, announced 2026-07): server-rendered reactive
  app; components paired with colocated GraphQL fragments (P.9 Q2).
  Topcoat is also the HTTP layer — it sits on hyper directly, with its
  own router (`#[route]` API routes alongside `#[page]`/`#[layout]`,
  tower layers, and a `tower` interop module for mounting tower
  services). No separate web framework. **Settled 2026-08-19** (was
  listed as a fourth stack item, axum): `/graphql`, upload slots, and
  export downloads are ordinary topcoat `#[route]` handlers; execution
  is `schema.execute(request)` in-process, so the transport binding is
  a few lines of handler, not a framework. Worst case, a tower service
  mounts through the interop module.
- **toasty** (tokio-rs): platform models, and the first `varve-store`
  substrate (DESIGN Q19 — gated on the dynamic-query spike, P.9 Q1).
- **async-graphql**: schema, resolvers, in-process execution; no
  transport adapter crate (`async-graphql-axum` etc.) — the topcoat
  route handlers above are the transport.
- **ICU4X + MessageFormat 2.0** for i18n, from the start. **Settled
  2026-08-19.** Two decisions, kept apart: the formatting/data layer
  is **ICU4X** (the `icu` crate — Unicode's official pure-Rust
  implementation: locale negotiation, plural rules, number/date/list
  formatting, collation; `rust_icu`/ICU4C bindings rejected — a C
  dependency and version skew for nothing ICU4X lacks). The message
  syntax is **MF2** (Unicode's successor to classic MessageFormat,
  final since LDML 47, tracking LDML 48.x): catalogs are spec-standard
  MF2 text files, so the stable contract is the asset format, not any
  crate — the same structural hedge as the rest of this stack.
  Rejected: MF1 (superseded by Unicode itself; Rust support is
  parser-only anyway, so it shares MF2's runtime cost while betting on
  the sunset format — its one edge, translation-platform support, is
  eroding in MF2's favor) and Fluent (the most mature native-Rust
  runtime, and MF2's chief ancestor, but not ICU; it is the fallback
  if the MF2 runtime spike fails — P.9 Q8). Server-rendered topcoat
  means zero client-side i18n runtime. English + French catalogs from
  day one (P.4's built-to-travel stance).
- **underway** for durable queue jobs (Postgres-native, transactional
  enqueue — the decisive property). **Settled 2026-08-19**, full
  rationale and rejected alternatives in P.13; gated on the P.9 Q9
  spike. Sweeps (resolution retries, scans, blobs, outbox) are
  hand-rolled — no crate — per P.13.

All of topcoat/toasty are early-stage with breaking changes expected.
The hedges are structural: the `varve-store` trait (swap the ORM), the
schema (transport-independent by construction), and thin resolvers
(use-case logic lives in `platform-core`, not in the framework).

## P.3 Crates (`platform/`, `publish = false` permanently)

**Settled (2026-08-20): one root workspace.** The `platform/` crates
are members of the kernel repo's root Cargo workspace, not a nested
workspace (DESIGN §13.5 carries the full argument). Pre-publish ruthless refactoring (CLAUDE.md) wants
kernel↔platform breakage surfaced by the single
`cargo test --workspace`; the accepted cost is kernel-lockfile churn
from the fast-moving topcoat/toasty line. DB-backed tests gate behind
an env var so the workspace tests run without Postgres (CI provides a
service container); the §13.5 layering guard already treats
`platform/` as same-tree, and its kernel-closure checks are unaffected
by new members.

- `platform-core` — Toasty models for platform-owned data (accounts,
  procedure catalog, team membership, messages, API tokens, webhook
  subscriptions, notification outbox) and the **use-case services**:
  each use case composes one `varve-service` operation with its
  platform side effects — submit case file = kernel append + system
  message + notification + webhook fan-out — in exactly one place.
- `platform-store` — the Toasty implementation of the `varve-store`
  traits (the name mirrors the kernel crate: `varve-store` defines,
  `platform-store` implements). The kernel tables — record-log rows
  keyed `(record, seq)`, publication events, block/nomenclature
  version rows, revision objects, surfaces — are Toasty models kept
  `pub(crate)`, making P.1's "kernel objects are never touched at the
  store level from platform code" structural rather than a comment.
  Depends on `varve-store` + toasty only, nothing platform-side:
  `platform-core`'s use-case services stay generic over the store
  traits (tested against `MemoryStore`, the §13.2 oracle),
  `platform-server` wires this impl in, and the same conformance
  suite reruns here against Postgres. This crate is P.2's "swap the
  ORM" hedge made concrete and the subject of the Q1 spike (DESIGN
  Q19): if toasty cannot carry the substrate, the replacement is this
  one crate. **Cross-table atomicity with platform writes (settled,
  was P.9 Q10):** the store is a *scoped* value, not a service — a
  use case in `platform-core` opens one toasty transaction, constructs
  the `platform-store` impl over `&mut` that transaction, runs its
  platform-model writes and its kernel writes (through the store)
  against the same transaction, and commits once. Two consequences:
  the impl holds its executor behind an async mutex, because the
  `varve-store` traits take `&self` and toasty's `exec` takes `&mut
  dyn Executor`; and no store handle ever lives in app context,
  because `Transaction<'a>` borrows the `Db`. The traits stay
  executor-free; the binding happens at construction, per use case.
  **Amended 2026-08-24 (migrations and registration):** the kernel
  tables are `platform-store`'s own migration set (its own
  `toasty/` directory and project-local migrate CLI, the
  `platform-core` pattern), and the crate exposes its `ModelSet`;
  `platform-core::connect_with` merges extra models and applies
  extra migration sets after its own — one database, one
  `__toasty_migrations` table, each crate owning its files. Callers
  that never touch kernel state keep plain `connect`.
- `platform-i18n` — the MF2 catalogs (English + French) and their
  runtime over ICU4X (correction, found in the Q8 spike: parse with
  `ox_mf2_parser` — the originally named `mf2_parser` is GPL-3 and
  parser-only, offering nothing over the MIT candidate), implement
  the MF2
  function registry (`:number`, `:datetime`, plural selection) by
  delegating to ICU4X (P.9 Q8, resolved: the interpreter is
  hand-rolled — no existing runtime crate qualified). Locale is a plain
  argument: resolved in `platform-app` (`Accept-Language`, principal
  preference), passed as ordinary context — no crate below this one
  knows what a locale is.
- `platform-graphql` — schema and resolvers over
  `Context { principal }`; knows nothing of sessions or tokens.
- `platform-app` — the Topcoat app: sessions, principal resolution,
  in-process document execution, components with colocated fragments.
- `platform-client` — the typed client, a **leaf crate** (depends on
  no other platform crate): the checked-in SDL (`schema.graphql`, the
  published contract), operations and fragments as `cynic` derives
  validated against that SDL at build time, and a `Transport` trait
  over GraphQL-over-JSON documents. Two transports: in-process
  (implemented in `platform-graphql`, used by the app's components and
  by resolver tests) and HTTP (bearer token, `http` feature, for Rust
  integrators and the P2 HTTP-path tests). See P.9 Q2.
- `platform-jobs` — both shapes of background work (P.13): the
  hand-rolled sweeps (tick + advisory-lock lease + per-resolver
  circuit state) driving `varve-service` steps — resolution retries,
  scan sweep, blob sweep, outbox drain — and the underway queue jobs
  (webhook delivery, emails, export generation, batch updates) with
  transactional enqueue from the use-case services.
- `platform-server` — the binary: the topcoat router (app + `/graphql`
  and upload/download `#[route]` handlers) + the `platform-jobs`
  runners (sweep ticks and queue workers — P.13). One process to start
  with; the seams are already crates.

## P.4 Domain model

**Vocabulary (settled 2026-08-19).** The platform speaks English-first
names — it is built to travel beyond France — with the DN term recorded
once, in parentheses, as provenance. Settled: **Procedure** (procédure
— kept; real English with the right administrative register; *service*,
*process*, *scheme* rejected for tech collisions and dialect skew),
**CaseFile** (dossier — DESIGN §2.9's own thesis language;
*application*/*submission* rejected as contradicting the case-file
model, bare *Case* rejected as a reserved word in Rust/Swift/Java
codegen), **applicant** (demandeur), **reviewer** (instructeur —
coheres with the `UNDER_REVIEW`/`startReview` lifecycle vocabulary;
*caseworker* rejected for its UK social-services register, *case
officer* was the runner-up), **team** (groupe instructeur — *group*
rejected: collides with kernel `group`), **procedure administrator**
(administrateur), **messaging** (messagerie). Decision verbs are
**accept / refuse** (administrative register, 1:1 with DN semantics)
over approve/reject.

**Procedure** = catalog row (title, description, owning organization,
open/closed) wrapping a revision DAG + surfaces + rules. Publication is
the impact-gated `varve-service` operation (DESIGN §3): the mutation
returns the impact report, and a lossy or breaking publication requires
explicit confirmation carrying that report.

**CaseFile** = record log + derived state. **Lifecycle states are
checkpoints** (DESIGN §2.9, Q12): `DRAFT` → `SUBMITTED` (dépôt — a
checkpoint pinning the reading revision; the case file stays editable
by the applicant, which is the §2.9 thesis at work, not an oversight) →
`UNDER_REVIEW` (passage en instruction — the checkpoint that freezes
the applicant surface's writable set, per Q12: "instruction locks the
applicant form") → `ACCEPTED` / `REFUSED` / `CLOSED_WITHOUT_DECISION`
(classé sans suite) as terminal checkpoints; `returnToApplicant`
(repasser en construction) appends a superseding checkpoint. The
platform contributes only the state *machine* — which checkpoint may
follow which — and mirrors the current state as a read-model column
(authority: P.9 Q3).

**Principals**: applicant, reviewer, procedure administrator, plus API
tokens as non-human principals. Teams are platform tables whose entire
effect is surface assignment over a set of case files. **Routing rules
are varve-logic predicates** evaluated at submission to pick the team —
the same language as visibility rules and queries.

**Organizations and membership (settled 2026-08-21).** An
**Organization** (the owning administration — DN's *administrateur*
scope, but as a first-class table) owns procedures. Two independent
memberships, both plain account⟷X join tables **with no role column**:
*organization membership* means administering every procedure the
organization owns — it is the procedure-administrator principal, held
at the organization rather than per procedure (DN assigns
administrators procedure by procedure; one organization-level fact
replaces that table); *team membership* means being a reviewer. Teams
belong to an **organization, not a procedure** — a departure from DN,
where a groupe instructeur is per procedure and identical groups get
re-created across an administration's procedures; an organization's
teams are reusable, and a procedure's routing rules select among them.
The two memberships do not imply each other: a reviewer is commonly
not an organization member. Roles are deliberately absent — membership
*is* the right; a finer grant appears only if the corpus shows demand
(P.9 Q13 holds the one known follow-up: which of the organization's
teams a given procedure may route to).

**Messaging**: platform entities keyed by case-file id; system messages
are emitted by use-case services (state changes, resolver failures, …).
Deliberately not kernel data — the log is cells, not chat (DESIGN
§2.9).

**Procedure lifecycle (settled 2026-08-24).** The catalog row's
open/closed is a three-state machine — `Draft` (never published) →
`Published { since }` ⇄ `Closed { since }` — hand-rolled in
`platform-core` as a plain enum with fallible transition functions:
three states and four transitions earn no state-machine crate, and
the case-file checkpoint machine will be the same shape (a `match`
on which checkpoint may follow which). Transitions: **publish**
lands in `Published` from any state — it consumes the revision
draft, appends the kernel publication event (DESIGN §2.1), and from
`Closed` it *is* the reopen; **close** only from `Published` (a
never-published procedure is deleted, not closed — G.2 rule 8);
**reopen** from `Closed` re-opens on the last published revision,
no kernel event. A publish-but-stay-closed variant (pre-staging a
schema for a later reopen) is refused until the corpus asks for it.
The row stores a `state` discriminant plus one `state_since`
timestamp under the existing `#[version]` guard — only the current
state's fact lives on the row; history is the event log's (below).
`since` means *since when the procedure is open* — deliberately not
named `published_at`, because `reopen` resets it with no publication
happening; revision publication timestamps are kernel facts
(`RevisionStore` publication events), and a catalog mirror of them
is a later read model if a list view asks. The **revision draft is
orthogonal to the state**: a draft may be in progress in all three
states (the first revision, the next, the reopen's), so "has a
draft" is neither an input nor an output of the machine. In the API
the state is the G.2 rule 5 union — `ProcedureDraftState |
ProcedurePublishedState | ProcedureClosedState` plus
`ProcedureStateValue` for filtering, generated from the one Rust
enum.

**Event logs (settled 2026-08-24): two tables, not one.**
`procedure_events` and `case_file_events`, separate because their
lifetimes and truths differ. Retention diverges: case-file events
are personal-data-adjacent and go with the case file under the
DESIGN §2.10 erasure guarantees, while procedure events are the
long-lived administrative audit trail — two tables make "erase the
case file" a clean cascade instead of row-level carve-outs in one
polymorphic table. Authority diverges: for procedures the platform
log *is* the primary record of close/reopen (the kernel has no
open/closed concept) and its `published` rows mirror kernel
publication events; for case files the kernel record log is
authoritative for lifecycle (checkpoints, DESIGN §2.9; the platform
state column is a read model, P.9 Q3), so `case_file_events` holds
only what the record log doesn't — team assignment, messages sent,
reviewer administrivia; its specifics land with case files (P1).
Shape, same for both: `id` UUID v7 (time-ordered, so also the
sequence), subject id (indexed), nullable `actor_account_id`
(system events), `kind` as a column (filtering), per-kind facts as
platform-owned JSON, `created_at` — written in the same transaction
as the state-column update by the use-case service, so column and
log cannot disagree. Two constraints: **draft autosaves are not
events** (the procedure alphabet is `created`, `published
{ revision, base }`, `closed`, `reopened`, `draft_discarded` —
per-edit logging is P.9 Q4's bloat, procedure-side), and
**case-file event payloads never hold cell values** — references
and metadata only, or the log becomes an erasure leak outliving
§2.10. The unified per-organization audit view, if ever wanted, is
two queries.

**Procedure drafts (settled 2026-08-23).** The kernel has no notion of
a schema *being edited*: a `Schema` is a plain value, a revision is
that value once published and content-addressed (DESIGN §2.1), and
`RevisionStore` holds publication events and objects only — an
unpublished, constantly changing schema has no place in the
hash-chained, erasure-bound kernel store and should not get one. The
draft is therefore **platform state**: `RevisionDraft` (named for
what it publishes as — "draft" alone will also be a case-file state),
a nullable embedded object on the catalog row
(`platform-core::procedure`), holding the schema as the kernel's own **wire-canonical bytes**
(`varve_wire::schema_bytes`, the body of a `revision` line, DESIGN §5)
plus `base`, the id of the published revision it forks from (the
publication's parent; `None` until the revision DAG lands). Rationale
for bytes over a normalized `columns` table or a JSON mirror: the
schema is a recursive tree the kernel already types (`Element`),
the only query a draft ever answers is "load the whole thing", and
the canonical encoding guarantees that what is stored is byte-for-byte
what `publishRevision` will hash — no second schema representation to
keep in sync, and no `serde` on kernel types outside `varve-wire`
(DESIGN §13.5). The column is deferred out of the catalog `SELECT`
and guarded by optimistic concurrency (`#[version]`): a stale editor
fails its save instead of overwriting.

**Editing is by element** (`platform-core::schema_edit`): add, update
(label / type / arity on a column, label / cardinality on a group),
move, remove — placement anchored on a sibling id (`before`, `None`
= append), never an index, since an index is only meaningful against
the tree the editor last saw — each atomic over the schema and
validated by the kernel
(`varve_schema::validate`, default depth policy) before it replaces
the draft. Two kernel facts shape the API: ids are identity (minted
once, opaque, never derived from labels — a type change must stay a
type change for the impact report, DESIGN §3, not a removal plus an
addition), and `required` / visibility / presentation are surface
properties (DESIGN §2.6) with no place in schema edits. The first
version carries the schema only; the **surface draft joins the same
`ProcedureDraft`** when the editor grows its surface side — an
administrator edits one form, and publishes schema and surface
together. Open: P.9 Q15.

**Surfaces on the draft (settled 2026-08-23).** Publication emits
**two kernel surfaces** over the same revision — the applicant form
and the reviewer screen. The published pair is forced by the kernel,
not chosen: entry visibility filters the log by a surface's *static
column set* (DESIGN §2.9), checkpoints freeze the writable set of
the surface they are taken through, and instruction locks the
applicant surface's writable set (per Q12) — so a reviewer-only
column must be genuinely *absent* from the applicant surface, not
flagged hidden, or the applicant's redacted log would leak
instructor writes. The draft, however, holds **one authored surface
tree**: the reviewer-ordered superset, each element (column or
group) carrying a **`private` marker** — DN's annotations privées —
and publication deterministically compiles the pair from it:
reviewer = the full tree, applicant = the tree with private subtrees
pruned, plus the write-policy differences. One tree, not two
maintained drafts, because every structural edit lands once — two
hand-kept trees drift, and the characteristic drift failure is a
private column left on the applicant tree, a privacy breach authored
by tooling, which pruning makes unrepresentable — and because
interleaving falls out: DN's trailing "annotations privées" section
was an artifact of keeping two lists, while here a private element
sits wherever the administrator put it, the deliberate improvement
over DN. The marker is one more per-element property in the editor's
element-operation shape. The reviewer view badges private elements
from the marker, which survives on the published version — the
compiled surfaces cannot disagree with it, so no surface diffing;
even from the surfaces alone the badge would be a membership check
against the applicant surface's static column set, the set entry
filtering already uses. The kernel stays out of it: no `private` on
kernel surface nodes — privacy is a relation between *paired*
surfaces, and the kernel has neither surface pairing nor actors
(DESIGN §2.9). Structural constraints, checked at edit and publish
time: a private node's whole subtree is private (a public child
under a private parent would be an orphan on the applicant surface —
refused); a private column inside a public `many` group is legal — a
per-item reviewer note, expressible where DN was not. The marker is
a bool for now — the corpus shows exactly DN's public/private pair;
a third interleaved audience would grow it into an enum on the same
tree, and genuinely independent surfaces (export layouts, print
templates) remain their own authored artifacts, never compiled from
this one. **Amended 2026-08-24: the marker ships as `audience:
Audience` (`all` | `reviewer`) from day one.** Two things moved it
off the bool: the third audience is already visible — DN's experts
(avis externes) see the case file but not the annotations privées,
an interleaved audience from institutional memory, not speculation —
and `private` as a field name is a reserved word in most client
languages of the public API (Java, C#, C++, Swift, PHP, JS class
syntax), so generated clients would trip on it; `restricted` was the
runner-up, rejected because it names the narrow case rather than the
axis. Semantics unchanged: `reviewer` is DN's private; the effective
audience of an element is the narrowest along its ancestor path
(inheritance — moving an element into a reviewer-only section
narrows it without rewriting markers), and only *explicitly*
authoring an element wider than its parent's effective audience is
the refused contradiction. "Private" stays the prose word for the
concept; `audience` is the field.

**Publication (settled 2026-08-24).** The kernel edge arrives as
`varve-service::publish_revision`, the §13.2 choreography around the
pure kernel, generic over the `varve-store` traits: check the
caller's fork point against the lineage head, validate the schema
(`varve_schema::validate`) and both surfaces
(`varve_surface::validate`, which re-checks the revision pairing),
classify against the head with `varve-impact`, gate on the report —
`worst() > Safe` without explicit confirmation returns the report
and writes nothing — then append the publication event and put the
surfaces. **The lineage is the procedure's UUID** (`LineageId` is
storage scoping by design, §13.2). **First publication classifies
against the empty schema**: every column `Added`, the report free —
one code path, no special case. **The surface pair compiles from
the authored tree** with fixed ids `applicant` and `reviewer`
(stable across revisions — §2.6 node identity for the pair itself):
reviewer = the full tree, applicant = effective-audience pruning;
the write-policy differences, unspecified until now, are settled
from DN semantics and DESIGN §2.7's "back-office yes, public form
no": on the applicant surface every column present is writable with
`override_derived: false`; on the reviewer surface only
reviewer-only columns are writable (annotations privées are the
instructeur's; the dossier's own fields are never edited by
reviewers — corrections go through `returnToApplicant` and
messaging) and they carry `override_derived: true`. `required:
true` compiles to the vacuous always-required rule on every surface
the column appears on. **The empty-enum refusal is publication
policy here** (G.7: a choice with no options is a draft state; the
kernel deliberately accepts it), refused before the service runs.
**The catalog row gains `latest_revision`**, a read-model column
maintained only by publication (the P.9 Q3 pattern): a new draft
forks from it (its `base`), and publication refuses a draft whose
`base` no longer equals the lineage head — the stale-fork answer,
resolving the base-echo half of P.9 Q15 server-side (the draft row
carries `base`; the client echoes nothing). The `published` event
carries its facts (`revision`, `base`) as the event log's first
facts payload. Publication also keeps **the head's authored tree on
the row** (`published_tree`, set only here): the next draft forks
from its base's *tree* (G.8), and audiences and presentation nodes
exist nowhere kernel-side — the compiled pair is derived from the
tree, never the source a fork rebuilds from. **Transaction composition** follows the settled Q10
shape made concrete: the use case in `platform-core` receives one
shared executor (`tokio::sync::Mutex<&mut dyn Executor>` over the
open transaction) plus the store scoped over that same executor;
the GraphQL mutation is the composition point that opens the
transaction, scopes `platform-store`, calls the use case, and
commits — amending P.3's "platform-server wires this impl in":
in-process execution makes the mutation the wiring point;
`platform-server` still owns choosing the database.

**Conditions across the privacy boundary (settled with the above).**
The authoring surfaces adopt DN's upstream-only rule, which DESIGN
§4.1 anticipates ("surfaces may impose the stronger upstream-only
authoring rule"): a rule's source columns must sit *above* the
conditioned element in document order. The privacy direction
composes with it into one statement: **sources must be upper in the
tree and at least as public as the conditioned element.** A private
element conditioned on public columns is the common DN case and
stays legal; a public element conditioned on a private column is
forbidden at edit and publication time — it would leak private data
through visibility flicker on the applicant form and make applicant
admissibility depend on columns the applicant cannot see. The kernel
keeps only acyclicity (DESIGN §4.1); both halves of this rule are
platform authoring/publication constraints.

**The authored tree is the draft's single source (settled
2026-08-23; revises the storage rationale above).** Sections and
notes — 18.3% of DN's descriptors (M0) — are surface nodes with no
place in the schema (DESIGN §2.6), so the tree the editor edits is
the authored surface tree itself: columns and groups interleaved
with sections and notes, each element carrying its surface
properties and its `audience` marker (amendment above). The draft stores that tree, and
publication *derives* everything from it: project the schema (strip
presentation nodes; schema document order = authored order),
validate and publish the revision, then compile the applicant and
reviewer surfaces (pruning, above) against it. Storing schema and
surface drafts side by side was rejected: keeping them in sync
reintroduces the drift class the single tree exists to kill —
orphan columns placed on no surface, double placements, order
disagreements. The stored draft is therefore no longer the kernel's
schema bytes ("byte-for-byte what publishRevision hashes") but a
platform object — it holds private markers, which no kernel object
carries — and the guarantee weakens only from "stored = hashed" to
"published is deterministically derived from stored", the property
surface compilation already relies on. The API and editor grow
accordingly (G.7): `addSection` / `addNote` and their updates
(title / help; text), the same sibling-anchored placement with
presentation nodes in the sibling universe, and *move to* gaining
sections as targets — moving into a group is a schema edit (scope,
row paths, impact); moving into a section is presentation only, and
the schema projection is what gets diffed, so the impact story
stays honest. Private composes: a private section hides its subtree
from the applicant (the subtree rule above); a private note is
reviewer-facing guidance DN's annotations privées never had. Node
identity is kernel identity (DESIGN §2.6, surface node identity):
sections and notes carry minted ids the editor's `?selected=`,
anchors and moves use directly — the same ids the compiled surfaces
carry, nothing platform-only. DN import maps flat HeaderSection
runs to container sections — a header swallows everything up to the
next same-level header — deterministic, stated here so the importer
and the editor agree.

**Required on columns (settled 2026-08-24).** `TreeColumn` carries
`required: bool` — the first surface property on the authored tree
(graphql.md, *Required on columns*): the editor's switch for DESIGN
§2.6's two constant cases; conditional requiredness waits for the
rule editor. New columns default by **effective** audience — public
required, reviewer-only optional — a deliberate inversion of DN's
optional-by-default authoring, backed by the corpus (M0: required
ratios run 65–88 % across the data-carrying types, so the common
case should be the default one).

**Format on text columns (settled 2026-08-24).** `TreeColumn`
carries `format: Option<varve_surface::Format>` — the second
surface property (graphql.md, *Format on text columns*), which
gives `platform-core` its first `varve-surface` dependency.
`tree_edit` keeps the kernel-edge backstops: text-only
(`FormatNotOffered`), patterns verified on the linear-time engine
at edit time (`InvalidPattern` — the no-ReDoS guarantee holds from
the first keystroke, not from publication), and a type change away
from text resets the format silently, the arity precedent. The
editor offers email / phone / IBAN / custom pattern on text
columns.

**The schema editor (settled 2026-08-23).** `platform-app`'s
`/organizations/{id}/procedures/{pid}/schema`, over the G.7 client
operations — the app is integrator #1 (P.1). Shape: **master–detail
with the state in the URL** (`?selected=<id>` picks the element whose
form the detail panel shows, `aria-current` on its row; deep-linkable,
readable, no client-side selection state), the structure as nested
lists with a row per element (label, type and multiplicity badges, an
actions menu), the draft's state in the header (badge, counts, last
save), and a one-shot notice after every action (`role=status` /
`role=alert`, the server's reason verbatim on a refused edit) in a
**slot that is always there at one height** (`aria-live=polite`), so
the panels never move when a notice comes or goes; notices name what
they acted on ("Added the column “Nom”.") and use the platform's own
`notice` component — a soft green or red tint, a colour outside
the theme tokens (Tailwind emerald/rose with dark values; recorded
here as the a11y contract asks — as is the second such colour, the
amber tint + eye-off glyph on the structure rows' reviewer-audience
badge, which must not read as one more type badge); a confirmation
fades out a few
seconds after render (pure CSS in `app.css`, `motion-safe:` only, the
live-region announcement unaffected), a refusal stays. Counts in the
header are CLDR-plural messages (CLAUDE.md, *UI strings*). **No
drag and drop**: moving is *up*, *down*, and *move to* (a group or
the top level) — named actions a keyboard and a screen reader reach,
and exactly what the API's sibling-anchored placement expresses;
*up/down* are computed server-side from document order. **Every
action is a form** (post → 303 → get), so the editor is complete with
no script and the router-level tests prove all of it; the topcoat
runtime is layered on top: the detail form **autosaves on change**
through a `#[procedure]` (the same field application the form POST
uses) — there is no *Save* button with the script running; it is
rendered in `<noscript>` for the script-less path, where a
`<select>` cannot submit on its own — reports through a
`role=status` line, and bumps a signal that
re-renders the structure panel, a `#[shard]`; kind-dependent
fieldsets hide when the kind cannot use them. Procedure and shard
authorize themselves through the client — they are public endpoints.
An enum's options are a card of their own under the column form —
one row per option (label autosaving, a red icon-only *Remove*),
an explicit *Add option* form beneath — because a "blank row
removes, extra row adds" list reads as nothing at all. An enum with
no options is a draft state (G.7): the editor never seeds one, and
publication is where an empty choice is refused. One runtime
constraint shaped the code and is recorded for the next interactive
page: a runtime closure can reach **signals declared in its own
`view!` and the event only** — captured locals are bound inside the
expression's block and do not outlive it (topcoat 0.6.2) — so the
handlers, the `revision` signal, and the shard call share one
`view!`, and the ids and texts a handler needs travel as signals.
The editor's inputs and selects that carry handlers are plain
elements styled like the vendored components for the same reason.
**Many values is offered on choices, attachments and geometries
only** (settled 2026-08-23): the kernel lets any column be list-valued
(DESIGN §2.2), but in the whole DN corpus arity `many` occurs only as
multi-select, multi-file and feature sets
(`corpus/M0-type-frequency.md`) — so this is a platform rule: **in
the API it is structural** (`multiple` lives on `EnumType`,
`AttachmentType` and `GeometryType` only, G.7), `platform-core::
schema_edit` keeps it as the kernel-edge backstop (`list_capable`),
and the editor shows the values select for those kinds only; a type
change away from them takes the arity back to one. Out of scope,
deliberately: publishing, surface properties, blocks and published
nomenclatures, undo.

## P.5 GraphQL schema

**Moved to `design/graphql.md` (2026-08-22).** The schema design grew
past a section; it has its own document with its own numbering (G.x).
This heading stays so every `P.5` cross-reference keeps resolving —
read them as pointers to `design/graphql.md` G.1, which carries the
original content verbatim. P.6 (read models) stays here: it is about
what resolvers read, not what the schema says.

## P.6 Read models

Resolvers read materialized read models, never fold logs per request.
What is materialized and who maintains it is DESIGN Q18; dataloaders
from the first resolver onward — the connection-of-case-files shape
guarantees N+1 otherwise. The reviewer table = read model + compiled
varve-logic filter + DESIGN §5.5 aggregate typing for mixed-revision
listings.

## P.7 Authentication

Browser sessions in the app; API tokens for integrators, scoped to
procedures as in DN. Both resolve to the same `Principal` (party ids,
surface assignments, platform roles) before execution; the schema never
sees the transport. FranceConnect / AgentConnect are session concerns,
invisible below `platform-app`.

**Transports never substitute for each other (settled 2026-08-22).**
`/graphql` authenticates with `Authorization: Bearer <token>` only;
the session cookie is ignored there, and a bearer token never
authenticates a page. Rationale: a route no cookie can authenticate
has no CSRF surface, so the bearer guard is the whole of the API's
security and the router's `OriginPolicy` stays a page concern; the
app's own components execute documents in-process, not over HTTP, so
nothing legitimate needs cookie-authenticated `/graphql`. Tokens are
stored as `SHA-256(secret)` only (never encrypted — nothing on the
server can recover a secret), shown once at creation, and expire six
months after it; the secret's fixed `varve_` prefix exists for leak
scanning. Per-procedure scoping (the DN shape above) is P2 work on
top of the account-level token.

## P.8 Milestones

- **P0 — walking skeleton.** `varve-store` traits + Toasty impl
  (registries + record logs only), `varve-service` with two operations
  (impact-gated publish, surface-gated append), minimal schema
  (procedure, case file, cells, updateCells/submitCaseFile), Topcoat
  applicant form rendered from the surface tree, sessions. Proof:
  create → publish → fill → submit, every step through the schema.
  **Ordering settled (2026-08-20): outside-in.** The platform shell
  comes first — the Topcoat app with sessions/auth (P.7) and i18n (Q8
  spike first, since UI strings land here) over `platform-core`'s
  account models — then the GraphQL schema executing in-process, and
  only then the kernel edge: `varve-service` (DESIGN §13.2; built
  2026-08-24, publication first), the store-contract harness extracted from `varve-store`'s
  tests, and `platform-store` over toasty (Q10 spike). Rationale: the
  young half of the stack (topcoat, toasty, the MF2 runtime) is the
  risk to retire first; the kernel side is already deterministic and
  oracle-tested. The proof sequence is unchanged.
- **P1 — instruction.** Read models + the query compiler (DESIGN
  Q18/Q19 land here, spike first), reviewer table with varve-logic
  filters, the checkpoint state machine, teams + routing.
- **P2 — collaboration.** Messaging, notifications, webhooks, exports
  (wire / tabular artifacts, `varve-export`), `platform-client`'s
  HTTP transport + HTTP-path integration tests (the crate itself and
  its in-process transport land in P0 — P.9 Q2), API tokens.
- **P3 — resolvers.** `varve-resolve`, SIRET/BAN blocks, prefill
  (DESIGN §2.7), attachment scan lifecycle end-to-end.

Each milestone ends the way kernel milestones do: with the check that
everything shipped exists in DN and nothing shipped that doesn't.

## P.9 Open questions

1. **Toasty dynamic queries.** The spike gating DESIGN Q19: can a
   runtime-constructed nested and/or predicate tree with an `EXISTS`
   subquery be expressed in Toasty's query API? Run before P1's
   compiler work starts; fallback is parameterized SQL against the same
   tables.
   **Evidence (2026-08-20, source reading at `toasty-v0.10.0`,
   distilled into `.claude/skills/toasty`): mostly yes.** `Expr<bool>`
   is a plain composable value — `.and`/`.or`/`.not`,
   `Expr::and_all(iter)` (empty → true), runtime `in_list`, `between`
   — so arbitrary runtime predicate trees fold fine
   (`crates/toasty/src/stmt/expr.rs`). Correlated subqueries exist but
   **only along declared relation paths** (`Path::any()`/`.all()`,
   lowered to `IN (SELECT …)`); the AST carries `Exists`/`InSubquery`
   with no public constructor for arbitrary tables, and there is no
   streaming terminal. The spike narrows to: does the Q19 compiler
   ever need `EXISTS` off a declared relation path? If not, supported;
   if so, the raw-SQL fallback covers that clause alone. Ratify only
   against a compiled program.
2. ~~**Fragment ↔ component pairing.** No Relay-style compiler exists for
   Rust. Candidates: a macro colocating the fragment with the Topcoat
   component, plus a build-time check validating every fragment against
   the SDL. Decide at P0 while the app is small.~~ **Resolved
   2026-08-22: `cynic` derives are the macro and the check.** A
   fragment is a `#[derive(cynic::QueryFragment)]` struct declared
   beside the component that renders it; a page query composes
   fragments by nesting the structs; `cynic-codegen` validates every
   derive against `platform-client/schema.graphql` at build time. The
   SDL is checked in and a `platform-graphql` test asserts it equals
   `schema().sdl()` (`VARVE_UPDATE_SDL=1` rewrites it), so a resolver
   change fails that test and a stale fragment then fails to compile.
   Runner-up `graphql_client` generates from `.graphql` files into
   names it chooses and composes cross-file fragments poorly; no custom
   macro was needed. **In-process is the same JSON boundary as HTTP**:
   every client is transport-agnostic by construction — an operation
   is a `{query, variables}` document and a response a `{data, errors}`
   document — so the in-process transport is
   `serde_json → async_graphql::Request → execute → Response →
   serde_json`, deliberately crossing the exact serialization
   integrators cross (custom scalars, ids, timestamps are dogfooded
   byte-for-byte, P.1 rule 4). The client is a leaf crate;
   `platform-graphql` depends on it to provide that transport and
   mirrors `error::Code` in the client's own enum (a test keeps the
   two sets equal — five variants do not earn a shared crate).
3. **Case-file state authority.** Derived from checkpoints alone, or also
   mirrored as a platform column? Lean: checkpoints are authoritative
   and the column is a read model maintained only by the use-case
   services, never written independently. Confirm when the P1 state
   machine lands.
4. **Draft autosave granularity.** An entry per autosave is the
   kernel-pure answer but may bloat logs; DESIGN §12.8 (post-submission
   edit profile) will size it. The alternative — a platform-side draft
   buffer folded into one entry at submit — trades away provenance.
   Decide with §12.8 data in hand.
5. **Public API armor.** (`design/graphql.md` G.2 bounds query depth
   structurally — full objects only at root, lists of Refs — which
   narrows this to page-size caps and per-list cost.) Depth/complexity
   limits ship at P0; whether
   persisted queries and cost-based rate limiting are needed before the
   API opens to third parties.
6. **Webhook payload shape.** GraphQL-shaped JSON vs wire lines — the
   one place integrators may want the low-level truth (DESIGN §13.4) —
   plus delivery semantics (at-least-once, signed payloads).
7. ~~Blob key policy: shreddable vs recoverable (P.10).~~ **Resolved
   (2026-08-19): shreddable — the per-blob identity is the sole
   recipient; no master co-recipient on blobs.** Settled by design
   argument: erasure is the guarantee the design cannot compromise on
   (DESIGN §2.10), while the property traded away — recovering files
   from the bucket alone — only pays off under total database loss, a
   scenario already existential for the platform and owned by backup
   discipline, not by weakening erasure. Key rows are wrapped under
   the master key, so database backups stay safe to retain; a shred
   truly completes as those backups age past retention — a stated,
   bounded window, the same caveat every crypto-shredding scheme
   carries. Uniform across blob classes: attachments and resolver
   payload snapshots both shreddable (the snapshots are §2.10's worry
   case), which is the operational evidence DESIGN Q11 wanted. The dev
   local-fs impl encrypts identically (parity — keyring and shred
   paths exercised in tests). Residual, deliberately deployment-level:
   master-key custody (KMS / injected secret, per environment).
   Contract: DESIGN §13.6.
8. ~~MF2 runtime spike: `mf2-i18n` vs hand-rolled over ICU4X.~~
   **Resolved (2026-08-20): hand-rolled interpreter over
   `ox_mf2_parser` + ICU4X.** Settled by spike (scratchpad, ~570-line
   interpreter, 18/18 corpus tests): parse with `ox_mf2_parser` (MIT,
   spec-final CST — its `SemanticModel` lowering is lint-only and
   carries no option values; evaluate from the CST), delegate plural
   selection, number, and datetime formatting to ICU4X 2.3
   `compiled_data` (`icu_plurals` cardinal rules, `DecimalFormatter`
   over `Decimal`, `DateTimeFormatter` fieldsets). French output is
   CLDR-correct end to end: 1 → `one`, 1 000 000 → `many`, U+202F
   group separators, `"20 août 2026"`. `mf2-i18n` **ruled out** on
   source reading: despite the name it parses a homegrown
   Fluent-style syntax, not LDML MF2; formatting is non-CLDR (chrono
   + POSIX locale data, options silently ignored, French `many`
   absent); runtime unusable without its manifest/bytecode pipeline;
   two months old, bus factor 1, no CI. `mf2_parser` proved
   spec-current despite Oct-2024 dormancy (accepts final `.match
   $count`, rejects the draft form) but is GPL-3 and parser-only — it
   offers nothing the MIT candidate lacks, so no licensing question
   arises. Fluent fallback not needed. ICU4X still ships no MF2
   formatter (checked `icu_experimental` 0.6); when it does, swap it
   in and delete ours — unchanged. Implementation notes carried into
   `platform-i18n`: evaluate declarations once in order; lower the
   CST to a small IR per message, don't re-walk per format; pin
   `:number` exact-match semantics on the resolved value; NFC-
   normalize names before env lookup; unknown options warn-and-
   continue per spec, don't Err; decide f64→`Decimal` via the `ryu`
   feature or restrict argument types; treat formatted output as
   opaque (NNBSP will break naive snapshots). Residual risk:
   `ox_mf2_parser` is pre-1.0 and two weeks old — it stays behind
   `platform-i18n`'s own interface, MIT/vendorable, swappable.
9. **Underway beside toasty.** The spike gating P.13's queue half: does
   underway's sqlx pool coexist cleanly with toasty on the same
   Postgres, and can the use-case services thread toasty's transaction
   into underway's enqueue (the transactional-enqueue property is the
   whole point — losing it demotes the queue to outbox-plus-drain)?
   Run alongside P.9 Q1 (same genus: early-stage ORM meets raw-sqlx
   neighbor). Sweeps are unaffected — they are hand-rolled regardless
   (P.13). **Evidence (2026-08-20, same source reading): the hard
   half is absent.** Separate pools on one database work (the PG
   driver is tokio-postgres behind toasty's own deadpool pool), but
   0.10 offers no pool injection and no access to the underlying
   client (`Db::driver()` yields only `&dyn Driver`) — a transaction
   *shared* with sqlx, hence with underway, is not possible at this
   version. What survives: toasty's raw SQL executes inside toasty
   transactions and savepoints, so a hand-rolled SKIP LOCKED queue
   written through toasty raw SQL regains transactional enqueue
   natively. That reorders the fallbacks: hand-rolled-via-toasty-raw-SQL
   now leads; apalis-postgres (sqlx again — same wall) drops to last;
   underway itself survives only as outbox-plus-drain. The spike
   confirms against compiled code and checks whether newer toasty
   ships pool/connection sharing before P2's queue work.
10. ~~One transaction across the kernel/platform table boundary.~~
    **Resolved (2026-08-22): (a) — one toasty transaction, with
    `platform-store` a per-use-case value scoped inside it; the shape
    is settled under P.3 `platform-store`.** The
    `varve-store` traits are per-method atomic and expose no
    transaction handle (deliberately — the trait must not name a
    backend), yet the use-case composition in `platform-core` — kernel
    append + system message + outbox row — wants a single Postgres
    transaction spanning `platform-store`'s kernel tables and the
    platform models. Candidates: (a) hand the `platform-store` impl a
    toasty transaction at construction, so one txn threads through
    both crates while the traits stay ignorant of it; (b) accept
    per-method atomicity, commit the kernel append first, and let
    P.13's sweeps heal the platform side effects (at-least-once, same
    genus as the outbox drain). Lean (a) — (b) forfeits "exactly one
    place" atomicity that motivates the use-case services. Same family
    as Q9 (toasty txn into underway's enqueue); run with the Q1 spike
    and decide at P0 with the first use-case service. **Evidence
    (2026-08-20, same source reading): the API for (a) exists, but
    its shape constrains the design.** `db.transaction().await?`
    yields `Transaction<'a>: Executor` (Send+Sync) with
    commit/rollback, auto-rollback on drop, nested savepoints, and
    isolation config — threadable as `&mut dyn Executor`. But it
    borrows `&mut Db`: it cannot be stored `'static`, so "hand the
    impl a transaction at construction" cannot mean storing it in the
    `platform-store` struct — the executor must flow per operation
    (a scoped store constructed inside the txn, or store methods
    over an executor argument). Reconciling that with the
    executor-free `varve-store` trait signatures is now the crux of
    the spike. **How it was resolved:** design argument on the same
    source reading, before `platform-store` exists — the first
    use-case service this was to be decided with is not written, so
    the resolution is a shape, and the conformance suite against
    Postgres (P.3) is what will confirm it. The argument: both sides
    execute through toasty's `Executor`, and `Transaction<'a>`
    *is* one, so feasibility was never in doubt once the store is
    toasty; the two remaining wrinkles are shape, and each has one
    answer. (1) `varve-store` methods take `&self` while every toasty
    `exec` wants `&mut dyn Executor`: the impl bridges with an async
    mutex around the executor — serializing kernel-store calls within
    one use case costs nothing, a transaction is one connection and
    already serial. (2) `Transaction<'a>` cannot outlive the `&mut
    Db` borrow: the store is therefore never a handle in app context
    but a value the use case constructs inside its transaction. The
    traits stay executor-free and the kernel learns nothing of
    toasty (DESIGN §13 boundary untouched). (b) is rejected for the
    reason already given — it forfeits the "exactly one place"
    atomicity that motivates the use-case services. Platform-only
    multi-write use cases (the first: `create_organization_for`,
    organization row + creator membership) use the same transaction
    and need no store at all.
11. **Whose time zone renders an instant?** Every timestamp the UI
    shows must become a civil date/time in *some* zone; the platform
    models none, so `platform-app` renders UTC as a documented
    stopgap (every UI date funnels through one `utc_date_arg`
    helper — near a midnight boundary the shown date is off by one
    for the viewer). Candidates, combinable: a per-account zone
    preference beside the locale preference (P.7's principal
    resolution would carry it); a per-deployment default (a French
    administration wants Europe/Paris even for accounts without a
    preference); a browser hint as a last resort. i18n is not the
    question — `platform-i18n` formats whatever civil date it is
    handed; the question is where the zone lives in the domain model.
    Decide before reviewer-facing timestamps land (P1's instruction
    views show submission times — off-by-one dates there are
    user-visible wrongness, not polish).
12. **Accessibility audit and declaration.** P.1.5 is enforced
    automatically only up to the baseline. Open: (a) the contrast
    audit of the topcoat-ui theme tokens in both color schemes (the
    theme is vendored — a failing token is an upstream conversation
    or a recorded local override in our own stylesheet); (b) when to
    run the first manual audit (screen readers, 200% zoom, Ara's RGAA
    grid) — no later than the first reviewer-facing release, since
    the public accessibility declaration (déclaration
    d'accessibilité + schéma pluriannuel) that French administrations
    must publish is derived from it; (c) whether the declaration is
    a platform-rendered page (per deployment, with the deploying
    administration's contact) or integrator-supplied content;
    (d) the header's account menu is a `<details>` disclosure, so
    Escape does not close it and focus does not return to the
    trigger — the WAI-ARIA menu-button pattern expects both. Correct
    HTML, but a keyboard user's expectation; closing it needs a few
    lines of script (the component docs say as much). The e2e
    keyboard journey asserts what holds today and is the place the
    fix gets proven.
13. **Routable teams per procedure.** Teams are organization-level
    (P.4); today every team of the owning organization is a candidate
    for a procedure's routing rules. Expected follow-up: an explicit
    procedure⟷teams set listing the teams a procedure may route to,
    so a large administration's procedures do not each see every
    team. Deferred until procedures and routing exist (P1); the
    membership tables are shaped so adding it is one join model.
14. **Who may create an organization?** (opened 2026-08-22, when
    `/organizations` made it visible.) Today `createOrganization` is
    open to any signed-in account — the viewer becomes the first
    member — and the app's creation form exposes exactly that. DN
    gated the equivalent (a new administration) on an operator: an
    administration is a legal entity that answers for its procedures,
    and self-service creation invites impersonation of public bodies.
    Candidates, from loosest to tightest: (a) open, as now, with the
    organization unverified until an operator confirms it; (b) an
    allowlist of email domains (`*.gouv.fr`, institutional domains
    on file) that may self-serve, an operator for the rest; (c)
    invitation-only — operators create organizations, which then
    invite members. The kernel is indifferent (§2.9: authorization is
    surface assignment; this is platform-only authorization over a
    platform-only resource). Decide before any non-test account can
    reach the form; until then the open behavior is a P0 convenience,
    not a settled policy, and the schema's error vocabulary is ready
    for it (`FORBIDDEN`, G.2.7).
15. **Draft mutation granularity on the public API.** (opened
    2026-08-23 with procedure drafts, P.4.) `platform-core` exposes
    element operations (add / update / move / remove); `graphql.md`
    G.1 names a single `editRevision (draft)`. Granular mutations
    compose with optimistic concurrency and give an audit trail per
    edit; a whole-tree `editRevision(schema: SchemaInput)` is simpler
    for a client-side form builder that holds the tree. The
    server-rendered P0 editor posts form-by-form and fits the granular
    shape; settle when the GraphQL side lands, and whether `base`
    must be echoed by the client (a stale-fork check distinct from the
    row version). **Partly settled (2026-08-23, `graphql.md` G.7):**
    the public API is granular by construction (`addColumn`,
    `moveElement`, … — one element operation per mutation, `CONFLICT`
    on a lost race); the arity-into-type rework landed the same day
    (G.7.2). Still open: whether a whole-tree `editRevision` is ever
    worth adding beside them. **The `base` echo is settled
    (2026-08-24, P.4 *Publication*):** no client echo — the draft row
    carries `base` (set from `latest_revision` when a draft starts),
    and publication refuses a draft whose base is no longer the
    lineage head (`CONFLICT`).

## P.10 Blob storage: platform-side encryption at rest (settled 2026-08-19)

Attachments and resolver payload snapshots (one blob machinery, DESIGN
§2.15) are stored in object storage as **ciphertext only**, encrypted
by the platform — the DN pattern (ds_proxy, a Rust streaming
encryption proxy in front of object storage), absorbed into the
platform instead of deployed beside it. The absorption is nearly free
because the design already forces the platform into the byte path:
the store verifies claimed content hashes against actual bytes and the
scan lifecycle needs the bytes (DESIGN §2.15), so presigned
direct-to-provider URLs were never fully available. Consequence,
accepted: upload/download URLs are platform URLs, download
authorization is checked per request (surface assignment, not bearer
presigned links), and all file traffic transits the platform. The
byte-plane endpoints stay a distinct component behind a seam so they
can scale out independently of the app; an external proxy remains a
deployment option, not a separate codebase.

**Format: age** (via the maintained Rust implementation, `age`/rage).
The payload is ChaCha20-Poly1305 in the STREAM construction — 64 KiB
authenticated chunks, constant memory, seekable decryption, which is
what serves HTTP Range requests by mapping plaintext offsets to
chunk-aligned ciphertext ranges. Over a hand-rolled stream cipher
(the ds_proxy approach), age buys: a standard header with
**multi-recipient key wrapping** (later: §2.15 export bundles — the
JSONL stream and its blob sidecar alike — encrypted to a receiving
administration's key, same format; settled 2026-08-19 that the sidecar
itself holds plaintext entries and confidentiality is a bundle-level
option), and
standard tooling — the rage CLI decrypts anything the platform wrote,
which is the disaster-recovery story.

**Envelope: one ephemeral X25519 identity per blob**, stored in the
database encrypted under a master key, used as the blob's recipient.
Master-key rotation re-encrypts small database rows, never object
storage payloads; deleting the identity row **crypto-shreds** the blob
including provider-side backups — blob-level erasure for exactly the
data (third-party resolver payloads) that §2.10 worries about, and
operational evidence for Q11's deferred mechanism choice.
**Settled shreddable (P.9 Q7, 2026-08-19)**: the per-blob identity is
the **sole** recipient — the bucket alone is unreadable by design, and
recoverability is owned by database backup discipline (key rows are
wrapped, safe to back up; a shred completes as database backups age
out — a stated, bounded window). Shredding is the **sweep's deletion
primitive**: a blob is shredded only when its last reference is gone —
never while other records still share it, §2.10's retention bound —
key row first, object second. The dev local-fs impl shares the age
pipeline, so keyring and shred paths are exercised in dev and tests.

Interactions checked: blob addresses stay plaintext hashes (DESIGN
§2.15 — dedup happens at the address before bytes are stored, so
random per-blob file keys cost nothing); ciphertext substitution in
the bucket is caught by the existing verify-claims-against-bytes rule.
Threat model, honestly: this protects against the storage provider,
leaky buckets, and backup exposure — not against platform compromise,
where the keys live. Crate placement: the `varve-files` trait stays
plaintext-in/plaintext-out streaming; encryption is the S3
implementation's concern; key custody is Tier 5 platform
configuration (DESIGN §2.10: "key management at Tier 5").

## P.11 Attachment scanning (settled 2026-08-19)

Scanning happens behind a **`Scanner` trait** (streaming bytes in,
verdict out) — the same pluggability argument as resolvers (DESIGN open
question 8's lesson). First implementation: **ClamAV** as a clamd
sidecar (freshclam for signatures), streamed over `INSTREAM` via the
`clamav-client` crate's Tokio API — what DN runs today,
sovereignty-clean, operationally boring. In-process libclamav FFI was
considered and rejected: a large C library in-process and
signature-reload lifecycle, for latency the design doesn't need.
Later implementations behind the same trait, only on demonstrated
need: `yara-x` (Rust-native, in-process) for custom rules; an ICAP
client if procurement ever mandates a certified commercial engine
(ESET, WithSecure, MetaDefender — all speak ICAP; the protocol is
small enough to hand-write a client). **Ruled out**: cloud scanning
APIs (VirusTotal-style) — they ship citizens' documents to third
parties; disqualified on GDPR and sovereignty grounds.

Two consequences of P.10 (ciphertext-only storage). Nothing that
crawls the bucket can ever scan, so scanning happens **in the byte
gateway at ingest, on plaintext** — the gateway's single streaming
pass becomes a tee: *hash-verify ⊕ scan ⊕ encrypt*, one read doing
all three. And **rescans against new signatures** (why §2.15 made scan
status a lifecycle, not a boolean) are a `varve-service` sweep that
stream-decrypts and rescans — a real, bounded cost the sweep
scheduling must budget for; on the record each rescan is a fresh
`scan` request op followed by its verdict (DESIGN §2.15, aligned with
§2.8 on 2026-08-19 — see P.12). The verdict is asynchronous by design:
request pending, verdict lands as an op, let surface admissibility
refuse submission of un-scanned attachments (§2.15) — which is what
makes clamd latency, slow rescans, and scanner swaps all non-events for
the kernel.

Alongside, regardless of engine: **magic-byte type validation at
ingest** (claimed MIME vs actual bytes — `infer`/`file-format`-class
crates), near-zero cost and catches masquerading. Config note: clamd's
`MaxScanSize`/`StreamMaxLength` must be aligned with the platform's
max upload size or large files silently get partial scans. Stated
honestly: ClamAV is a known-signature compliance layer, not protection
against novel malware — reviewer safety leans at least as much on
serving attachments with `Content-Disposition: attachment` and
sandboxed, no-inline-HTML preview rendering.

## P.12 Resolution scheduling and abandonment (settled 2026-08-19)

The kernel records *that* a lookup was requested and *how it ended*
(DESIGN §2.8: lifecycle ops in the log), and hands the platform one pure
enumeration, `pending_resolutions(record)`. Everything between — attempt
timestamps, transient errors, backoff, next try, and the **deadline** —
is platform state, owned by the platform's resolution sweep
(`platform-jobs`, P.13) — which drives `varve-service` *steps*, per
DESIGN §2.8 steps-not-loops — never written into the record. DESIGN §2.8
settled the deadline as policy precisely so a multi-day upstream outage
(the normal case, per institutional memory) is handled by changing one
policy, not by rewriting records.

Obligations this places on the platform:

- **Termination.** The kernel cannot guarantee it (no clock); the
  platform must: every pending resolution is either landed, answered,
  or explicitly abandoned by policy — pending-forever is the leak DESIGN
  forbids. The abandonment policy runs per resolver, with a reason
  (`deadline` · `operator` · `unavailable` · `superseded`, the last
  when the applicant changed the input mid-lookup) written into the
  `abandon` op, and its summary (`attempts`, `last_error`) taken from
  the scheduler's own attempt history at that moment.
- **Outage posture is a policy choice, not a code path.** Whether a
  resolver's pending lookups should be abandoned after N days or simply
  wait out the outage is a per-resolver parameter. DESIGN §12.7
  (deferred-resolution frequency) sizes N and decides the default; until
  then the default leans to *waiting* — abandonment exists for
  never-resolving lookups and removed resolvers, not for weathering an
  outage.
- **Re-request in bulk** (DESIGN §2.8): an operator action, per
  procedure and resolver, that reopens `abandoned`/`failed` instances as
  a reported act — the "API is back" morning-after operation. Surfaces
  in the back office expose it next to bulk re-map.
- **Backoff is per resolver and shared**, not per record: when a
  référentiel is down, every pending lookup against it should back off
  together (one circuit, not ten thousand timers), and resume together.
- **Import** (DESIGN §2.8, §5): pending instances arriving by history
  import are picked up by the same scheduler through the same
  enumeration, with no import-specific path; instances the platform
  cannot serve stay pending until an operator decides (re-request once
  the resolver exists, or abandon with `unavailable`).
- **Scans follow the same rules** (DESIGN §2.15, aligned 2026-08-19):
  the scanner sweep is this scheduler's twin — transient clamd failures
  stay in its own attempt history, the verdict lands as one `scan` op
  with the summary, the P.11 rescan-against-new-signatures sweep is a
  bulk re-request, and a pending scan whose element was removed is
  ended with `superseded`. Blob-level dedup (scan one shared blob
  once, propagate the verdict to every element naming it — §13.6
  `BlobScan`) is the sweep's optimisation, invisible to the record.


## P.13 Background work: sweeps and queue jobs (settled 2026-08-19)

The platform's background work has two shapes, and forcing them into
one abstraction would break both. The boundary beneath them is DESIGN
§2.8 steps-not-loops: `varve-service` exposes scheduled duties as
callable steps plus policy types; everything below runs those steps.
The platform is integrator #1 bringing its own loop — any other
integrator drives the same steps from the job system they already run.

**Sweeps** (state-driven, enumeration-based): resolution retries, the
scan sweep, the blob sweep, outbox drain. A sweep is a periodic tick
(tokio interval) + a lease so exactly one process runs it (Postgres
advisory lock) + its own state tables (attempt history, per-resolver
circuit). P.12 already fixed the key property — backoff is per
resolver and *shared*, one circuit, not ten thousand timers — which is
why sweeps are **hand-rolled, no crate**: a job-queue framework's
per-job retry model actively fights the shared-circuit design. The
sweep reads kernel enumerations (`pending_resolutions`,
`referenced_blobs`) and calls `varve-service` steps.

**Queue jobs** (event-driven, per-item, retryable): webhook
deliveries, emails, export generation, batch record updates. Classic
durable jobs — enqueue at commit, per-job retry with backoff,
dead-letter. The decisive requirement falls out of P.3's use-case
services ("one `varve-service` operation + its platform side effects
in exactly one place"): **transactional enqueue** — the job row
commits in the same Postgres transaction as the toasty models and the
read-model, so handoff is exactly-once by construction and the
notification outbox collapses into the queue. This rules out external
brokers (Redis, Faktory) on architectural grounds: same database, or
the distributed-tx problem the outbox exists to avoid is reinvented.

**Crate: underway** (Postgres-native: `FOR UPDATE SKIP LOCKED`,
transactional enqueue as a first-class feature, retries, cron,
multi-step jobs; sqlx-based). Rejected: **apalis** — the most
established general framework and tower-aligned, but enqueue goes
through its own storage abstraction, so sharing the commit transaction
is not its native grain; its pluggable-backend generality costs
exactly the property needed most (kept as fallback — P.9 Q9);
**hand-rolled SKIP LOCKED queue** — honest and small, but
retry/cron/heartbeat/observability is undifferentiated plumbing a
maintained crate does better (second fallback); **external brokers** —
above. No backend trait of our own: the platform deploys as one
binary + Postgres, and a multi-backend job framework here would be
second-system syndrome. The swap seam is the crate boundary itself
(the P.2 hedge pattern), and the deeper pluggability — integrators
bringing their own scheduler — is already guaranteed one level down by
steps-not-loops.

Home: a `platform-jobs` crate (P.3) owning both shapes — sweep
runners, circuit state, job definitions — with `platform-server`
running its runners. The routed unknown is P.9 Q9.
