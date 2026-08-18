# PLATFORM.md — the platform above the kernel

Design document for the platform: the DN-successor web application and
HTTP API built on the varve kernel. `DESIGN.md` remains the single
source of truth for the kernel (tiers 0–5); this document owns
`platform/`. DESIGN §13 fixes the boundary between the two and is
authoritative where they overlap.

**Conventions** — the same as DESIGN.md: open questions are never
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

## P.2 Stack

- **topcoat** (tokio-rs, announced 2026-07): server-rendered reactive
  app; components paired with colocated GraphQL fragments (P.9 Q2).
- **toasty** (tokio-rs): platform models, and the first `varve-store`
  substrate (DESIGN Q19 — gated on the dynamic-query spike, P.9 Q1).
- **async-graphql**: schema, resolvers, in-process execution.
- **axum**: HTTP transport — `/graphql` with token auth, upload slots,
  export downloads.

All of topcoat/toasty are early-stage with breaking changes expected.
The hedges are structural: the `varve-store` trait (swap the ORM), the
schema (transport-independent by construction), and thin resolvers
(use-case logic lives in `platform-core`, not in the framework).

## P.3 Crates (`platform/`, `publish = false` permanently)

- `platform-core` — Toasty models for platform-owned data (accounts,
  procedure catalog, team membership, messages, API tokens, webhook
  subscriptions, notification outbox) and the **use-case services**:
  each use case composes one `varve-service` operation with its
  platform side effects — submit case file = kernel append + system
  message + notification + webhook fan-out — in exactly one place.
- `platform-graphql` — schema and resolvers over
  `Context { principal }`; knows nothing of sessions or tokens.
- `platform-app` — the Topcoat app: sessions, principal resolution,
  in-process document execution, components with colocated fragments.
- `platform-client` — typed client generated from the SDL, for Rust
  integrators and for integration tests that exercise the real HTTP
  path.
- `platform-server` — the binary: axum router + the app + the
  schedulers (`varve-service` resolution retries, blob sweep, outbox
  delivery). One process to start with; the seams are already crates.

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

**Messaging**: platform entities keyed by case-file id; system messages
are emitted by use-case services (state changes, resolver failures, …).
Deliberately not kernel data — the log is cells, not chat (DESIGN
§2.9).

## P.5 GraphQL schema

- **One static schema for all procedures.** Record values are generic:
  `cells: [Cell!]`, a union/interface over the value types, addressed
  by column id (enum options by identity, DESIGN §2.11; group rows as
  nested cell lists). GraphQL types the transport; the revision types
  the domain.
- **Filtering** = varve-logic AST as a structured input (DESIGN §13.3),
  kernel-typechecked against the querying party's surface-scoped
  (aggregate) revision; kernel type errors surface as structured
  GraphQL errors, not empty result sets.
- **Connections** for all lists; depth/complexity limits from day one
  (P.9 Q5).
- **Mutations are use cases**, not kernel primitives: createProcedure,
  editRevision (draft), publishRevision (returns the impact report /
  confirms), createCaseFile, updateCells (a batch of cell writes folded
  into one kernel patch — legal in `DRAFT` and `SUBMITTED`, hence not
  named after a state), submitCaseFile, startReview, acceptCaseFile,
  refuseCaseFile, closeWithoutDecision, returnToApplicant,
  reopenReview, sendMessage, requestExport (returns an artifact URL —
  wire/CSV, DESIGN §5), createUploadSlot.
- **Attachments bypass GraphQL**: a mutation mints an upload slot
  backed by `varve-files`, the client PUTs bytes, then references the
  blob id in a cell write; the DESIGN §2.15 scan lifecycle gates
  admissibility as usual. No bytes through the executor, ever.

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

## P.8 Milestones

- **P0 — walking skeleton.** `varve-store` traits + Toasty impl
  (registries + record logs only), `varve-service` with two operations
  (impact-gated publish, surface-gated append), minimal schema
  (procedure, case file, cells, updateCells/submitCaseFile), Topcoat
  applicant form rendered from the surface tree, sessions. Proof:
  create → publish → fill → submit, every step through the schema.
- **P1 — instruction.** Read models + the query compiler (DESIGN
  Q18/Q19 land here, spike first), reviewer table with varve-logic
  filters, the checkpoint state machine, teams + routing.
- **P2 — collaboration.** Messaging, notifications, webhooks, exports
  (wire/CSV artifacts), `platform-client` + HTTP-path integration
  tests, API tokens.
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
2. **Fragment ↔ component pairing.** No Relay-style compiler exists for
   Rust. Candidates: a macro colocating the fragment with the Topcoat
   component, plus a build-time check validating every fragment against
   the SDL. Decide at P0 while the app is small.
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
5. **Public API armor.** Depth/complexity limits ship at P0; whether
   persisted queries and cost-based rate limiting are needed before the
   API opens to third parties.
6. **Webhook payload shape.** GraphQL-shaped JSON vs wire lines — the
   one place integrators may want the low-level truth (DESIGN §13.4) —
   plus delivery semantics (at-least-once, signed payloads).
