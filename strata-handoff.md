# Strata — design handoff

> Working code name. Nothing published to crates.io yet.

## 0. Context

DN is a ~10-year-old French public-service platform: administrations build
"procedures" (schemas) and citizens submit "dossiers" (records). Millions of
records. The author has been a core contributor for ~10 years.

Goal: extract a **generic, embeddable kernel** in Rust, aimed beyond the French
administration.

**Explicit risk being managed: second-system syndrome.** Every feature must earn
its place by appearing in the real DN corpus.

## 1. Thesis

The valuable extraction is **not the form builder** — that is commodity, and a
dozen good open-source ones exist (Orbeon, form.io, Open Formulieren, SurveyJS,
ODK).

The valuable extraction is the **versioning kernel for multi-party case files**:

- schema revisions applied to hundreds of thousands of *live* records
- records as long-lived case files appended to by many actors over time (§2.9),
  not frozen submissions
- a pure conditional-logic language, statically typed against a revision
- the static-analysis tooling that falls out of the above

**Nobody occupies this intersection.** Form platforms treat schema change as an
afterthought and freeze or corrupt historical submissions. Versioned databases
(Dolt, TerminusDB) do branch/diff/merge beautifully but know nothing about
forms, conditional logic, or presentation. No-code DBs (Airtable, Baserow,
Quickbase) do tables and views but not lossless evolution with historical
fidelity.

The product artifact that no competitor can offer: **an impact report shown to
an administration before it publishes a revision.**

## 2. Core model

### 2.1 Structure

- **Schema** — versioned structure. Deliberately *not* called "table": that
  would promise SQL semantics (joins, FKs) not being delivered.
- **Revision** — immutable published schema version. Content-addressed.
  A checkpoint is a tag; the revision is the object.
- **Column** — a typed field. Has an **arity**: `one | many`.
- **Group** — ordered container of columns. Has a **cardinality**: `one | many`.
- **Block** — a published, reusable group definition with its own identity and
  version, referenced by inclusion.
- **Record** — instance of a schema, bound to a revision.
- **Item** — one instance of a `many` group. (Avoid "row" — it collides with
  export rows.)
- **Cell** — value at `(column_id, row_path)`.
- **Surface** — presentation + admissibility tree over a revision. A "form" is
  one *kind* of surface, alongside review screens, export layouts, print
  templates.

### 2.2 The two multiplicities (do not conflate)

| | introduces | row_path | example |
|---|---|---|---|
| **group, cardinality `many`** | a **scope** | contributes a segment | repeatable block |
| **column, arity `many`** | a **value** | contributes nothing | multi-select, multi-file |

Test: if elements need independent reference from logic rules, or are
heterogeneous across several columns → **group**. If homogeneous and the set is
the semantic unit → **list-valued column**.

Consequence: **attachments are a list-valued column, not a group.** So
multi-file inside a repetition block stays at depth 1. Multi-select proves the
principle — nobody wants each checked option to be a separately-addressable
item.

### 2.3 Depth-1 is a policy, not a type

`row_path` is a **possibly-empty sequence**. Storage, addressing and diffing
already work at depth N. `depth <= 1` is enforced as a *schema validation rule
with an error message*, never compiled into the types.

Rationale: the real cost of depth-N is in the logic language (two scopes →
lexical scoping), not in storage. Ship two scopes; don't foreclose the rest.

The genuine casualty of depth-1 is **repetition inside repetition** (buildings →
apartments; household members → income sources). **Resolved from DN
experience:** DN never supported it, and the few requests over the years were
refused without much pushback. Depth-1 stands. `row_path` keeping storage and
addressing depth-N-ready is the entire extent of the accommodation — a door
left open, not a roadmap item.

### 2.4 Cells and identity

Two levels of identity, deliberately separated:

- **Addressing identity** — `(column_id, row_path)`. What logic rules,
  projections and exports can name.
- **Value-internal identity** — element IDs inside a list-valued cell (e.g.
  files). Used by diff only. **Opaque to the logic language.**

This gives fine-grained file diffs ("file 2 replaced") without paying for a
scope.

**Cell state is two orthogonal axes, not one enum:**

- **Stored state** — `absent` (never written) | `empty` (written, blank) |
  `value`. Intrinsic to the cell; this is what the wire transmits.
- **Reachability** — derived at read time from surface + revision + logic.
  Never stored, never transmitted. It is **surface-relative**: the same cell
  can be reachable on the back-office surface and unreachable on the public
  one, so "unreachable" is a classification *with respect to a surface*, not
  a state a cell is in.

**Kernel rule: hidden never deletes.** A cell may hold a value written before a
condition toggled it hidden; that value persists — and must round-trip through
export (§5). Deleting on hide is irreversible data loss the moment a condition
is toggled, and makes projection non-deterministic.

### 2.5 Group values are derived

A group's value is a **view over the cells sharing its prefix**, never a stored
blob. Atomic validation and atomic casting of a published block operate on that
view and emit a new set of cells. Flat storage, composite semantics, no
duplication.

Cardinality `one` = "exactly one implicit item contributing nothing to
row_path", making the path rule uniform.

### 2.6 Schema vs surface — the load-bearing separation

- **Schema defines what is *representable*.** Types, arity, cardinality,
  structural constraints.
- **Surface defines what is *admissible*.** Required-ness, visibility,
  ordering, prompts, help text, alerts, pagination.

Therefore **`required` is not a schema property.** The same record can be
complete for the public application surface and incomplete for the back-office
review surface, and neither is lying.

A record is never globally "invalid" — it is **non-admissible with respect to a
surface**.

Format constraints get the same treatment (**settled from the M0 residue**):
email, phone, IBAN, regex formats are admissibility, not representability.
The schema type is plain `text`; the format check lives in the surface.
Casts stay trivial, strictness may differ per surface, and a mis-formatted
value is non-admissible — never ill-typed.

Logic splits accordingly: type-level predicates in the schema; visibility and
requirement rules in the surface. Both compile against a specific revision and
are re-checked on every new one.

## 2.7 External resolvers (SIRET-style externally-fed fields)

A key goes to an external source; a payload comes back; cells are derived from
it. DN experience gives two flavours:

- **direct match** — one or a few fields yield a unique result (SIRET)
- **autocomplete** — a search yields candidates, the user picks one

**These are one mechanism, not two.** Both reduce to: a **key**, a **snapshot**
of what the source returned, and **mapped cells** derived from it. The
difference is only *how the key is populated* — typed vs selected — which is
entirely a surface concern.

Consequence: an autocomplete-fed group needs a system column holding the chosen
candidate's key. Without it you cannot explain why those cells hold those
values, or re-derive them.

**The kernel never fetches.** IO is Tier 5. The kernel's job is to make the
mapping statically checkable and to record where values came from.

### Provenance is a kernel concept

Every cell carries an origin:

```
origin: entered
      | derived { source, source_version, mapping_version, snapshot_ref }
      | overridden { superseded? }   // the derived origin it replaced
```

`source` is a resolver, or a prefill payload (see below) — same shape either
way. **`overridden` retains the provenance it replaced.** That makes
divergence — what the source said vs what the human wrote — a cell-local
read instead of a log walk (which matters at millions of records), and makes
*restore* a re-derivation from `superseded.snapshot_ref`. The one case where
`superseded` is absent: overriding while a resolution is still pending
(§2.8) — the overriding entry cannot reference a snapshot that does not yet
exist, so there the landed snapshot lives on the resolution instance, which
sits beside the cells precisely for this.

Orthogonal to stored state and reachability (§2.4) — those say what the cell
holds and whether a surface shows it; this says where the value came from.

Load-bearing for three reasons:

- **Diff** — "INSEE data refreshed" and "a human edited the company name" are
  different events and must not look alike.
- **Legal weight** — administrations must distinguish *données déclaratives*
  from data drawn from a *référentiel authentique*. This is the basis of
  "dites-le-nous une fois" / the EU once-only principle. No form platform does
  this properly.
- **Impact** — if a mapping changes, which cells are affected?

### Schema side: resolver declaration

Versioned like everything else:

- ID and version
- **input signature** — which columns feed it, with types
- **declared result type** — the shape of what comes back
- **mapping** from result fields to columns

Declaring the result type *in the schema* is what makes the whole thing
analysable; otherwise an untyped blob lands in typed cells.

**Mapping is projection.** Typed source structure → typed target structure,
per-field casts, lossiness report. `strata-projection` already does exactly
this, with a different source. No new machinery.

### Surface side

Trigger mode (blur / live autocomplete / explicit button), candidate
presentation, and **whether derived cells may be overridden**. The split pays
off immediately: back-office review can permit an address correction that the
public form forbids.

### Worked example: SIRET, the three-layer decomposition

DN has a first-class `SiretChampDescriptor`. The kernel has no SIRET type —
the ensemble decomposes across the three layers, each holding exactly what
it owns:

- **Schema** — a published block ("entreprise") of *plain* columns:
  `siret: text` (the key), `raison_sociale: text`, `date_creation: date`,
  address columns… nothing special about any of them. Beside the block, a
  **resolver declaration**: id + version, input signature (`siret: text`),
  declared result type (the INSEE payload shape), mapping from result
  fields to the block's columns. The declaration is a versioned schema
  *object* — like a block, not like a type. Publishing typechecks the
  mapping against the columns it feeds.
- **Surface** — trigger mode (typed key on blur, or explicit button),
  candidate presentation if autocomplete, and whether the mapped cells may
  be overridden *here* (back-office yes, public form no).
- **Record** — the retained INSEE payload (content-addressed snapshot), a
  resolution instance with its lifecycle if the fetch was deferred (§2.8),
  and a `derived { source, source_version, mapping_version, snapshot_ref }`
  origin on every mapped cell.

This answers M0's "which composites are first-class in the kernel vs merely
published blocks": **none are first-class.** SIRET, Address (BAN), RNA,
RNF, AnnuaireEducation are all block + declaration pairs — library content,
shippable, versionable and retirable without touching the kernel. Static
sources (commune, pays, département…) collapse further, into nomenclatures
(§2.12).

### Retain the payload, not just the cells

Keep the raw response, content-addressed, keyed by
`(resolver, resolver_version, input_key, fetched_at)`.

- Mappings change — v2 extracts a field v1 ignored. **Re-map without re-fetch**,
  across a whole table, is only possible if the payload was retained.
- The API may be down, deprecated, or the historical value simply unrecoverable.
- Audit: what did the source actually say on the day the decision was made?

Same content-addressed blob pattern as attachments — **design the two
together**.

### Snapshot vs live

Declared per resolver. **Ship snapshot semantics only.** A decision was made on
the basis of certain facts; freezing them is what administrations need.
Reference-at-read-time stays declared-but-unimplemented rather than foreclosed.

### Prefill is a push-mode resolver

Heavily used DN feature: records created prepopulated, via API call or query
params. This is not a new mechanism — it is the third flavour of the one
above. The only difference is that the payload is **pushed at record
creation** instead of fetched against a key. Everything else carries over
unchanged: the payload is retained content-addressed, cells are derived from
it through a mapping, and every prefilled cell carries a `derived` origin
pointing at the snapshot.

- Query-param prefill uses the trivial mapping (column ↔ value); API prefill
  may use a declared typed mapping like any resolver.
- No key, no resolution lifecycle — the payload is present at creation or
  not at all.
- **Restore falls out for free.** "Reset this field to its prefilled value"
  is an ordinary `set` op re-derived from the retained snapshot — the same
  re-map machinery as bulk re-mapping, at single-cell scale. No special
  kernel operation; the override stays in the log, the restore is just the
  next entry.

A fourth flavour — a resolver whose source is static, versioned data
rather than an API — is the **nomenclature** (§2.12).

## 2.8 Deferred resolution

DN allows submitting an incomplete record and fetching later when the resolver
recovers. This is not an add-on — it is the constraint that settles several
otherwise-loose questions.

### It forces the record log to exist

A record mutates **after submission, without human action**. So records need an
append-only patch log with an **origin on every patch**. Not full event
sourcing (§6 still holds), but "the log is optional" is now false.

### Resolution instances have a lifecycle

Confirms they sit *beside* cells, not inside them. Per
`(group instance, resolver)`:

```
pending → resolved | not_found | ambiguous | failed | abandoned
```

plus attempt count, last error, deadline.

**Abandonment must be an explicit recorded event.** Pending-forever is a leak;
silent give-up is unauditable.

Kernel contributes one pure function — `pending_resolutions(record)` — so a
Tier 5 scheduler can drive retries without the kernel knowing about queues or
clocks.

### Three rules (RATIFIED)

1. **Version binding at request time, not completion.** A resolution requested
   under revision N may land when the schema is at N+3 with a changed mapping.
   Bind resolver version *and* mapping version at request, so the same
   submission resolves identically whenever the API returns. Moving to the new
   mapping is then an explicit bulk re-map from retained snapshots —
   deliberate, reportable, reversible.

2. **Override wins over late resolution.** If a user fills a derived-target cell
   manually while resolution is pending, the cell becomes `overridden` and
   resolution must not clobber it — but the snapshot still lands, so divergence
   stays visible and re-derivable.

3. **Pending is readable by the logic language.** Resolution status is part of
   record state, so a surface can express "required unless pending". Keeps
   admissibility binary rather than adding a third value.

All three are ratified. Rule 2 is the one the rest of the design leans on:
`overridden { superseded }` (§2.7) and prefill restore encode exactly its
asymmetry — machine values never win by force, and are always one explicit
entry away from restoration. Note the cost of rule 3: resolution status
becomes part of the logic language's typed input environment — still pure
and total (status is an input like cells), but `strata-logic`'s environment
must include the revision's resolver declarations.

### Checkpoint interaction

This partially re-opens locks (§6), cleanly:

> **A checkpoint freezes entered cells and enumerates the pending resolutions it
> expects to be filled.** Late derived writes are legal only if they were on
> that list. Anything else is rejected.

Defensible legal story — *this is what the applicant declared, these are the
référentiel lookups still outstanding* — without building general-purpose
locking.

### Wire format

New line kinds: `resolver` (declarations, part of the schema), `snapshot`
(payloads, part of the record's meaning), `resolution` (instances with status).

The resolver *implementation* — endpoint, credentials, rate limits — is
instance-local and does not travel.

Design goal worth stating explicitly: **an imported record remains fully
meaningful on an instance with no access to INSEE.** That is the difference
between portable records and records that decay. Pending resolutions import as
pending-and-unresolvable, which is correct and honest.

### New impact-report questions

- resolver result type changed → which mappings break
- mapping changed → which cells are stale **and re-derivable from retained
  snapshots**
- resolver removed → which columns are orphaned, which records have pending
  resolutions against it

The middle one is a genuinely valuable bulk operation, and it exists *only*
because the payloads were retained.

## 2.9 Records are case files, not submissions

**Core platform behaviour, confirmed from DN:** after submission a record
continues to be appended to by **multiple actors** — the applicant, one or more
instructors, third parties, and resolvers (§2.8). A record is a long-lived
collaborative case file, not a frozen form submission.

This is the sharpest differentiator against the entire form-platform landscape,
and it should be stated as the kernel's subject rather than discovered later.

### The log entry IS a wire patch

```
Entry {
  seq,
  prev,                       // hash of entry seq-1; fixed genesis value at seq 0
  base_version,               // what this was computed against
  actor,                      // opaque id + kind: human | resolver | system
  origin,                     // ties to cell provenance (§2.7)
  authored_against_revision,
  timestamp,
  ops: [ set | unset | add_item | remove_item | reorder ],
  note?                       // optional human reason
}
```

The ops are exactly the §5 wire ops. So **the log, the export, the migration
stream and the diff are one representation.** One serializer, one apply
function.
Current state is `fold(log)`, with periodic snapshots for performance at
millions of records.

Entries are content-addressed (canonical serialization is already required for
revision hashing). Content-addressing alone is only *per-entry*
tamper-evidence; the **chain** comes from `prev` — each entry commits to its
predecessor's hash. `base_version` cannot serve as that link: concurrent
entries deliberately share a base (that is how conflict detection works), so
it is not a linear pointer. `prev` and `seq` are assigned together, at append
time, by whoever owns the log.

Snapshots must stay verifiable or they become the tamper hole: a snapshot
records the hash of the last entry it folds, so any snapshot can be audited
by refolding the chain up to that entry. Tamper-evidence on administrative
records is worth having at the cost of one hash field per entry.

### A record is not "on" a revision

**Correction to the earlier model.** An applicant submits under revision N; an
instructor edits when the procedure is at N+2. Therefore:

> Each **entry** is authored against a revision. The record's revision is a
> **reading lens**, not an intrinsic property.

Consistent with §3 — cells are revision-agnostic, only interpretation is
revision-dependent. The log is heterogeneously typed and that is fine.

Consequence: the reading lens is an explicit per-schema policy —
**`pinned` (at submission) or `latest`**. Administrations will have strong
opinions and they will not all be the same one.

**Settled (was open question 10): the default is `pinned`.**

- It is what DN does today, and what the rest of the design already believes:
  resolver snapshots freeze what a decision was based on; a declaration is
  read as it was declared.
- Publishing a revision must never change the meaning of existing records.
  Under `pinned` it cannot; under `latest`, every publication silently
  re-interprets every live record — the impact report becomes a description
  of damage already done rather than a preview.
- Nothing is lost: `latest` is always one explicit projection away (with its
  lossiness report), and cross-record views already read through the
  aggregate revision (§5.5), which is where latest-like behaviour is
  actually wanted.

**The policy may change after records exist.** The lens is pure
interpretation — cells are revision-agnostic, so switching touches no bytes
and is instantly reversible — but it is a reportable act: run the impact
machinery over the switch as if it were a publication (which records change
admissibility or rendering under the new lens?). Checkpoints are immune by
construction: each pins its own reading revision, so past decisions do not
re-interpret when the live lens moves.

### Concurrency: detect, do not merge

Per-cell LWW (§6) still holds, but with multiple actors it can no longer be
silent. Every entry carries the `base_version` it was computed against, so the
kernel detects that two actors wrote the same cell from the same base and
**reports** it.

Detect-and-report is not merge. Cheap, and exactly what an instructor needs to
see. **Record branching stays cut.**

Free consequence: **diff between any two log points** — "what changed since I
last opened this file" — a real daily need for anyone processing cases.

### Surfaces absorb writability

Different actors get different surfaces at different lifecycle points. So a
surface declares a **per-column write policy** alongside visibility and
requiredness — generalising the "may derived cells be overridden" rule of §2.7.

Authorization then reduces to *which surface does this actor get*, and stays
**entirely out of the kernel**. The kernel never needs a permission model.

### Entry visibility derives from surfaces

**Settled (was open question 9): there is no per-entry visibility class.**
Confirmed from DN practice: everything actor-restricted lives in restricted
*columns* (the private-annotation pattern); messages and third-party opinions
are platform features beside the record, not log entries; applicants see
current state, not history.

Visibility therefore reduces to machinery that already exists:

> An entry is visible through surface S iff it touches at least one column
> visible in S — and only those ops are shown.

The kernel contributes one pure function — `filter(log, surface) → redacted
log` — and the platform's only job remains *which surface does this actor
get*, the same reduction as authorization above. No entry ACLs, and no
side-table of entry permissions that would fail to travel on migration.

Chain interaction: omitting entries from a filtered history export would
break `prev` verification. A filtered export therefore uses **redacted
entries** — envelope transmitted (`seq`, `prev`, content hash), content
withheld — verifiable because canonical hashing is already erasure-tolerant
(§2.10). Visibility filtering and erasure ride the same mechanism. Specified
now, needed later: applicant-facing exports are folded state, and operator
migration carries full history, so nothing requires redacted entries on day
one of `strata-wire`.

### Checkpoints, now precisely defined

> A checkpoint is a named entry hash in the log — the hash, not the seq, is
> what pins content — plus a reading revision, plus the set of pending
> resolutions expected to land after it.

## 2.10 Erasure (GDPR) is a kernel design input

DN practice today: full records erased after a set retention period. The
design must guarantee that baseline and keep finer-grained erasure possible.
An append-only, content-addressed, hash-chained log looks like the natural
enemy of erasure; it isn't, provided three commitments are made now rather
than retrofitted.

### Baseline guarantee: whole-record erasure

- **Chains are per-record.** `prev` links entries of one record's log;
  nothing global commits to record hashes. Deleting a record's entire log
  invalidates nothing else.
- **No global anchor, ever.** A Merkle root over record heads would upgrade
  tamper-evidence into a completeness proof — and silently break erasure.
  The trade is explicit: a chain proves a *surviving* log is intact, not that
  a record ever existed. For a system with retention obligations, that
  asymmetry is a feature. Adding a global anchor "for extra tamper-evidence"
  later would be a regression, not an improvement.
- **Content-addressed blobs are shared by design** — the same uploaded
  document, the same INSEE payload for one SIRET, referenced by several
  records. Erasure therefore requires reference counting (or a sweep) in the
  blob store. Without it, erasing a record either strands personal data in
  unreferenced blobs or deletes a blob another record still needs.

### Hash the ciphertext, never the plaintext — a `strata-core` invariant

Whatever finer erasure mechanism is chosen later, entry hashes must not
commit to plaintext values: after erasure, a retained hash over plaintext is
a brute-force oracle for low-entropy fields (birth dates, postal codes,
SIRET). Canonical serialization must be defined over an **erasure-tolerant
encoding** from day one — hash-of-ciphertext, or per-value random salts that
are destroyed with the values. This changes what the canonical bytes *are*;
it cannot be retrofitted after M3 without invalidating every existing hash.

### Intra-record forgetting: two candidate mechanisms, choice deferred

Both build on the invariant above, which is why the encoding decision is
urgent and the mechanism choice is not:

- **Crypto-shredding** — value payloads encrypted per `(record, epoch)`;
  erasure = key destruction; the chain still verifies over ciphertext.
  Cost: key management at Tier 5.
- **Externalized values** — entries store `H(salt ‖ value)`; values live in
  a separate value store; erasure is actual deletion, leaving an explicit
  *redacted* hole in a still-verifiable chain. No keys; one indirection per
  read.

Shared constraints either way:

- **Fold needs a foothold.** `fold(log)` cannot cross erased entries.
  Erasing up to a horizon requires a retained snapshot *at* that horizon,
  which becomes the effective genesis for refolds. One of the §2.9
  performance snapshots becomes load-bearing rather than a cache.
- **Erasing history is not erasing live data.** A value set once and never
  overwritten survives in the horizon snapshot — correctly: shredding
  history erases *superseded* values, i.e. the audit trail. An erasure
  obligation covering live data is an ordinary `unset` entry *plus*
  shredding of history. Key destruction alone does not discharge it.

### Decide the mechanism from the corpus

Whole-record erasure is what DN does today. Whether intra-record horizons
(erase the trail, keep the case) are ever actually required decides whether
either mechanism gets built at all — a §12 question, not a taste question.

## 2.11 Enum options carry identity

**Settled from DN experience.** DN's enum options are bare values, and
"rename an option without losing selections on existing records" is a
recurring, unmet demand. The fix is the kernel's own identity principle
(§3) applied one level down: an option is **`(id, label)`** — cells store
the id, labels live in the revision, and a rename changes interpretation,
not data.

- **Rename → free.** Zero records touched, zero cast.
- **Rules are rename-proof.** Logic rules reference option ids, so renames
  drop out of the breaking-change set entirely — with bare values, every
  rule mentioning the option breaks silently.
- **Removal is precise, not catastrophic.** A removed id follows the
  `deprecated_since` pattern (§5.5) rather than deletion, and the impact
  report counts exactly the records holding it.
- **enum→text projection needs a lens.** The emitted string is the label
  resolved through the projection's reading revision — existing machinery.
  CSV mirrors the two-header-row pattern: id authoritative, label
  cosmetic.
- **Labels live in the schema, not the surface.** They resemble prompts,
  but per-surface option labeling is unasked-for complexity; a surface can
  override presentation later without a kernel change, and schema labels
  keep exports and migration self-contained.
- **Codelist foundation.** External codelists are natively `(code,
  libellé)` — commune codes over renamed municipalities, COG editions. A
  codelist-backed enum (the remaining M0 residue) becomes the same shape
  with pairs sourced from a versioned published object instead of declared
  inline.

Migration from DN: synthesize ids from legacy values (slug or hash), label
= the original string; existing cells map over mechanically.

## 2.12 Nomenclatures (referential-backed enums)

**Settled — closes the last M0 residue item.** A **nomenclature** is a
versioned, content-addressed table of `(id, label, …fields)`. It is to
values what a block is to structure: published with its own identity and
version, referenced by inclusion.

- **Every enum is nomenclature-backed.** Inline options are the degenerate
  case — a small nomenclature owned by the schema. One concept, no special
  cases; §2.11's `(id, label)` is the row shape. **Inline nomenclatures
  have no identity and no ceremony**: they version with the revision that
  contains them (ids synthesized by the authoring tool), exactly as inline
  groups do — publication with standalone identity is the lift-out, the
  same relationship a group has to a block. The resolver aspect stays
  dormant until rows carry more than `(id, label)`.
- **It is the fourth flavour of the §2.7 mechanism** — a resolver whose
  source is data, not IO. Key = the chosen id; "payload" = the row; mapped
  cells derive extra fields (pick a commune → the département fills).
  Trigger and candidate presentation stay surface concerns, as ever.
- **Stronger than the API flavours on both §2.7 axes:**
  - *Portability.* An API resolver's implementation is instance-local and
    does not travel (§2.8); a nomenclature travels in the wire stream like
    a block (`nomenclature` line kind, schema-side). These resolvers are
    fully portable — imported records need no caveat at all.
  - *Determinism.* Resolution is synchronous and total: no pending
    lifecycle, no retries, no abandonment. Per-pick snapshots are
    redundant — binding `(nomenclature, version, id)` suffices, since the
    version is already content-addressed.
- **Typing the API flavours can never have.** An enum column typed "id
  from nomenclature N@v" gives the checker a closed id set: rule literals
  and exhaustiveness become statically checkable. Version bumps run
  through the §3 enum rows — label edits free, removed ids flagged with an
  exact record count.
- Lives in `strata-schema`, beside blocks.

**Name.** Chosen for domain authenticity: INSEE publishes *nomenclatures*
(COG, NAF, PCS) — the kernel uses the word of the référentiels world it
models. Rejected: `codelist` (accurate SDMX term, colder), `codebook`
(survey-practice kin but documents variables — broader), `vocabulary`
(implies words and RDF semantics), `valueset` (misuses the FHIR precedent —
a ValueSet is a selection from a code system), `taxonomy` (promises
hierarchy), `registry` (promises mutability and a central authority).

## 3. Change classification

Because column IDs are stable, **cells are revision-agnostic; only their
interpretation is revision-dependent.**

| change | projection cost |
|---|---|
| column added | free (absent) |
| column removed | free (ignored, retained) |
| column moved **within the same scope** | free |
| **column moved into or out of a `many` group** | **cast required — probably breaking** |
| **column retyped** | **cast required** |
| **arity or cardinality changed** | **cast required** |
| enum option label edited | free (§2.11) |
| enum option added | free |
| enum option removed | flagged — id retained with `deprecated_since`; impact report counts records holding it |

> **Correction, found via §5.5.** "Column moved" is only free when `row_path` is
> unchanged — i.e. between two `one` groups. Moving into or out of a `many`
> group changes the column's row_path **arity**: root-scoped for some records,
> item-scoped for others. In a flat sheet it cannot occupy a single header.
> Treat as breaking.

`many → one` is the first genuinely lossy cast that isn't type narrowing.

Projection is therefore a no-op over the vast majority of any record, and the
compatibility relation is a **per-column function** — i.e. exactly Avro's
reader/writer schema resolution. Lift that model wholesale.

## 4. Logic language

Its own crate. The crown jewel and the highest-risk component.

- Pure, **total**, no recursion, no unbounded iteration
- Typed against a specific revision
- Two scopes: record, item
- Shared by validation, visibility, requiredness, computed values, routing
- **Statically analysable**: which schema edits break which rules

Fuzzers and a property-test corpus from day one.

## 5. Wire format

**Tagged JSONL.** The point is not just streamability — it's that the stream can
be **heterogeneous**, carrying schema, records, items, attachments and history
in one file in dependency order.

```
{"k":"header", ...}                 // format ver, kernel ver, source instance, manifest
{"k":"revision", "id":"...", ...}   // writer schema travels with the data (Avro property)
{"k":"block", "id":"...", ...}
{"k":"nomenclature", "id":"...", ...}   // versioned (id, label, ...fields) table (§2.12); travels like a block
{"k":"record", "id":"...", "lens":"...", "cells":{...}}   // snapshot mode; lens = fold revision, not a record property (§2.9)
{"k":"item", "record":"...", "group":"...", "id":"...", "ord":0, "cells":{...}}
{"k":"entry", "record":"...", "seq":0, "prev":"...", "ops":[...], ...}  // history mode: one log entry (§2.9)
{"k":"attachment", "sha256":"...", ...}
```

### Key unifications

- **A snapshot export is a patch against the empty state.** Operation tags
  (`set`, `unset`, `add_item`, `remove_item`, `reorder`); a snapshot export
  just never uses more than `set`/`add`.
- This collapses **export, import, cross-instance migration, and record diff**
  into one format and one apply function. For a kernel whose thesis is
  "incremental changes," the interchange format *must be* the change format —
  otherwise two serializers, which will drift.
- **Migration is one-way and one-time.** The receiving instance verifies and
  adopts each record's chain and continues appending to it; after migration
  the source stops writing. There is no reconciliation path, by design —
  bidirectional sync is cut (§6).
- **Export = projection + pivot.** A snapshot export is therefore always taken
  *through a single revision* (otherwise a retyped column yields one header
  with heterogeneous values below it). With a lossiness report.

### Two export modes, one op set

The log-centric model (§2.9) splits export into two modes sharing the same op
set and the same apply function:

- **History export** — the record's entry log verbatim, as `entry` lines
  (seq, prev, actor, origin, authored_against_revision, timestamp, ops).
  Lossless: preserves provenance, actors and the hash chain. This is the
  canonical cross-instance transfer format, and what makes an imported record
  fully meaningful on another instance.
- **Snapshot export** — the folded state as one patch against the empty state,
  projected through a single reading revision. Cheaper; loses history by
  design. The `record` line's `lens` field names the revision the fold used —
  a record is not "on" a revision (§2.9), so the field records the fold's
  lens, never a property of the record.

Do not blur them: a snapshot export must not carry `entry` lines, and a
history export must not carry folded `record`/`item` cell lines — a stream
mixing both has two sources of truth for the same cells.

(A third variant — history export filtered through a surface, with
chain-preserving redacted entries — is specified in §2.9 but required by
nothing on day one.)

### Stored state on the wire

- key absent from `cells` → absent
- key present, `null` → empty
- key present, value → value

Reachability is never serialized — always derived on read from surface +
revision (§2.4). Stored values are transmitted even when unreachable on every
current surface: "hidden never deletes" implies hidden must round-trip.

### Constraints

- **Lines must be bounded.** One-record-per-line breaks at 5,000 items. Hence
  `record` (root cells) and `item` lines are separate. Reader must handle
  "record not yet complete" — needs an explicit terminator or a
  contiguity rule.
- **Line 1 must carry everything needed to fail fast**: format version, kernel
  version, source instance ID, manifest of revision IDs / group IDs / counts /
  whether attachments are bundled or referenced. Import rejects on line 1 or
  commits to the whole stream. Apply into staging, then atomic swap — never
  stream into live tables.
- **Canonical serialization required** for content-addressed checkpoints. Adopt
  JCS (RFC 8785) or define one; hash the canonical bytes, never the emitted
  line.
- **No decimals or money in JSON numbers.** Strings for exact decimals,
  RFC 3339 for instants.

### CSV

A pure **downstream renderer** over the same stream. Not importable.

Design reference: REDCap's flat export (`redcap_repeat_instrument`,
`redcap_repeat_instance`) is the same shape and is proven at scale — including
the `NEW` sentinel for auto-numbering instances on import. Their users' one
complaint is analysis ergonomics, not correctness. So:

- **canonical interchange** = JSONL, lossless, round-trippable
- **convenience views** = CSV/XLSX, denormalized or per-scope, explicitly lossy,
  never re-importable

Leading columns for the CSV renderer: `record_id`, `scope` (group id, blank for
root lines), `item_id`, `item_ordinal`. Two header rows: label (row 1, cosmetic)
and column ID (row 2, authoritative). Repeat parent values on item lines for
human usability; `scope` governs ownership.

## 5.5 Aggregate revisions (mixed-revision table views and exports)

**Confirmed DN behaviour.** A dashboard or CSV export spans records locked on
different revisions. Headers must therefore cover columns from *all* revisions
in play. Two value strategies exist:

- project each row into **its own locked revision**, or
- project every row into an **aggregate revision** = latest revision + all
  previously existing columns.

**DN uses the aggregate strategy.** Adopt it.

### It is not an exception to "one revision per export"

The aggregate **is** the one revision — constructed rather than published. Make
it a first-class derived object rather than a special case in the export code.

### The aggregate is a lattice join

Safe casts define a partial order ("widens to"), so the aggregate type for a
column is the **least upper bound** across revisions — the exact dual of the
cast table (§2.x, now in `strata-schema`).

Conflict cases needing declared policy:

| conflict | policy options |
|---|---|
| retyped incompatibly (no join exists) | widen to opaque text / split / omit |
| removed then re-added with a different type, same ID | split by revision range |
| **scope moved** (root ↔ `many` group) | must split — cannot share a header |
| cardinality changed one ↔ many | widen to `many` where possible |

Every aggregation emits an **AggregateReport** listing which columns hit which
policy. Same shape as the impact report.

### Compute over the full revision DAG, not the result set

Tempting to aggregate over "revisions actually present in this filter" — do not.
That makes the aggregate a function of the query: it churns as records enter and
leave, nothing is cacheable, and two dashboards over the same data disagree.

Aggregate over the schema's **entire revision history**. Stable,
content-addressed, computed once per publication, cached.

Cost: columns no record in a given view uses. **Suppress all-empty columns at
render** — a presentation decision, not a semantic one.

### Guards

- The aggregate revision is **synthetic and non-publishable**. No record may
  ever be created on it. Without an explicit guard, eventually someone will.
- Columns absent from the latest revision carry `deprecated_since: rev N`, so
  table views can grey them and CSV headers can flag them.
- Its surface is auto-derived: latest revision's column order, then deprecated
  columns appended.

### Lossiness must not be silent

Aggregate projection casts every row, and some casts lose information across
potentially millions of rows. An aggregate export **must** carry a projection
report: how many cells were lossily cast, by column. Without it this is a quiet
data-corruption machine with a nice UI.

### Settles the export division

| path | revision handling | fidelity |
|---|---|---|
| **JSONL canonical** | history export (§5): each entry on its own authored revision; all revisions carried in-stream | lossless |
| **Table view / CSV** | aggregate revision, uniform typing | lossy + report |

> **Aggregation is a presentation concern, not an interchange one.**

This also disposes of a third option (aggregate headers with un-cast native
values plus a `_revision` column): JSONL already covers the lossless case.

### Crate placement

- type **join / LUB** → `strata-schema` (dual of the cast table)
- `aggregate(revision_dag) -> Revision + AggregateReport` → `strata-revision`
- consumed by `strata-projection`

## 6. Deliberately cut or deferred

| item | reason |
|---|---|
| record fork / merge / rebase (branches) | 10× cost multiplier for a use case that can't yet be named. Real stories (draft vs submitted, prefill-from-record, agent-proposed corrections, post-decision correction) are all served by **revisions + proposed-changes**. |
| general-purpose record locks | the real need is covered by checkpoints, since defined (§2.8–2.9): freeze entered cells, enumerate expected late writes, reject the rest. Locking beyond that still needs a user story before a design. |
| record references (`record_ref` scalar, DossierLink-style) | on ice. Many DN uses of DossierLink are better served by multiple surfaces over one schema — same case file, different actors — than by records pointing at records. Legacy links import as `text` holding an opaque id; promoting to a typed ref later is a checked cast. Needs proven demand from record-side data first. |
| bidirectional cross-instance sync | came up once in ~10 years of DN. Two instances appending to one record's log means concurrent `seq`/`prev` assignment — branching by another name, so the branch cut above applies. What is actually needed is **one-way, one-time migration** of schemas and/or records (§5), which the history export already covers. |
| workflows, labels, webhooks, GraphQL, document templating | platform, not kernel — separate repo, later year |
| full event sourcing on records | schemas are few and high-value → full git semantics. Records are millions → current state + append-only revision log + snapshots. Same machinery for both is how second systems get heavy. |

**Conflict unit:** field-path-level last-write-wins covers ~95% of real
administrative editing at near-zero cost. Full three-way merge on record trees
costs a year.

**Library first, one reference service second.** This is the main structural
defence against scope creep.

## 7. Crate plan

Strict DAG. Everything below Tier 5 is deterministic: no IO, no async, no clock.

**Tier 0**
- `strata-core` — IDs (column, group, record, item, revision), row path, scalar
  primitives (exact decimal, RFC 3339 instants), canonical serialization,
  content hashing. **Canonical encoding is erasure-tolerant (§2.10): hashes
  commit to salted or encrypted value encodings, never plaintext.** Depends on
  nothing.

**Tier 1**
- `strata-schema` — types, arity, groups, cardinality, blocks,
  nomenclatures (§2.12), structural constraints, depth policy. **Includes the cast table** — the compatibility
  relation between two types is a property of the type system itself — **and its
  dual, the type join / least upper bound** used to build aggregate revisions
  (§5.5). Canonical hash → revision ID. **The `Revision` object itself — an
  immutable, hashable schema snapshot — lives here in Tier 1**; `strata-revision`
  (Tier 3) owns only the DAG, publication and merge. That is what lets
  `strata-logic` (Tier 2) type-check against a revision without a Tier 3
  dependency.
- `strata-value` — cells, items, typed conformance, structural diff and patch.
  Pure and stateless. *(Narrowed: the record log moved out to
  `strata-record`.)*

**Tier 2**
- `strata-logic` — expression AST, parser, type checker against a revision,
  total evaluator.
- `strata-projection` — records viewed and edited through a revision they
  weren't written on. Casts applied, lossiness reported.
- `strata-impact` — what does publishing revision N+1 do? Change classification
  (safe / lossy / breaking), broken rule references, statically unreachable
  required columns, count of records whose cells fail the new cast.
  *(Name not final. Alternatives considered: `resolve`, `transit`, `morphism`.
  Avoid `compat` — in Rust that connotes legacy shims. Avoid `migrate` — implies
  one-way and destructive. Avoid `evolve` — implies forward-only.)*

`projection` and `impact` both depend on `schema`; neither depends on the other.

**Tier 3**
- `strata-surface` — presentation + admissibility tree. Depends on schema +
  logic. **Nothing depends on it** — that's the proof that "form isn't core."
- `strata-revision` — revision DAG, publication, three-way schema merge,
  **aggregate revision construction (§5.5)**.
- `strata-record` — the log (§2.9): entries, fold, snapshots, checkpoints,
  concurrency detection, resolution instances (§2.8). Depends on `value` +
  `schema`. Still deterministic — no clock, no IO; timestamps are inputs.

**Tier 4**
- `strata-wire` — tagged JSONL. Reader, writer, header/manifest, patch ops,
  apply.

**Tier 5 — IO appears here for the first time**
- `strata-store` (traits, async), `strata-store-postgres`, `strata-files`
  (content-addressed manifest + blob trait).

## 8. Milestones — corpus-first

**The first deliverable is not a library. It is an oracle over the real DN
corpus.** This is the strongest available antidote to second-system syndrome:
every feature must appear in the corpus to earn its place.

- **M0** (`core` + `schema` + `value`) — **Expressibility.** Can every DN
  procedure be expressed? The list of ones that can't, and why, *is* the type
  system requirements document.
  *Structural half achieved (`corpus/M0-expressibility.md`): all 42,723
  published procedures express and validate with zero residue, via
  `tools/m0` over `strata-core` + `strata-schema`. `strata-value` exists
  (cells, items, typed conformance, the five-op diff/patch with
  diff∘apply round-trip tested, element-level change reports); the corpus
  carries no records, so value conformance is exercised by tests only
  until record-side data arrives. Rule expressibility is M2.*
  - Also produce a **type frequency report**: how many real DN fields are SIRET,
    address, date, attachment, free text? Expect a handful of types to cover
    ~90%. This decides which composites are first-class in the kernel vs merely
    published blocks — much harder to reverse than it looks.

- **M1** (`projection` + `impact`) — **Falsification.** Can every *historical*
  revision transition be classified? Where DN actually broke or froze dossiers,
  does the impact report predict it? This is the thesis, and it is testable.

- **M2** (`logic`) — **Rule expressibility.** Can every existing conditional and
  routing rule be expressed and type-checked? The residue is either a needed
  language feature or complexity to refuse to carry forward.

- **M3** (`wire`) — **Round-trip.** The corpus in and out, byte-stable.

Only then: `surface`, `store`, service.

## 9. Guardrails

- One workspace, one repo, all crates `0.x`, **nothing published until M3**.
- `#![forbid(unsafe_code)]`.
- **No async below Tier 5.** If a Tier-1 crate needs a runtime, the layering is
  wrong.
- `serde` on wire types only — internal representations stay free to change.
- Every crate below Tier 5 testable with no fixtures beyond bytes. That's what
  makes property testing and fuzzing viable, and those are how the cast matrix
  earns trust.
- **No global commitment over record logs.** Tamper-evidence stays per-record;
  a global anchor would break whole-record erasure (§2.10).

## 10. Open questions

1. **Name.** "Strata" is provisional — and now known-compromised: as of
   2026-08-16, `strata` and `strata-core` are **taken on crates.io** (the
   rest of the family is free). A real name must be chosen before M3
   publishes anything; until then stop investing in "strata". Rejected
   `-DB` suffix: invites comparison to Dolt/TerminusDB on axes that are
   lost (query planner, replication, durability) while hiding the actual
   differentiator.
2. **`strata-impact` name.** Not settled.
3. **Attachments / files.** Needs its own design pass. A filename is ambiguous,
   a URL is instance-bound. Cross-instance export needs a manifest + bundle
   format with content-addressed references. Probably its own crate.
4. ~~Depth-1 demand.~~ **Resolved from DN experience.** The corpus cannot
   answer this — DN never supported nested repetition, so no demand signal
   exists in the data. The few requests over the years were refused without
   much pushback. Depth-1 stands as policy; `row_path` staying a sequence
   (§2.3) is the entire accommodation.
5. **Group-level atomic validation.** What exactly does a published block
   guarantee, and what does violating it produce?
6. **Import modes.** Full-replace-within-declared-scope vs explicit patch with
   an unchanged sentinel. Do not let one format do both.
7. ~~The three deferred-resolution rules in §2.8.~~ **Ratified** (§2.8) —
   version binding at request time, override-wins, pending-readable-by-logic.
8. **Resolver pluggability.** If the corpus shows a long tail of one-off
   integrations rather than a handful of shared référentiels, the resolver
   declaration must be pluggable from day one rather than an enum of known
   sources. Decide from the M0 data, not from taste. *First M0 signal
   (`corpus/M0-type-frequency.md`): DN itself grew a generic connector
   (`Referentiel`, 44 uses) and a dead one-off (`COJO`, Paris 2024, 5 uses)
   alongside the shared référentiels — leans strongly toward pluggable,
   plus a resolver-retirement story. Not yet closed: needs the resolver
   census (§12.6) over mapping-change frequency.*
9. ~~Log entry visibility.~~ **Resolved (§2.9): no per-entry visibility
   class.** Confirmed from DN practice that restricted data lives in
   restricted columns, so visibility derives from surfaces: an entry is
   visible through S iff it touches a column visible in S. Kernel provides
   `filter(log, surface)`; chain-preserving redacted entries (via §2.10's
   encoding) cover filtered history export, specified but not day-one.
10. ~~Reading lens default.~~ **Resolved (§2.9): default is `pinned`;
    changeable after records exist** — the lens is pure interpretation, so
    the switch is byte-free and reversible, but reportable via the impact
    machinery. Checkpoints pin their own reading revision and are immune.
11. ~~Snapshot retention policy.~~ **Resolved by §2.10.** Whole-record
    erasure is the guaranteed baseline (per-record chains, no global anchor,
    refcounted blobs); erasure-tolerant canonical encoding is a
    `strata-core` invariant from day one. Resolver snapshots fall under the
    same blob refcounting, so "keep the payload forever for re-mapping" is
    bounded by the referencing records' own retention. Residual choice —
    crypto-shredding vs externalized values for intra-record horizons —
    deferred to corpus data (§12.11).

## 11. Prior art to consult

**Directly liftable**
- **Avro** reader/writer schema resolution — literally the projection algorithm,
  specified rigorously
- **Buf** — breaking-change detection and compatibility policy for Protobuf
- **REDCap** — flat export with repeat discriminator columns; schema + records +
  versioning + audit at scale

**Merge semantics**
- **Dolt** (git on SQL tables, cell-level conflicts, clone/push/pull between
  instances), **TerminusDB** (immutable delta layers, branch/diff/merge/
  time-travel; stewardship moved to DFRNT in 2025), XTDB, Datomic, Automerge

**Domain peers**
- **Open Formulieren** (NL, Django, Common Ground — closest sibling),
  GOV.UK Forms (deliberately narrow), GC Forms (CA), FormSG (SG), OS2forms (DK)
- **Orbeon Forms** — most serious prior art on form modelling (XForms)
- **Frappe DocType** — uncomfortably close to the full original feature list
- form.io, SurveyJS, ODK/XLSForm, KoboToolbox

**Commercial incumbents in public-sector procurement** (the real competition):
Microsoft Dataverse + Power Apps, Salesforce OmniStudio, ServiceNow, Appian,
Pega, Unqork, Laserfiche, Tyler, Accela, Granicus.

## 12. Suggested first session in Claude Code

With access to open DN schema statistics:

1. Load and characterise the corpus — how many procedures, revisions per
   procedure, columns per procedure, records per procedure.
   *Partially done — see `corpus/M0-type-frequency.md` (42,723 published
   procedures, current revision only; revision history and records need
   other sources).*
2. ~~Type frequency report (M0).~~ **Done — `corpus/M0-type-frequency.md`.**
   Top 15 types cover 92.9% of 1.8M fields; 18.3% of descriptors are
   presentation-only; depth is empirically ≤1 with zero nested repetitions.
3. ~~Mapping of DN field types → proposed kernel type set.~~ **Done — same
   report.** Proposed scalar set + residue. Two residue items (enum-or-text
   `otherOption`, hierarchical LinkedDropDownList) resolved from
   institutional memory as **pre-conditional-logic artifacts**: both desugar
   to columns + surface visibility rules, so they cost the kernel nothing —
   but they are ~21k *implicit* conditional rules any M2 rule census must
   count. Since resolved: format constraints are surface admissibility over
   plain `text` (§2.6); geometry is a single-Feature scalar with arity
   `many` (FeatureCollection is a render shape); civilité is an enum block;
   record references are **on ice** (§6); referential-backed enums are
   **nomenclatures** (§2.12) — the residue is fully closed.
4. Enumerate historical revision transitions and hand-classify a sample into
   safe / lossy / breaking; check whether the proposed change-class table
   predicts them.
5. Extract the existing conditional/routing rules and assess expressibility
   under a pure, total, two-scope language.
6. **Resolver census** — how many procedures use externally-fed fields, against
   which sources, direct-match vs autocomplete, how many columns each mapping
   populates, and how often mappings changed. Drives open question 8.
7. **Deferred-resolution frequency** — how often were records submitted with
   unresolved lookups, and how long did they stay pending? Sizes the retry and
   abandonment machinery.
8. **Post-submission edit profile** — how many distinct actors touch a record
   after submission, how many entries per record, over what elapsed time, and
   how often do two actors touch the same cell? Validates per-cell LWW +
   detection (§2.9) and sizes log-vs-snapshot storage.
9. **Revision drift at edit time** — when an instructor edits, how far has the
   procedure moved from the submission revision? Question 10 is settled as
   `pinned` (§2.9); this now measures the *cost* of that default — how often
   instructors edit through an old lens, i.e. the real demand for explicit
   projection to newer revisions.
10. **Aggregate revision width and conflicts** — for the largest procedures,
    how many columns does the full-history aggregate contain versus the latest
    revision? How many columns hit a join conflict (incompatible retype, scope
    move, re-added ID)? If aggregates are 3× the latest revision's width, the
    empty-column suppression in §5.5 is load-bearing rather than cosmetic, and
    the conflict policies need to be right rather than nominal.
11. **Retention and erasure profile** — beyond whole-record retention expiry,
    has DN ever needed to erase *part* of a record's history while keeping the
    case (erasure requests against superseded values, third-party data in
    resolver payloads)? Decides whether intra-record erasure (§2.10) gets a
    mechanism at all, and if so which one — crypto-shredding vs externalized
    values.
12. **Prefill census** — how many procedures use prefill (§2.7), query-param
    vs API, how many columns per prefill, and how often prefilled values are
    overridden — and then restored. Sizes whether the trivial mapping
    suffices or API prefill needs declared typed mappings from day one.

Only after that: start `strata-core` and `strata-schema`.
