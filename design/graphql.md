# graphql.md — the public schema

Design document for the platform's public GraphQL schema: the only way
to read or write domain data, the app included (platform P.1.1). Split
out of `design/platform.md` P.5 on 2026-08-22; `design/platform.md`
owns the platform around it (crates, auth, read models, jobs),
`design/kernel.md` (DESIGN) the kernel below. Where this document and
DESIGN overlap, DESIGN §13 is authoritative.

**Conventions** — the same as the other two: sections are `G.x`, open
questions live in G.5 and are never deleted (struck through with a
**Resolved** note and a pointer); decisions record *how* they were
settled — corpus data, institutional memory, design argument. Unknowns
that touch the kernel route to DESIGN §10/§12, platform-only ones to
platform P.9, schema ones here. Features earn their place by existing
in DN, or in the prototypes that preceded varve (G.2 cites one).

## G.1 Schema shape (was platform P.5, verbatim)

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

## G.2 Type graph: full objects only at root (settled 2026-08-22)

**Source: institutional memory** — the 2024 JS prototype (`repository`
= procedure, `submission` = case file; its generated SDL is the
reference) ran with these rules and they held up; re-examined against
varve's invariants, they fit better here than there. Settled by design
argument on top of that memory.

1. **Full objects only at root.** `procedure(id)`, `caseFile(id)`,
   `organization(id)`, `team(id)`, `revision(id)` return full objects.
   Every list field — at root or inside an object — returns `*Ref`
   types. **A Ref holds scalars plus the Refs of its ancestors and
   nothing else**: `CaseFileRef → ProcedureRef → OrganizationRef`;
   never a child list. The type graph is therefore a DAG of fixed
   depth: the recursion P.9 Q5 worries about cannot be written, and
   "complexity limits" reduce to page-size caps and per-list cost.
   Why it matters *more* in varve than in the prototype: authorization
   is surface assignment (DESIGN §2.9). A root fetch resolves the
   viewer's surface once, and everything below — which cells exist,
   what is readable, what `required` means — derives from that one
   resolution. Arbitrary nesting would re-resolve surfaces at every
   hop; this rule makes "one surface resolution per root object" the
   transport's shape. It is also the dataloader shape platform P.6
   wants: one query per root object, one per list field.
2. **List rows are designed read-model types where a table needs more
   than a Ref.** The reviewer table (platform P.6: surface-scoped,
   mixed-revision, DESIGN §5.5 aggregate typing) is the case: a row is
   `CaseFileRow { ref, state, summary: [Cell] }` with `summary` the
   columns the viewer's surface and the procedure's table configuration
   select. Not a Ref, not the full object, and still no traversal from
   a row — rule 1 stays intact and the table is one query.
3. **Filters: typed `@oneOf` inputs per context, plus the logic AST
   for cells.** Per-scalar filters (`DateFilter`, `StringFilter`,
   `IdFilter`) combined with `AND`/`OR`/`NOT` under `@oneOf`; a
   distinct filter input per *context* (the filter on
   `procedure.caseFiles` has no `procedure:` member — the parent fixed
   it; the root `caseFiles` filter has `organization:`, `procedure:`,
   `team:`). Nonsense queries fail validation. Accepted cost: a range
   is `AND: [{gte}, {lt}]`. This is the *metadata* layer; G.1's
   varve-logic filter is the *cells* layer, carried as a `cells:`
   member of the same input, validated by GraphQL around it and
   typechecked by the kernel inside it.
4. **Bounded lists are arrays; unbounded lists are connections.**
   Refines G.1 "connections for all lists": teams of an organization,
   revisions of a procedure, members, invitations are bounded by
   design and return `[XRef!]!`; case files are the unbounded list and
   return a connection. Procedures (an organization's catalog, the
   viewer's administered set) are treated as bounded for now (G.6);
   a connection is a compatible later change if the corpus shows
   catalogs large enough to page.
5. **State is a union of state-specific objects** (`Draft
   { createdAt }`, `Submitted { submittedAt }`, and with P1's
   checkpoints `InReview { startedAt, team }`, `Accepted { decidedAt,
   reason }`, …) with a parallel `*StateValue` enum for filtering.
   Facts live where they are meaningful; no nullable `submittedAt`.
   Both the union and the enum generate from one Rust enum.
6. **`counts`** sub-objects (`organization.counts { procedures teams
   members }`) answer counts off read models without fetching lists.
7. **Mutations: one `input` each, verb-first names, the full object
   returned (a Ref on removal), errors structural.** Refines G.1's
   list; `assignCaseFileToTeam`, never `teamAssignCaseFile`. No
   `payload { errors }` types — G.1 already makes kernel type errors
   structured GraphQL errors, and the same holds for use-case errors.
8. **No `delete` on anything that has been published or submitted.**
   Hidden never deletes (DESIGN §2.4). `deleteProcedure` exists for a
   never-published draft only; a submitted case file leaves through
   an *erasure* mutation with the DESIGN §2.10 guarantees (salted
   hashing, chain intact), named as such. "Duplicate" likewise can
   only be *create a case file prefilled from another*, with prefill
   provenance (DESIGN §2.7) — records never branch (§5, §6).

**What the prototype had that varve reshapes rather than adopts.**
Its `Form` was one mutable thing with a state; varve's `Procedure`
exposes `revisions`, the live one, the impact report as
`publishRevision`'s result, and every `CaseFile` pins the revision it
was filled against — the layer the prototype never had, not a
conflict. Its form tree encoded nesting depth in types (`Page >
SectionH1 > H2 > H3`, repeaters one level shallower) to forbid
recursion — the same instinct as rule 1, and exactly DN's three header
levels; whether varve's surface tree is typed that way or flattened is
G.5 Q1. Its `join*(token)` mutations returned a `JoinAction`
(`show | signIn | signUp | verify`) telling the client what to do
next — two of four outcomes are not state changes, so this may be a
query plus an `acceptInvitation` mutation: G.5 Q2, P2 material.

## G.3 Transport (settled in platform P.2/P.7, recorded here)

`POST /graphql` is an ordinary topcoat `#[route]`; execution is
`schema.execute` in-process over a `Principal` handed in as request
data (`platform-graphql` reads no headers, cookies, or tokens). The
route authenticates with `Authorization: Bearer <api token>` only; the
session cookie never authenticates the API and a token never
authenticates a page (platform P.7). Attachments bypass the executor
(G.1).

The SDL is a checked-in artifact, `platform/platform-client/schema.graphql`
— the contract integrators build against and the input `cynic`
validates the client's operations against; a `platform-graphql` test
keeps it equal to `schema().sdl()` (platform P.9 Q2).

## G.4 Naming

`DateTime` (RFC 3339, UTC) not `DateTimeISO`; `Id` filters not `ID`
filters as type names; `counts` not `info`; `*Ref` for the list
shapes; English-first vocabulary per platform P.4 (`procedure`,
`caseFile`, `organization`, `team`, `revision`).

## G.5 Open questions

1. **Surface-tree representation.** Bounded types per depth as the
   prototype did (matches DN's three header levels, which the M0
   corpus can confirm), or a flat list with `depth`/`parentId`. Decide
   when `varve-surface`'s tree type is fixed; keep the discipline
   either way — no recursive output type. **Settled for the schema
   side (2026-08-23, G.7):** a flat list in document order with
   `parentId` — the kernel allows groups 24 deep, so per-depth types
   do not fit the schema; the surface side stays open but now has a
   precedent to match.
2. **Invitations: `join(token)` as a query or a mutation?** The
   prototype's mutation returned the next client action; two of its
   four outcomes change nothing. Candidate: `invitation(token)
   { action }` + `acceptInvitation`. P2.
3. **Table columns.** Who chooses `CaseFileRow.summary` — the
   procedure's table configuration, the viewer's saved view, or both
   — and how that interacts with surface scoping and mixed revisions
   (DESIGN §5.5). P1, with the reviewer table.

## G.6 The P0 slice (settled 2026-08-22)

The first types and mutations shipped under G.2, before the kernel
edge: `organization`, `organizations`, `team`, `procedure`,
`procedures`, `Member`, and `createOrganization`, `createTeam`,
`createProcedure`. Settled by design argument on top of platform P.4
(membership is the only right) and P.7 (the P0 principal is
account-level only), so the rules below are the membership stand-in
for "authorization is surface assignment" until surfaces arrive.

1. **Visibility is membership, resolved once at the root.** An
   organization (full object) is visible to its members only; a team
   to its organization's members and to its own members; a procedure
   to its organization's members. `organizations` and `procedures`
   list the viewer's. A reviewer who is not an organization member
   reaches the organization only as `team.organization`
   (`OrganizationRef`) — never its procedures or members: the full
   object is the administrator's view, and the Ref is exactly what a
   reviewer needs to name where they work.
2. **Absent and invisible are the same `null`.** Root lookups
   (`organization(id)`, `team(id)`, `procedure(id)`) answer `null`
   for an id that does not exist *or* that the viewer may not see, so
   an id never reveals whether it exists. The same holds for
   mutations: `createTeam`/`createProcedure` on a missing or foreign
   organization are one `FORBIDDEN`.
3. **`Member` is one type for both containers**: `{ account:
   AccountRef, joinedAt }`, the account⟷container link with the
   container as the parent (no back-pointer, G.2 rule 1).
   `AccountRef` carries `id`, `name`, `email`: co-members see each
   other's addresses, as DN shows reviewers to each other.
4. **Errors carry `extensions.code`** (G.2.7 made concrete):
   `INVALID_INPUT` (malformed id, empty name/title, and a bad slug —
   `[a-z0-9-]` after normalization, enforced by the `Slug` scalar's
   parse, so a resolver never sees an invalid one), `FORBIDDEN`,
   `SLUG_TAKEN`, and `INTERNAL` for a platform failure (store error,
   wiring bug): the cause is logged server-side and the message is a
   fixed `internal error` — a store's own error text names constraints,
   tables, and hosts, which are for operators, never for a client.
   Kernel type errors join the set with the kernel edge.
5. **`createOrganization` makes the caller the first member.** With
   membership the only right, an organization nobody belongs to is
   unreachable; the use case is the two writes in one transaction
   (`platform-core`'s `create_organization_for`, the first use of the
   transaction shape platform P.9 Q10 settled): no orphan
   organization can exist.
6. **Counts are live `COUNT(*)` for now**, not read models: nothing
   is materialized yet (P.6 / DESIGN Q18), and three bounded counts
   per root fetch is the cost of waiting for the real thing.

## G.7 The revision-draft slice (settled 2026-08-23)

The editor's API, over platform P.4's procedure drafts: one read and
seven mutations, shipped with `platform-core::schema_edit`. Settled by
design argument on top of G.2 and the kernel model (DESIGN §2.1, §2.6,
§3).

1. **`Procedure.revisionDraft: RevisionDraft`** (`null` = nothing in
   progress) carries `base` (the revision it forks from, `null` until
   the DAG lands) and `schema { elements: [SchemaElement!]! }` — the
   kernel tree **flattened in document order**, each `SchemaColumn` /
   `SchemaGroup` naming its `parentId` (`null` at the root). No
   recursive type (G.2), the tree rebuilds in one pass, and one query
   carries the whole draft. Named *revision* draft because "draft" alone
   is also a case-file state (G.2.5) and other published documents may
   grow drafts; the noun is what the draft publishes as.
2. **Column types are a union from the kernel's `ScalarType`** —
   `TextType | … | IntegerType { unit } | EnumType { options } |
   AttachmentType { accept maxBytes } | …` — G.2.5 applied to types:
   no nullable `unit` on a text column. Each member carries `kind:
   ColumnTypeKind` so a client that only wants the constructor need
   not match on `__typename`. On input the mirror is a **`@oneOf`
   `ColumnTypeInput`** — `{ text: true }`, `{ integer: { unit } }`,
   `{ enum: { options } }`, `{ attachment: { accept maxBytes } }` —
   so the validator, not a resolver, keeps a unit off a text column,
   and introspection shows which facts go with which constructor.
   Constructors without facts are `Boolean` markers (`false` is
   `INVALID_INPUT`); the cosmetic cost of that idiom is outweighed by
   what comes next: once the surface joins the editor, presentation
   options (placeholder, text length, …) attach per constructor, and
   a `kind` + nullable-fields input would degrade into a bag of
   fields while `@oneOf` members each grow their own. Same mechanism
   as the G.2.3 filters — one way to say "pick one variant".
   Enum columns are inline-backed only (DESIGN §2.12) until
   published nomenclatures
   have a platform home; option ids are kept when the client sends
   them and minted when omitted (identity, DESIGN §2.11).
3. **Mutations are element operations**, one `input` each, all
   answering with the full `Procedure` (G.2.7) so the editor reads
   `revisionDraft` off the response: `addColumn`, `addGroup`,
   `updateColumn`, `updateGroup` (omitted fields unchanged; ids never
   change — a type change must stay a type change for the impact
   report, DESIGN §3), `moveElement`, `removeElement` (a group with
   its subtree), `discardRevisionDraft`. **Placement is
   `{ parentId, beforeId }`** — a sibling anchor, never an index:
   `beforeId: null` appends, and the two cases reach both ends, while
   an index is only meaningful against the tree the client last saw.
   Ids are server-minted; the element `addColumn`/`addGroup` created
   is the one in front of `beforeId` in its parent, or that parent's
   last child. `required`, visibility and presentation are surface
   facts (DESIGN §2.6) and have no mutation here; the surface draft
   joins `RevisionDraft` later and publishes with it.
4. **Two codes join G.6.4:** `INVALID_EDIT` — the draft or the kernel
   refused the operation (unknown element or parent, anchor outside
   its parent, a schema `varve_schema::validate` rejects: duplicate
   id, `many` nested in `many`); the message carries the reason and
   the draft is unchanged — and `CONFLICT` — the row's optimistic
   concurrency check failed (two editors racing); re-read and retry.
   No client-side version token yet (platform P.9 Q15).
5. **Not here:** `publishRevision` (needs the revision DAG store and
   the impact report, DESIGN §3 — `base` is its hook) and blocks /
   published nomenclatures (registries not yet wired).

Platform P.9 Q15 (granular vs whole-tree mutations) is thereby
answered for P0 by construction — granular — and stays open only as
to whether a whole-tree `editRevision` is ever worth adding beside it.
