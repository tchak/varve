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
(G.1). Anonymous execution exists in-process only (G.16): the
bearer guard means every HTTP request executes as an account.

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
4. **Persisting the confirmed report.** Today the history diff
   (G.11) recomputes exactly what `publishRevision` gated on,
   because `ImpactReport` is a pure function of the two schemas.
   The day record assessments join the report (G.10.2's reserved
   room), a publish-time report over live records becomes a
   point-in-time fact recomputation cannot reproduce — showing
   "what the administrator confirmed" would then mean persisting
   the report on the `published` event's facts at publish time.
   Not needed until that day; the historical read stays the
   schema-only classification either way.
5. **Attachment values in the preview.** The fillable preview
   (G.12) keeps attachment controls inert: filling one would mean
   minting upload slots against a *draft* — scratch blobs entering
   `varve-files` with the §2.15 scan lifecycle, needing GC when the
   draft is discarded. Whether that is ever worth building, or
   attachments stay permanently preview-inert, decide when real
   case-file uploads exist and the slot machinery has a shape to
   share. P2.
6. **The pinned reading lens in the API.** DESIGN §2.9 settled the
   default lens as `pinned` (at submission) per schema, and G.15's
   checkpoint records the pinned revision — but the API reads
   through the head publication (G.14), a deliberate P0
   simplification: surfaces hang off *publications* while the
   checkpoint pins a *revision*, and two publications may share a
   revision (§2.13 decision 9), so "the surfaces the applicant
   submitted under" needs the publication resolved from the log's
   position, not the revision alone. Decide with the reviewer
   table's mixed-revision reads (P1, DESIGN §5.5).
7. **The portal slice.** G.16 gives the anonymous visitor the
   catalog and nothing past the card. What comes next: a public
   procedure detail page (which of `procedure(id)`'s fields are
   public, or a separate `Published`-only lookup), the start flow
   (catalog → sign-in gate → P.7 return-to → `createCaseFile`),
   and catalog search — at DN scale (M0's 42,723 published
   procedures) paging alone is a wall, and full-text search is a
   platform concern, not a varve-logic filter. Decide with the
   applicant portal, which is the catalog's whole reason to exist.

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

**The virtual draft amendment (settled 2026-08-25).**
`Procedure.revisionDraft` becomes **non-nullable** and answers the
tree the next edit would operate on — *head until touched*: the
stored working buffer when one exists; otherwise the published
head's authored tree with `base` naming the head; otherwise (never
published, nothing in progress) the empty tree with `base: null`.
Nothing is stored for the two virtual cases — this is a read-time
projection, the same move as `report` (G.10.5) and derived
reachability (DESIGN §2.4): platform-core already forks lazily on
the first edit (any edit, an update of a head element included —
P.4 *Publication*), and the read model now shows the fork-on-write
semantics instead of hiding the head behind `null` (which left the
first integrator's editor blank after every publication, with head
elements unreachable through element-addressed routes). The halfway
shape — non-null only when a head exists — was rejected: it keeps a
`null` case whose meaning differs from the old one, worse than
either endpoint. A new field **`RevisionDraft.inProgress: Boolean!`**
carries the one fact the projection would otherwise erase: whether a
stored working buffer exists. It cannot be inferred from an empty
`report` — a draft whose edits never touch the derived schema (a
label rename, a note, a section) is a real stored draft with an
empty impact report — and clients need it (draft badge, discard
offer, publish gating, "saved on" lines). **Writers keep talking
about the stored buffer**: `publishRevision` on a pristine draft
stays `INVALID_DRAFT` ("nothing to publish" is true, and an
identical republication would mint the same content-hash revision
id — pure churn); `discardRevisionDraft` on pristine is a no-op.

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
   draft races. **Amended 2026-08-27 (G.13):** the code widens to
   any operation the subject's lifecycle state refuses —
   `createCaseFile` on a `Closed` procedure answers it too. `publishRevision` joins with the kernel edge and
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
   lands (it gained one with G.10.4). Full-object only: the trail
   is the administrator's detail view, not list-row material.
   **Amended 2026-08-25 (G.11):** `ProcedureEvent` becomes an
   interface so the `published` event can carry its facts; the
   row's shared shape is unchanged.

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
   **Amended 2026-08-25 (G.11.4):** `ColumnImpactEntry` carries
   `label: String!`, resolved server-side from the two schemas.
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

## G.11 History and the publication diff (settled 2026-08-25)

The read side of the audit trail: the events list *is* the history,
and each publication answers the diff it made. Settled by design
argument on G.9, G.10 and the kernel store shape.

1. **The events list is the history; no new list.**
   `Procedure.events` (G.9.3) already carries the timeline —
   lifecycle transitions with actors and dates, bounded by design —
   and the `published` rows carry `{ publication, base }` facts
   (G.10.4; amended 2026-08-26 — publication content addresses,
   DESIGN §2.13 decision 9). History needed no new query, only those
   facts made API-visible. Newest-first is presentation; the log stays oldest
   first.
2. **`ProcedureEvent` becomes an interface** — the schema's first —
   with the G.9.3 row as its shared shape (`id`, `kind`, `actor`,
   `createdAt`) and one member per kind:
   `ProcedureCreatedEvent | ProcedurePublishedEvent |
   ProcedureClosedEvent | ProcedureReopenedEvent`. Only
   `ProcedurePublishedEvent` adds fields: `publication: ID!` and
   `base: ID` (`null` = first publication — the honest optional the
   platform facts already store). **Amended 2026-08-26 (was
   `revision: ID!`):** the facts are publication content addresses
   (DESIGN §2.13 decision 9) — a revision id cannot tell two
   surface-only publications apart. This is G.2 rule 5 applied to
   events — facts live where they are meaningful, so no nullable
   `revision` on a `CLOSED` row — but as an interface, not a union:
   the state unions share nothing, while every event shares the
   trail's shape, and union members would each re-declare it.
   `kind` stays on the interface, the G.7.2 precedent (clients that
   want the constructor without matching on `__typename`).
3. **The diff hangs on the event, not on a `Revision` object.**
   `base` is a *publication* fact, not a revision fact: a revert
   republishes the same content-address with a different base
   (DESIGN §2.1), and content-addressed objects converge across
   lineages — so "revision → its base" is not a function, and the
   event is the publication edge a diff describes. G.2 rule 1's
   root `revision(id)` remains reserved for the G.8 read side
   (browsing a historical tree), which this slice deliberately does
   not build.
4. **The diff *is* the report, recomputed.**
   `ProcedurePublishedEvent.report: ImpactReport!` — the exact
   classification `publishRevision` gated on (G.10.1), computed at
   read time from the two publications (resolved through the
   lineage's event log — each arrives with its schema, and its
   surface map names the stored surfaces; a `null` base classifies
   against the empty schema and the empty surface set, so a first
   publication renders as the initial column and surface list with
   no special case) through the pure classifier and the §3.1 surface
   diff (amended 2026-08-26) — the G.10.5 move again, resolved only
   when the field is selected. Recomputation is *exact* because
   today's report pair is a pure function of the two publications'
   content; the day record assessments join it, the historical read
   keeps the static classification and persisting the confirmed
   report becomes G.5 Q4. With the labels now on the entries (below), the report needs
   no companion tree fetch to render.
5. **`ColumnImpactEntry.label: String!`** (amends G.10.2): resolved
   server-side from the next schema, falling back to the base
   schema for `REMOVED` columns — kernel schemas carry labels, so
   the store already holds them for every published revision. This
   serves every consumer of the type: the publish confirmation had
   to aggregate removals into a count because a removed column has
   no label in the *draft* tree the client holds; the base schema
   names it.
6. **`Procedure.event(id: ID!): ProcedureEvent`** — the point
   lookup the diff page reads, beside the list. Without it, reading
   one publication's report means selecting `report` across the
   whole list and computing every publication's classification to
   show one. `null` for an unknown id, the G.6.2 absent/invisible
   rule; no breach of G.2 rule 1 (a scoped point lookup inside the
   full object, not a traversal).
7. **The report carries both §3.1 halves (settled 2026-08-26).**
   `ImpactReport` grows the surface section and the rename entries,
   everywhere the type appears — the mutation result, the draft's
   live report, the event diff: `worst` becomes the composed
   verdict (the worst class of the schema and surface halves — the
   exact class the gate used, so a client never re-derives it);
   `columns` gains `RELABELED` with `renamedFrom: String` (the base
   label; `label` is already the new one) instead of a rename
   vanishing as unchanged; `relabeledGroups: [GroupRelabelEntry!]!`
   names group renames; and `surfaces: [SurfaceChangeEntry!]!`
   carries the §3.1 diff — `surface` (`applicant` | `reviewer`),
   authoritative `class`, a `SurfaceChangeKind` of 26 members, and
   server-resolved naming (`label` from the schemas, section titles
   from the kernel diff, `from`/`to` for retitles) — G.11.5's rule
   extended: the report needs no companion fetch to render. The
   platform renders the pair deduplicated (one line when a change
   holds on both compiled surfaces, a qualifier when not) with a
   lapse sentence on `CHECKED` entries; that presentation is the
   app's, not the API's.

## G.12 The fillable preview (settled 2026-08-26)

The schema editor's preview tab stops being inert: administrators
fill the draft form with scratch values and watch admissibility
behave before publishing — the "later slice" the read-only preview
promised. One read shape and one mutation, settled by design argument
on G.7, the virtual-draft amendment, and the kernel value model
(DESIGN §2.4–§2.6). This slice is deliberately the **pilot of the
case-file models**: the write input `updateCells` will take and the
`cells` read shape G.1 promises are fixed here first, against scratch
values, where a wrong call costs a redesign and not a migration.

1. **`RevisionDraft.preview: Preview!`** — non-null like its parent
   (an empty bag until filled, the virtual-draft rule applied again).
   `Preview { cells: [Cell!]!, items: [ItemList!]!, findings:
   [AdmissibilityFinding!]! }` — `items` (each `many` group's ordered
   item list, amended in implementation 2026-08-26) is there because
   a freshly added item has no cells yet, and its server-minted id is
   exactly what the next write must address: cells alone cannot carry
   it.
   `Cell` pilots G.1's record read model: a union with one member per
   value kind plus the written-blank state (§2.4: `Empty` is a value
   state, not absence), each carrying `columnId` and the row path
   (the `{group, item}` segment chain of DESIGN §2.4), lists on the
   three `multiple` kinds, each member with `kind` (the G.7.2
   precedent). `findings` is the point of the slice: the
   server-evaluated admissibility of the values against the draft —
   `compile_surfaces` over the draft tree, `varve_surface::
   admissibility` per compiled surface, findings unioned with their
   surface named (`applicant` | `reviewer`) and deduplicated in
   presentation, the G.11.7 move. A union `MissingRequiredFinding |
   FormatViolationFinding`, each naming its column and path;
   eligibility is not evaluated (no lifecycle in a preview) and the
   pending set is empty.
2. **`updatePreview(input)`** — the `updateCells` pilot:
   `{ procedureId, writes: [CellWriteInput!]! }`, applied in order,
   all-or-nothing, answering the full `Procedure` (G.2.7). **Not
   named `updateDraft`**: bare "draft" is reserved for the case-file
   state (the G.7.1 argument that named `revisionDraft`), and the
   seven element mutations are already what "updating the draft"
   means. `@oneOf CellWriteInput` mirrors the kernel's five-op patch
   set (`varve_value::patch::Op`, the one representation the record
   log, export and migration already share): `set { columnId, path,
   state }`, `unset { columnId, path }` (back to absent, distinct
   from set-empty), `addItem { groupId, parent, beforeItemId }`,
   `removeItem`, `reorder { order }` — item placement is a sibling
   anchor, never an index, and item ids are server-minted, both by
   the G.7.3 argument. `state` is a `@oneOf CellStateInput`:
   `{ empty: true }` or one value constructor per scalar kind, the
   `ColumnTypeInput` idiom. Type errors refuse the whole batch —
   unknown column, kind mismatch, a path into a non-`many` group, a
   bad anchor — as **`INVALID_WRITE`**, a new G.6.4 code minted here
   precisely so `updateCells` inherits it; `CONFLICT` applies as
   everywhere. **Admissibility never refuses**: required and format
   findings are the read model's output, not a gate — watching them
   is what the preview is for.
3. **Values are scratch, not a record.** Stored as a plain
   `varve_value::RecordValues` (cells + item lists) beside the draft
   on the procedure row — never a `varve-record` log: no history, no
   actor, no hash chain, and §2.10's salted-encoding obligations
   exist for case files, not for a bag an administrator discards.
   Two consequences are the decision's substance: **filling the
   preview does not fork the virtual draft** — `inProgress` stays a
   statement about the authored tree, previewing the pristine head
   creates no working buffer, and publish gating is untouched — and
   the preview is **scoped to a draft cycle**: cleared by
   `discardRevisionDraft` and by publication. Preview writes share
   the procedure row's optimistic guard, so a fill racing a tree
   autosave is `CONFLICT`, re-read and retry (G.7.4's answer);
   accepted for P0, and the separate column leaves room to relax it.
4. **Stale cells are inert.** The tree changes under stored values
   (a type change, a removal): reads fold only cells whose column
   still exists with a matching kind; orphans are dropped lazily on
   the next write, never eagerly — `tree_edit` stays ignorant of
   preview state, and running `varve-projection` over scratch buys
   nothing.
5. **Not here:** attachment and geometry *values* — those controls
   stay inert (no upload slots against a draft, no map), and a
   required attachment shows honestly as a standing
   `MissingRequired` finding (G.5 Q5 holds the residual); and
   per-audience preview (filling the form as the applicant or the
   reviewer sees it) — a later slice over the same value bag; the
   fillable preview is the full-tree administrator's view with the
   audience badges.

## G.13 The case-file catalog slice (settled 2026-08-27)

The first `CaseFile` appears in the schema — the **platform catalog
half only**. The kernel record log (cells, checkpoints, `updateCells`,
`submitCaseFile`) waits for `varve-store`'s record-log persistence;
this slice is the row an applicant creates, who is on it, and the two
listings G.2 promised. Settled by design argument on G.2's recorded
rules plus the P.4 membership pattern (participants: platform P.4,
*Case-file catalog and participants*).

1. **`createCaseFile(input: { procedureId })` → the full `CaseFile`.**
   Any authenticated account may create on a `Published` procedure —
   applicants need no prior relation to it, and nothing bounds how
   many case files one account opens on one procedure (DN allows any
   number). One transaction: the row, the creator's participant row,
   and the `case_file_events` `created` entry (the P.4 event-log
   shape, landing now with a one-word alphabet). Refusals: a `Closed`
   procedure is `INVALID_TRANSITION` — the G.6.4 code's doc widens
   from "a lifecycle transition the machine refuses" to "an operation
   the subject's lifecycle state refuses", no tenth code — while a
   `Draft` procedure is `FORBIDDEN` for everyone: a never-published
   procedure does not exist for non-members (G.6's same-answer
   discipline), and the one party who *can* see it, its
   administrators, has the G.12 preview — a case file on an
   unpublished schema is a contradiction, not a permission an
   organization member is missing.
2. **Root `caseFiles` is viewer-scoped — and the schema's first
   connection** (G.2 rule 4: case files are the unbounded list).
   G.2 rule 3 already gave its filter `organization:`, `procedure:`,
   `team:` members, i.e. the recorded design reads "the case files
   the viewer can see" — which today collapses to exactly
   *participant-of*. Recorded as viewer-scoped so P1's team
   assignment extends the scope instead of forking a second query.
   Newest first (id descending — UUID v7 is creation order);
   forward-only (`first` defaulting to 25 and capped at 100, `after`
   an opaque cursor). `last`/`before` and the rule 3 filters land
   with the reviewer table (P1), which stays its own designed read
   model (`CaseFileRow`, rule 2) either way.
3. **`caseFile(id)`: participants and the owning organization's
   members; `null` otherwise.** Administering a procedure means
   seeing its case files — at the metadata level, which is all this
   slice has; cells arrive later gated by surfaces (DESIGN §2.9).
   **`Procedure.caseFiles`** (same connection shape, all of the
   procedure's files) inherits the full object's member gating and
   needs no scoping of its own today; when applicant-facing
   procedure reads arrive (the portal), viewer-scoping extends it
   rather than forking it.
4. **Shapes.** `CaseFile { id, state, createdAt, updatedAt,
   procedure: ProcedureRef!, participants: [CaseFileParticipant!]! }`
   — participants bounded → array (rule 4), each `{ account:
   AccountRef!, joinedAt }`. `state` is the rule 5 union with one
   member today, `CaseFileDraftState { createdAt }`, beside the
   `CaseFileStateValue` filtering enum (`DRAFT`); both grow with the
   checkpoint machine (P1), and until checkpoints exist the platform
   state column is the only authority (then P.9 Q3's read-model rule
   takes over). `CaseFileRef { id, state: CaseFileStateValue!,
   createdAt, procedure: ProcedureRef! }` — scalars plus ancestor
   Refs (rule 1); both connections' nodes are Refs. `deleteCaseFile`
   (legal for a never-submitted draft, rule 8) is deferred until the
   applicant home needs it.

## G.14 Cells on the case file (settled 2026-08-28)

The G.12 pilot pays out: the case file grows the record read model
G.1 promised and the write mutation it reserved, both in the shapes
the preview fixed — and everything new is underneath: the batch lands
as **one entry in the kernel record log** through the `varve-service`
append operation (platform P.4 *Case-file record log*).

1. **Read shapes are the G.12 types, verbatim, unwrapped.**
   `CaseFile.cells: [Cell!]!`, `items: [ItemList!]!`, `findings:
   [AdmissibilityFinding!]!` directly on the full object — `Preview`
   wrapped them only because they hung off `RevisionDraft`; the
   record read model is the case file's own. Values are the fold of
   the record log read through the **head publication**: schema by
   revision id, surfaces by content hash from the publication's
   surface map. Stale cells are inert at read (the G.12 rule; for a
   record the pruning is read-time only — the log is append-only and
   keeps history). `cells` renders through the **applicant surface's
   column set** for every viewer at P0 — the only writing side today,
   and the guard that reviewer-only cells (annotations privées) never
   leak to applicants is in place from day one, vacuous or not;
   per-audience reads arrive with reviewers (P1). `findings` stays
   the surface-tagged pair (admissibility of both compiled surfaces
   is what "can this submit" needs, and findings carry no cell
   values), pending set from the fold (empty until resolvers),
   eligibility unevaluated as in G.12.
2. **`updateCells(input: { caseFileId, writes: [CellWriteInput!]! })`
   → the full `CaseFile`.** The exact `updatePreview` write union —
   set/unset/addItem/removeItem/reorder, sibling anchors,
   server-minted item ids, `@oneOf` cell states per kind —
   all-or-nothing as **one entry per batch** (P.9 Q4): origin
   `entered`, actor the viewer's account, authored against the head
   revision at write time. Participants only; writes go through the
   applicant surface (P.4's fixed pair).
3. **Refusals.** `INVALID_WRITE` for a batch the record refuses —
   the G.12 set (unknown column, kind mismatch, bad anchor, bad
   reorder, conformance) **plus the surface refusal new here**: a
   cell op on a column not writable through the writer's surface, or
   an item op on a group the surface does not carry (§2.9 *surfaces
   absorb writability*); the message names the offender and nothing
   is stored. An **empty batch is `INVALID_INPUT`** — no entry is
   minted for nothing (`updatePreview` tolerates it because a bag
   write of nothing writes nothing; a log is different). `CONFLICT`
   for a lost race between co-participants (the store's next-seq
   rule surfacing; server-side `base_version`, P.4). `FORBIDDEN` for
   a non-participant or an absent case file, one answer. Legal in
   `DRAFT` — the only state today; `SUBMITTED` inherits per G.1 with
   the P.4 *Two-sided editing* caveat. Admissibility never refuses
   (the standing G.12 rule): findings are the output, not a gate.

## G.15 `submitCaseFile` (settled 2026-08-28)

The first checkpoint (dépôt): the record log gets its first lifecycle
op, the case-file state machine its second state, and admissibility
its one gate. The kernel machinery exists whole (§2.9 checkpoints,
§2.8 expected resolutions); this slice is the platform composition.

1. **`submitCaseFile(input: { caseFileId })` → the full `CaseFile`.**
   Participants only. The use case, one transaction: evaluate
   admissibility of the record through the **applicant surface**,
   pending set from the fold — §2.8 is explicit that DN submits
   incomplete records while resolutions are pending, so the gate is
   "no applicant finding *that pending does not excuse*" (vacuously
   the plain no-findings rule until resolvers exist); refuse as
   **`INADMISSIBLE`** — a tenth G.6.4 code, minted because this is
   the one place admissibility gates (G.12/G.14's "never refuses"
   holds everywhere else): the message carries the finding count,
   the findings themselves are already readable on the object. Then
   the kernel **checkpoint entry** through the `varve-service`
   checkpoint operation — name `submitted`, `reading_revision` the
   head revision (the §2.9 `pinned` default, recorded at its
   source), `expected` empty until resolvers, frozen sets **empty**:
   dépôt does not lock the applicant form (P.4/Q12 — instruction
   does, at P1). Then the platform state column mirrors
   `Submitted { since }` in the same transaction — **P.9 Q3
   confirmed**: the checkpoint is authoritative, the column a read
   model maintained only by use-case services. **No
   `case_file_events` row**: the record log holds the lifecycle fact
   (the P.4 event-log split doing its job).
2. **State shapes** (G.2 rule 5): the union grows
   `CaseFileSubmittedState { submittedAt }`, the enum `SUBMITTED`.
   Submitting a submitted case file is `INVALID_TRANSITION`.
   `updateCells` stays legal in `SUBMITTED` unchanged — the §2.9
   thesis: the case file is editable until instruction, and the
   dépôt checkpoint froze nothing.
3. **Refusals**: `FORBIDDEN` (non-participant or absent, one
   answer), `INVALID_TRANSITION` (already submitted),
   `INADMISSIBLE`, `CONFLICT` (the row race, as `updateCells`).
4. **Reads stay head-lens at P0.** The checkpoint records the pin;
   the API keeps folding through the head publication (G.14) until
   the reviewer side lands mixed-revision reading (DESIGN §5.5) —
   whether and where the `pinned` lens reaches the API is G.5 Q6.

## G.16 The anonymous viewer and the published catalog (settled 2026-08-28)

The app's root page `/` is the visitor's front door: the catalog of
published procedures, rendered with or without a session. P.1 rule 1
makes that an API gap, never an internal route — the rejected
alternative was the page calling `platform-core` directly, one
"exceptional" bypass that would put the app's highest-traffic page
outside the dogfooded schema and leave the catalog's read shapes
unpinned in the SDL. What made the bypass tempting — reluctance to
open an unauthenticated path into the API — dissolves by separating
two decisions the architecture already keeps apart (the P.3 seam:
`platform-app` owns transports and principal resolution): whether the
*schema* can answer an anonymous viewer, and whether the *HTTP
endpoint* accepts requests without a token. Only the first is needed,
and only the first is settled here.

1. **The principal grows an anonymous variant.** The executor keeps
   receiving an already-resolved principal as request data
   (`platform-graphql` reads no transport); the principal is now an
   account principal *or* the anonymous one. The app already
   resolves a page's session to "account or none" (P.7); a public
   page builds its in-process transport with the anonymous principal
   instead of having none to build. `/graphql` is untouched:
   bearer-only (G.3, P.7), and every token resolves to an account,
   so anonymous is unreachable over HTTP **by construction** — no
   exclusion list, no flag. Opening anonymous HTTP access later is a
   transport-policy decision needing no schema change, deliberately
   deferred until an integrator need exists.
2. **`UNAUTHENTICATED`, the eleventh G.6.4 code.** The shared
   `session` helper — every account-gated resolver's one door to the
   principal — refuses the anonymous principal with it, so the
   existing schema closes to anonymous without any resolver
   changing, non-null `viewer` included; a public field opts in by
   reading the principal through an anonymous-tolerant sibling.
   Distinct from `FORBIDDEN`, which presumes an account that lacks a
   right: `UNAUTHENTICATED` says nobody is signed in and signing in
   is the fix. The G.6.2 null discipline (absent and invisible are
   one `null`) governs among authenticated viewers; anonymous gets
   the uniform refusal and learns nothing about any id.
3. **`publishedProcedures(first, after): ProcedureConnection!`** —
   the catalog root field: every `Published` procedure of every
   organization, answering any viewer, anonymous included. `Draft`
   is never listed (a never-published procedure does not exist for
   non-members — G.6.2 extended to the world); `Closed` leaves the
   catalog while its case files live on, and re-publication re-lists
   (P.4: publishing from `Closed` is the reopen). The pagination
   shape is G.13.2's: forward-only, `first` defaulting to 25 and
   capped at 100, opaque `after`, newest first (id descending —
   UUID v7 is creation order); nodes are `ProcedureRef` (G.2
   rule 1). The schema's second connection type.
4. **`ProcedureRef` gains `description: String!`** — a scalar, so
   rule-1 legal: the catalog card is title, organization, and
   description, and a dedicated catalog node type would fork a
   parallel Ref shape over one field. Catalog metadata is the live
   platform row — title and description stay editable after
   publication: the catalog names the procedure, the schema stays
   behind publications.
5. **The catalog stops at the card.** `procedure(id)` stays
   member-only, and nothing else opens to anonymous in this slice.
   What a visitor does next — the public procedure page, the
   "commencer" flow into `createCaseFile` through the sign-in gate
   and the P.7 return-to cookie, catalog search — is the portal
   slice, opened as G.5 Q7; where a card links is that question's
   first decision.
