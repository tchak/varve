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
- `platform-server` — the binary: the topcoat router (app + `/graphql`
  and upload/download `#[route]` handlers) + the
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
  wire, or tabular CSV/XLSX with surface-scoped columns, DESIGN §5),
  createUploadSlot.
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
  (wire / tabular artifacts, `varve-export`), `platform-client` + HTTP-path integration
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
is platform state, owned by the `varve-service` scheduler in
`platform-server` (P.3), never written into the record. DESIGN §2.8
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

