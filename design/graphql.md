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
  confirms), closeProcedure, reopenProcedure (publishRevision from
  `Closed` also reopens — platform P.4), createCaseFile, updateCells (a batch of cell writes folded
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
   **Amended 2026-08-24: state objects are prefixed with their
   subject** — `CaseFileDraftState`, `CaseFileSubmittedState`, … —
   because `Procedure` now carries the same pattern
   (`ProcedureDraftState | ProcedurePublishedState |
   ProcedureClosedState`, filtering enum `ProcedureStateValue`) and
   bare `Draft` would collide. `ProcedurePublishedState.since` /
   `ProcedureClosedState.since` are transition times — `reopen`
   resets `since` with no publication happening, so the field is
   not `publishedAt`; revision publication dates live on revisions
   (platform P.4, *Procedure lifecycle*).
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
   not match on `__typename`. **Many values ride the type** (settled
   2026-08-23, the rework platform P.9 Q15 anticipated): `EnumType`,
   `AttachmentType` and `GeometryType` carry `multiple: Boolean!`
   (the kernel's arity, DESIGN §2.2) and their inputs `multiple:
   Boolean! = false`; no other member has the fact and `SchemaColumn`
   has no `arity` — the platform rule that only those three kinds
   hold many values (P.4, corpus-backed) is structural here, not a
   validation, and `platform-core::schema_edit` keeps it as the
   kernel-edge backstop. On input the mirror is a **`@oneOf`
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
   published nomenclatures have a platform home; option ids are kept
   when the client sends them and minted when omitted (identity,
   DESIGN §2.11). **An enum with no options is accepted in the
   draft** (the editor builds the list one option at a time, and an
   empty choice is nothing a record can break on yet); refusing it is
   `publishRevision`'s job, with the rest of publication-time
   validation.
3. **Mutations are element operations**, one `input` each, all
   answering with the full `Procedure` (G.2.7) so the editor reads
   `revisionDraft` off the response: `addColumn`, `addGroup`,
   `updateColumn` (label / type — the type carrying `multiple`),
   `updateGroup` (omitted fields unchanged; ids never change — a type
   change must stay a type change for the impact report, DESIGN §3), `moveElement`, `removeElement` (a group with
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

**The authored-tree amendment (settled 2026-08-24, from platform
P.4's authored-tree and audience records).** The draft's tree stops
being only a schema, and the API says so:

- `RevisionDraft.elements: [DraftElement!]!` replaces
  `schema { elements }` — the wrapper named the wrong thing once
  presentation nodes joined. Union `DraftElement = DraftColumn |
  DraftGroup | DraftSection | DraftNote`, still flattened in document
  order with `parentId` (which may now name a group *or a section*;
  sections nest, mirroring the kernel). Section and note ids are the
  kernel's minted `NodeId`s (DESIGN §2.6, surface node identity).
- Every member carries `audience: Audience!` (`ALL | REVIEWER`; P.4:
  `reviewer` is DN's private, effective audience is the narrowest
  along the ancestor path, and explicitly authoring wider than the
  parent's effective audience is `INVALID_EDIT`). Add inputs take
  `audience: Audience! = ALL`; update inputs take it optional.
- Mutations grow `addSection` (title, optional help), `addNote`
  (optional title, body), `updateSection`, `updateNote` — same
  placement, same anchor semantics, presentation nodes in the
  sibling universe; `moveElement` / `removeElement` cover all four
  kinds. Surface *properties* (prompts, required/visibility rules,
  formats) are still not here — they attach per element in a later
  slice, as item 2 anticipates.

**Required on columns (settled 2026-08-24).** The first surface
property lands on the authored tree: `DraftColumn.required:
Boolean!` — the editor's v1 of DESIGN §2.6's requiredness *rule*,
covering its two constant cases ("always required" compiles to the
vacuous `And([])`, "not required" to no rule) until the rule editor
arrives. `addColumn` and `updateColumn` take `required: Boolean`
(optional): omitted on add it defaults by the column's **effective**
audience — public columns required, reviewer-only ones optional
(each element's admissibility is owned by its audience, P.4) —
omitted on update it is left alone. Only columns carry it: a
group's "required" (at least one item) is DESIGN Q22's count
bounds, not this.

**Format on text columns (settled 2026-08-24).** DESIGN §2.6's
format constraints land as the second surface property, and the
`@oneOf` idiom pays the dividend item 2 promised: the `TEXT`
constructor stops being a bare marker and grows its facts —
`{ text: {} }` on input (plain text),
`{ text: { format: { email: true } } }`,
`{ text: { format: { regex: { pattern: "…" } } } }` — while
`TextType` carries `format: TextFormat` (`null` = unconstrained), a
union `EmailFormat | PhoneFormat | IbanFormat | RegexFormat {
pattern }`, each member with `kind: TextFormatKind`. Format is
admissibility over text only (§2.6): the tree stores it beside the
type, never in it; format on any other constructor is
unrepresentable on input; a type change away from `TEXT` resets it
(the arity precedent); and a custom pattern is verified at edit
time on the kernel's linear-time engine
(`varve_surface::Format::verify`) — a refused pattern is
`INVALID_EDIT` now, not a stored mistake surfacing at publication.

**The element union is not draft-scoped (settled 2026-08-24).**
`DraftElement = DraftColumn | DraftGroup | DraftSection | DraftNote`
renames to **`Element = Column | Group | Section | Note`**;
`RevisionDraft.elements: [Element!]!`. The `Draft` prefix had followed
the container's name without an argument of its own — the operations
were already bare (`moveElement`, `removeElement`, item 3) — and the
shape is not draft-specific: the authored tree exists at every point
of a revision's life (publication derives schema and surfaces from
it, the audience marker survives on the published version, and the
next draft forks from its base's tree — platform P.4), so published
and historical revisions expose the same union (G.8). If a draft-only
fact ever appears, it belongs on `RevisionDraft` itself, never on
forked element types. Naming checks, recorded: bare `Group` is safe
precisely because teams were named `Team` to keep "group"
kernel-reserved (P.4 vocabulary — the union member is the rightful
holder of the name); and GraphQL `Element` is the *tree* element —
four kinds — not the kernel's `varve_schema::Element` (columns and
groups only), a distinction the API never has to draw because no
schema-element type is exposed.

## G.8 One tree, viewer-scoped (settled 2026-08-24)

How revisions expose their structure, settled ahead of the published
read side by design argument on G.2 and platform P.4 (the authored
tree and the compiled pair), so the G.7 union could be named for
reuse rather than renamed at reuse.

1. **The compiled surface pair is never an API object.** The API
   exposes one thing: a viewer-scoped element tree per revision —
   `Revision.elements: [Element!]!`, the G.7 union, flattened with
   `parentId` like the draft's. Which compiled surface backs it is
   resolved from the viewer at the root (G.2 rule 1, literally): an
   applicant reading `CaseFile.revision.elements` gets the tree
   backed by the applicant surface; a reviewer, the full tree; an
   administrator browsing a procedure's revisions likewise. Nobody
   queries "the applicant surface" as a noun — surfaces stay the
   kernel's enforcement machinery (DESIGN §2.9), below the API's
   waterline. Genuinely independent authored artifacts (export
   layouts, print templates — P.4) would be their own objects; the
   applicant/reviewer pair never is.
2. **The audience invariant.** A viewer's tree never contains an
   element whose effective audience excludes that viewer — pruning
   guarantees it by construction: an element surviving on the
   applicant view has no `REVIEWER` marker anywhere on its ancestor
   path, so its authored and effective audience are both `ALL`.
   `audience` on the applicant view is therefore uniformly `ALL` —
   degenerate but honest, and it stays non-null: the field is
   informative exactly for viewers who see elements narrower than
   another audience's view (the reviewer badge, P.4), and it
   degrades gracefully when the enum grows (DN's experts, the third
   audience P.4 already anticipates — they would receive the
   `ALL`-only tree today and their own slice later, same invariant,
   no shape change).
3. **`Revision.elements` is static per (revision, viewer).**
   State-dependent facts — writability after checkpoints,
   instruction freezing the applicant's writable set (DESIGN §2.9) —
   never land on elements: one revision is read through many case
   files in different states, so writability is a case-file-level
   read beside the cells, not an element fact.
4. **Coherence with entry visibility comes free.** The applicant's
   redacted log (DESIGN §2.9) filters by the applicant surface's
   static column set — exactly the column set of the pruned tree the
   same viewer receives from `elements`. The two reads cannot
   disagree, by construction rather than by discipline.

## G.9 The lifecycle slice (settled 2026-08-24)

The API surface of platform P.4's *Procedure lifecycle* and *Event
logs*, shipped before publication itself (the kernel edge).

1. **The state rides at two altitudes, per G.2 rule 5.** The full
   `Procedure` carries `state: ProcedureState!`, the union of
   subject-prefixed members (`ProcedureDraftState { createdAt }` —
   the row's creation is the draft state's one fact —
   `ProcedurePublishedState { since }`, `ProcedureClosedState
   { since }`); `ProcedureRef` carries `state:
   ProcedureStateValue!`, the bare parallel enum — a list row shows
   a badge without breaching G.2 rule 1 (an enum is a scalar leaf;
   the facts stay on the full object). Both generate from the one
   platform-core discriminant. `since` is deliberately not
   `publishedAt` (G.2 rule 5's amendment).
2. **`closeProcedure` / `reopenProcedure`**, G.2.7-shaped (one
   input, full object back). A transition the machine refuses —
   closing a draft, reopening an open procedure — is the new
   structured code **`INVALID_TRANSITION`**: distinct from
   `CONFLICT`, which stays the optimistic-concurrency answer
   (re-read and retry) and now covers lifecycle races as well as
   draft races. `publishRevision` joins with the kernel edge and
   lands in `Published` from any state (P.4).
3. **`Procedure.events: [ProcedureEvent!]!`** — the audit trail,
   oldest first, an array not a connection: the log holds lifecycle
   transitions only, never authoring workflow (P.4, amended
   2026-08-24: `draft_discarded` dropped with the rest), so it is
   bounded by design (G.2 rule 4). A row is `{ id, kind:
   ProcedureEventKind!, actor: AccountRef, createdAt }`; `actor` is
   `null` for a system event or an account since deleted — the
   entry outlives both. `PUBLISHED` is in the kind enum from day
   one (the alphabet is settled) with no writer until publication
   lands. Full-object only: the trail is the administrator's
   detail view, not list-row material.

## G.10 `publishRevision` (settled 2026-08-24)

The G.1 mutation made concrete, on platform P.4 *Publication*.

1. **One mutation, two phases through one shape.**
   `publishRevision(input: { procedureId, confirm: Boolean! =
   false })` returns `PublishRevisionResult { report:
   ImpactReport!, published: Boolean!, procedure: Procedure! }`. A
   free report (`worst = SAFE`) publishes immediately; a lossy,
   checked, or breaking one without `confirm: true` returns the
   report with `published: false` and an untouched procedure — the
   client re-sends with `confirm: true`, carrying the report to the
   administrator (P.4: confirmation carries the report). First
   publication classifies against the empty schema — every column
   `ADDED`, free — so the walking-skeleton path never sees a
   confirmation.
2. **`ImpactReport` starts minimal and honest**: `{ worst:
   ChangeClass!, columns: [ColumnImpactEntry!]! }` with
   `ChangeClass = SAFE | LOSSY | CHECKED | BREAKING`,
   `ColumnImpactEntry { columnId, class: ChangeClass!, change:
   ColumnChangeKind!, removedOptions: [ID!]! }`, and
   `ColumnChangeKind = ADDED | REMOVED | CAST | SCOPE_MOVED |
   FORBIDDEN` — `IDENTICAL` entries are filtered out (the report
   says what changed). The kernel report's unit and constraint
   detail, blocks, broken rules, and record assessments join the
   type as the platform grows them; the shape leaves room.
3. **Errors.** A procedure with no draft, or a draft an enum of
   which has no options (G.7: publication is where an empty choice
   is refused; the kernel deliberately accepts it as a draft
   state), is the new structured code **`INVALID_DRAFT`** — fix the
   draft and retry. A draft whose `base` is no longer the lineage
   head (another administrator published since it forked) is
   `CONFLICT` — discard or rebase the draft. Kernel validation
   failures at publish are `INTERNAL`: the editor validates every
   edit, so a draft that stops validating is a wiring bug, not a
   user error.
4. **Publishing transitions the lifecycle** (P.4): `Published` from
   any state, from `Closed` it is the reopen; the `published` event
   carries `{ revision, base }` facts and the trail's `PUBLISHED`
   kind gains its writer.
5. **`RevisionDraft.report` (amended 2026-08-24).** The same
   classification `publishRevision` gates on, computed at read time
   against the draft's base (the empty schema when none): impact is
   visible while editing — the editor shows what a publication would
   do without one being attempted. `publishRevision` stays the only
   writer; the read is the store's point lookup of the base schema
   plus the pure classifier.
