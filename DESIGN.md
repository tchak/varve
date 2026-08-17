# Varve — design document

> A varve is an annual layer of lake sediment: an append-only sequence of
> timestamped layers whose value is that history can be read back from
> them. Nothing published to crates.io yet.

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
  A checkpoint is a tag; the revision is the object. The revision DAG is
  the set of objects plus a **publication log** of events: publishing a
  schema whose object already exists (reverting to an earlier revision)
  creates no new revision — same content, same id — but is an event: it
  becomes `latest`, following the revisions it was published after.
  Identity is content; history is the log.
- **Column** — a typed field. Has an **arity**: `one | many`.
- **Group** — ordered container of columns. Has a **cardinality**: `one | many`.
- **Block** — a published, reusable group definition with its own identity and
  version, referenced by inclusion. Two halves along the tier boundary
  (settled 2026-08-17, was open question 5): the **schema-side block** —
  shell group + paired resolver declarations — lives in `varve-schema`,
  hashes plain like a nomenclature and travels as a `block` line; the
  **surface-side defaults** — prompts, visibility/required rules, formats,
  write policy over the block's own columns — live in `varve-surface` and
  reference the block by `(id, version)`. Inclusion **pastes with
  provenance**: the shell becomes an ordinary group of the revision
  carrying `included_from: (block, version)`, so nothing downstream learns
  about blocks and yet the revision knows what it included — rules pin to
  a block version, and the impact report can name a block bump.
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

- **Stored state** — `absent` (no stored state: never written, or
  `unset` since) | `empty` (a blank was written) | `value`. Intrinsic to
  the cell; this is what the wire transmits. **Settled (was open question
  13): what separates `empty` from `absent` is provenance, not logic.**
  An `empty` cell was *set* by an entry, so the fold carries its origin
  (§2.7 — which actor, entered or derived); an `absent` cell has no
  origin at all — `unset` removes provenance along with the value. "The
  applicant saw this field and left it blank" is a fact with an author;
  "nothing was ever written" is the absence of one. The logic language
  reads both as absence (§4.1: comparison atoms false, `is_empty` true);
  diff, audit and provenance are what tell them apart. **One state, one
  encoding**: an arity-`many` cell with no elements is `empty`, never a
  zero-length list, and a `many` group with no items has no item list —
  `apply` refuses to produce either, and conformance and the wire reader
  refuse them if built by hand.
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
value is non-admissible — never ill-typed. Custom patterns run on a
**linear-time engine** (author-supplied patterns meet user input, so
no-backtracking is a security property — no ReDoS), always full-match by
construction; backtracking-only constructs are publication errors, and DN
patterns using them surface as counted residue at import.

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
*restore* a re-derivation from `superseded.snapshot_ref`. **Cell provenance
is derived by the fold, not copied from entries**: a human `set` over a
derived cell yields `overridden { superseded }` whether or not the entry
said so, so the retention cannot be forgotten by an entry author. The one
case where `superseded` is transiently absent: overriding while a
resolution is still pending (§2.8) — the overriding entry cannot reference
a snapshot that does not yet exist. When the late derived write lands, the
fold fills `superseded` from it (rule 2 below), and the landed snapshot
also lives on the resolution instance, which sits beside the cells
precisely for this.

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
per-field casts, lossiness report. `varve-projection` already does exactly
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

Keep the raw response, content-addressed — a blob under its plain hash
(§2.13, §2.15) — with `(resolver, resolver_version, input_key,
fetched_at)` as the platform-side index over it.

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
   stays visible and re-derivable. *Enforced by the fold (2026-08-17)*: a
   **resolver's** derived write onto a human-authored cell (`entered` or
   `overridden`) is not applied; the cell keeps its value, gains the late
   derivation as `superseded`, and the refused write is reported in the fold
   result — never silent. The fold cannot see resolution state, so "while
   pending" is realised as "human-authored wins", which is the stronger
   reading and the one surfaces already imply (where humans may not write
   derived targets there is nothing to protect). Derived writes by *humans
   or the system* — restore, bulk re-map (rule 1) — are deliberate acts and
   apply: machine values never win by force; people may choose them. The
   payload lands on the resolution instance (`Resolution::land`) regardless
   of what the cells do — a resolution never resolves without its snapshot.

3. **Pending is readable by the logic language.** Resolution status is part of
   record state, so a surface can express "required unless pending". Keeps
   admissibility binary rather than adding a third value. Read **per group
   instance**, like cells: `pending(r)` in an item sees that item's and the
   record's pending resolutions, never a sibling item's (the §4.1 scope
   rule) — the evaluator's environment is keyed by `(scope, resolver)`.

All three are ratified. Rule 2 is the one the rest of the design leans on:
`overridden { superseded }` (§2.7) and prefill restore encode exactly its
asymmetry — machine values never win by force, and are always one explicit
entry away from restoration. Note the cost of rule 3: resolution status
becomes part of the logic language's typed input environment — still pure
and total (status is an input like cells), but `varve-logic`'s environment
must include the revision's resolver declarations.

### Checkpoint interaction

This partially re-opens locks (§6), cleanly:

> **A checkpoint freezes the cells of the surface it is taken through and
> enumerates the pending resolutions it expects to be filled.** Between the
> checkpoint and the one that supersedes it, a write into that frozen set is
> legal only if it is an expected derived write. Anything else is **reported
> as a violation**; writes outside the frozen set are not the checkpoint's
> business.

Defensible legal story — *this is what the applicant declared, these are the
référentiel lookups still outstanding* — without building general-purpose
locking.

**Settled (was open question 12): the freeze is surface-scoped, and the
kernel reports rather than gates.** An earlier wording ("freezes entered
cells … anything else is rejected") contradicted §2.9: a case file keeps
being appended to after submission — instructor annotations, third-party
columns, back-office corrections through their own surfaces. So the frozen
set is the columns (and the `many` groups holding them) **writable on the
surface the checkpoint was taken through** — the applicant form — the same
surface-relativity as reachability (§2.4) and admissibility (§2.6). DN
practice is exactly this shape: passing to instruction locks the applicant's
form while *annotations privées* stay writable, and "repasser en
construction" lifts it — a **superseding checkpoint**, which ends the
previous one's regime. Consequences: a human override of a derived cell
inside the frozen set (§2.7 back-office rule) after the checkpoint is a
*reported* violation — a real legal act the platform must take a new
checkpoint for, not something the kernel silently permits or forbids; and
`validate_after_checkpoint(log, checkpoint, superseded_by)` stays a pure
function that append never consults — the kernel has no permission model
(§2.9), Tier 5 decides what a violation means. The kernel cannot see
surfaces, so the platform fills the frozen set from the surface
(`varve-surface` exposes the writable column and group sets).

### Wire format

New line kinds: `resolver` (declarations, part of the schema — carried
inside the `revision` line's schema), `snapshot` (payloads, part of the
record's meaning), `resolution` (instances with status). The latter two
are not yet emitted — §10 question 14.

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
predecessor's hash, and the genesis (`prev` at seq 0) **commits to the
record's id** (settled 2026-08-17, open question 16): a log verifies only
under the record it belongs to and cannot be transplanted under another —
still per-record, nothing global (§2.10). `base_version` is the **log
version the writer read** before computing its ops — the number of entries
then present, i.e. the seq its entry would get if nothing intervened; two
entries touching one cell from the same `base_version` are the §2.9
conflict. It cannot serve as the chain link: concurrent entries
deliberately share a base (that is how conflict detection works), so it is
not a linear pointer. `prev` and `seq` are assigned together, at append
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
"Visible in S" here means the surface's **static column set**
(`Surface::columns()` — the columns S presents at all), not derived
reachability (§2.4): entry visibility must not flip with record state.
The function lives in `varve-surface` (which may depend on `varve-record`;
nothing depends on `varve-surface`, §7) and waits on the redacted-entry
representation (§2.13 decision 4, open question 14) — specified, not day
one.

Chain interaction: omitting entries from a filtered history export would
break `prev` verification. A filtered export therefore uses **redacted
entries** — envelope transmitted (`seq`, `prev`, content hash), content
withheld — verifiable because canonical hashing is already erasure-tolerant
(§2.10). Visibility filtering and erasure ride the same mechanism. Specified
now, needed later: applicant-facing exports are folded state, and operator
migration carries full history, so nothing requires redacted entries on day
one of `varve-wire`.

### Checkpoints, now precisely defined

> A checkpoint is a named entry hash in the log — the hash, not the seq, is
> what pins content — plus a reading revision, plus the set of pending
> resolutions expected to land after it, plus the **frozen set**: the columns
> and `many` groups writable on the surface it was taken through (§2.8).
> A later checkpoint supersedes it; its regime runs from its entry to the
> superseding one's.

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

### Hash the ciphertext, never the plaintext — a `varve-core` invariant

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
  exact record count. **The binding is meaning** (settled 2026-08-17, the
  §2.14 unit precedent): lifting an inline enum out into a published
  nomenclature whose ids contain it is widening; dropping the binding
  (published → inline) or switching it (published A → B) is **lossy** even
  when every id survives — the values keep their bytes and lose their
  référentiel. In the aggregate join, an inline enum bounded by a
  published one lifts out to it; two different nomenclatures meet only at
  `Text`.
- Lives in `varve-schema`, beside blocks.

**Name.** Chosen for domain authenticity: INSEE publishes *nomenclatures*
(COG, NAF, PCS) — the kernel uses the word of the référentiels world it
models. Rejected: `codelist` (accurate SDMX term, colder), `codebook`
(survey-practice kin but documents variables — broader), `vocabulary`
(implies words and RDF semantics), `valueset` (misuses the FHIR precedent —
a ValueSet is a selection from a code system), `taxonomy` (promises
hierarchy), `registry` (promises mutability and a central authority).

## 2.13 Canonical bytes and content addresses

Settled design for `varve-core::canonical`. Eight decisions:

1. **Two hashing regimes — and blobs.** Schema-side objects (revisions,
   blocks, nomenclatures, resolver declarations) hash **plain**: no
   salts, so identical schemas converge on identical ids on every
   instance — the point of content addressing. Record-side **entries**
   carry personal data and hash as **salted commitments** (§2.10): never
   plaintext. Salting schema hashes would silently destroy cross-instance
   schema identity; not salting entry hashes would make low-entropy
   fields brute-forceable after erasure. **Blobs — attachments and
   resolver payload snapshots alike — hash plain** (§2.15): dedup across
   records is the blob store's purpose (§2.10, "the same INSEE payload
   for one SIRET"), and salting would give every fetch of identical bytes
   a distinct address. The two regimes compose: a blob's plain address is
   referenced from *inside* an entry's salted content (`snapshot_ref` in
   a `derived` origin, attachment elements in cells), so the chain never
   discloses which blob a record holds, while the store dedups. *(An
   earlier wording listed snapshots among the salted objects,
   contradicting §2.10 and §2.15; corrected 2026-08-17.)*
2. **Canonical form is JCS (RFC 8785)** over wire-shaped JSON values. One
   encoding family for wire and hashing; hash the canonical bytes, never
   the emitted line (§5).
3. **Scalar rendering pinned.** Exact numbers are **strings**: integers
   as their decimal digits, decimals as the normalized `Decimal` string
   form — a JSON number is an IEEE double under JCS, so it cannot carry
   a full i64, and a verifier in any other language would round it.
   JSON numbers in canonical form are reserved for structural counts,
   bounded to the JCS-safe range (|n| ≤ 2^53 − 1; larger is a
   serialization *error*, never a rounding — the §2.14 no-silent-rounding
   precedent) and for geometry. Instants: normalized UTC RFC 3339.
   Geometry: the Feature embedded as a JSON **value** — never a
   stringified blob — with every number a double rendered per ES6
   (`1.0` → `1`, `-0.0` → `0`; implemented and tested against known
   vectors); feature equality is equality of that canonical form. Text:
   hashed as entered — no Unicode normalization. Absent vs `null`: the
   §5 rule, identically. *(Corrected 2026-08-17, before any record hash
   existed: geometry had been committed as a stringified serde rendering
   and integers as full-range JSON numbers.)*
4. **Entries are vector commitments over ops.** The entry's content hash
   is a plain hash of a body in which each op appears as its **salted
   per-op commitment** `H(salt_i ‖ canonical(op_i))`. A §2.9-filtered
   export can disclose some ops (with their salts) and withhold others
   (commitment only) while the entry hash — and the chain — verify
   unchanged. Redaction and erasure are the same mechanism at different
   lifetimes.
5. **Salts are inputs.** Tier 0 has no randomness: 32-byte salts are
   generated at Tier 5 append time and passed in, like timestamps. One
   fresh salt per op. Erasure granularity above per-op (epochs, key
   hierarchies) stays open per §12.11 — the encoding does not preclude
   it.
6. **SHA-256, with an algorithm tag.** Chosen for audit defensibility:
   the chain's legal weight starts with a primitive on the lists auditors
   already accept (ANSSI, FIPS, eIDAS-adjacent), plus ecosystem interop
   (attachments, blob stores, registries) and hardware speed at entry
   sizes. Every content address carries an algorithm tag so migration
   stays representable. BLAKE3 stays available to Tier 5 blob internals;
   kernel addresses are SHA-256.
7. **Revision identity.** Identity-bearing: types, arity, cardinality,
   the order of columns and groups (containers are ordered), inline
   nomenclature rows including labels (a relabel is a new revision,
   §2.11), resolver declarations, and a group's block provenance
   (`included_from` — the same structure typed by hand is a different
   revision, as an inline enum differs from a published one). Not
   identity-bearing: surfaces (separate objects), including block
   defaults, which are surface fragments hashed plain on their own.
   Schema-side blocks hash plain like nomenclatures. Canonical shapes
   (field names, optionals omitted when absent) live in code with test
   vectors.
8. **Envelope vs content; actor pseudonymity is a contract.** Envelope —
   survives redaction, lives as long as the record: `seq`, `prev`, actor
   (opaque id + kind), timestamp, `authored_against_revision`,
   `base_version` (structural — concurrency detection needs it on
   redacted entries too), content commitment. Content — redactable and erasable: ops, origin (it
   describes the values), note. Stated deliberately: *who acted when*
   shares the record's lifetime; *what they wrote* may have a shorter
   one. This is GDPR-sound only under a kernel contract: **actor ids are
   pseudonymous references**, and the id→person mapping lives
   platform-side, separately erasable — deleting the mapping anonymizes
   every envelope at once without touching a hash. A platform that
   writes direct identifiers into actor ids has broken the contract, and
   only whole-record erasure recovers it.

## 2.14 Units on numbers (settled from standing demand)

A long-wanted DN feature: an **optional unit** on integer/decimal
columns, with casting across compatible units. Plain numbers stay
plain.

**Units are a closed kernel set with exact rational ratios** — the cast
table is kernel semantics, so ratios cannot be user data. The pragmatic
v1 list, by dimension:

| dimension | units (exact ratios) |
|---|---|
| length | mm, cm, m, km |
| mass | g, kg, t |
| duration (exact) | minute, hour, day, week (60 / 24 / 7) |
| duration (calendar) | month, year (12) |
| area | m², ha, km² |
| volume | L, m³ (1000) |
| percent | % (own dimension, no conversions) |

- **Calendar time is deliberately its own dimension**: days ↔ months has
  no exact ratio, and the kernel refuses the conversion rather than
  invent a 30. Candidates pending corpus/demand: kWh, kW (energy
  renovation forms). Extension path as ever: closed set now, additions
  are new table rows, never user-defined ratios.
- **Casts** (§3 rows): same dimension → **exact-or-fail on the target
  representation** (rational conversion; 1500 m → 1.5 km fails into an
  integer column, succeeds into decimal — the no-silent-rounding
  precedent). Cross-dimension → forbidden. **Unit added** to an existing
  column → values unchanged, widening (the values were always implicitly
  in that unit) but never identity — **reported** by the impact report as
  a semantic change. **Unit removed** → values unchanged, **lossy**: the
  meaning is dropped, and every cell counts in the lossiness report.
  *(Corrected 2026-08-17 — both directions were "free". That made the
  widening relation non-transitive: `day → none → week` composed two free
  casts into the unit swap the direct cast refuses, and §5.5's "join is
  the least upper bound" was false. The asymmetry restores the partial
  order.)*
- **Logic** (§4.1): constants carry units; the typechecker requires
  dimension match; comparisons across compatible units are computed on
  **exact rationals**, never through decimal rounding — so they are
  total and exact even where a storage cast would fail (100 min vs
  hours compares exactly; it just can't be *stored* as a finite decimal
  of hours).
- **Duration is not a scalar either — the demand decomposes.** The
  value model is number-with-duration-unit; the "duration picker" is a
  **surface widget** over it, free to accept mixed granularity *within
  one dimension* (2 h 30 min → 150 min; 3 weeks 2 days → 23 days —
  exact normalization) and refused across dimensions, same rule as
  casts. A dedicated duration type's distinctive power is mixing
  components (`P1M15D`) — and that is exactly the poison: mixed
  calendar/exact durations are not totally ordered (jiff refuses to
  compare such spans without an anchor date), which would break the
  logic language's total exact comparison and smuggle the days↔months
  fiction back in through the value side. Future date arithmetic in
  computed values (`date + 6 months`) takes a number-with-unit operand
  via calendar arithmetic — well-defined even though month lengths
  vary — still no duration scalar. Jiff's ISO 8601 duration support
  exists if a wire rendering is ever wanted.
- **Currency is not a unit — agreed, and on principle**: unit ratios
  are facts of *definition* (timeless, exact, kernel-pure); exchange
  rates are facts of *the world* (dated, sourced) — and the kernel
  never fetches (§2.7), so currency conversion structurally cannot be
  a cast. Money, if demanded, is a **separate scalar**: exact decimal +
  currency code (nomenclature-backed), per-currency scale rules, and no
  cross-currency casts ever.

## 2.15 Attachments and blobs (settles open question 3)

An attachment cell element is: **element id** (§2.4 value-internal
identity, minted at Tier 5 like salts and timestamps), **content hash**
(a §2.13 `ContentHash` — algorithm-tagged), **filename** (what the
applicant called it — user data, stored as entered), **content type**
and **byte size** (verifiable claims about the blob). Nothing else: all
further metadata lives platform-side, keyed by hash.

- **Blobs hash plain — a deliberate, stated exception** to the
  record-side salted regime (§2.13): dedup across records is the blob
  store's design goal (the same certificate uploaded twice, the same
  INSEE payload for one SIRET — §2.10), and salting would destroy it.
  The §2.10 residual (a retained hash after blob-only erasure) is
  accepted, bounded by the record's own retention.
- **One blob machinery, two clients**: attachments and resolver
  payloads (§2.7 "design the two together") share the store, the
  address format, and the GC roots.
- **The kernel contributes roots, not sweeping**: a pure function
  `referenced_blobs(record) → set of ContentHash` (attachment cells +
  snapshot refs in origins). Tier 5 (`varve-files`: blob trait —
  get/put/has + sweep given roots) does mark-and-sweep. Same shape as
  `pending_resolutions`: pure enumeration in, scheduler out. This is
  §2.10's refcounting requirement, made concrete.
- **Wire**: the `attachment` line describes the *blob* (hash, size,
  type); filenames stay in cells — two records naming one blob
  differently is correct, not a conflict. The manifest declares
  `referenced` or `bundled`; bundled means a **sidecar archive keyed by
  hash** beside the JSONL stream (settled: streams stay small and
  text, blobs stay binary). A single self-contained file with chunked
  base64 blob lines was considered and rejected — ~33% overhead and
  chunk-assembly machinery for a need DN does not have.
- **Scan lifecycle is kernel-modeled, mirroring resolutions (§2.8)**:
  per attachment element, `pending → clean | infected | failed`, driven
  by a Tier 5 scanner; the kernel provides the pure pending-enumeration.
  Settled so that surfaces can express "submittable only when scanned"
  without every platform reimplementing the gate; the corresponding
  logic atoms (paired, like `pending`/`not_pending`) join §4.1 when
  the surface work needs them.
- **Schema-level restrictions — settled**: `Attachment` carries
  constraints the way numbers carry units: an **accept set of media-type
  patterns** (`application/pdf`, `image/*` — IANA's vocabulary, not any
  administration's; empty = unrestricted, so plain attachments stay
  plain) and a **per-file `max_bytes`**. These are *representability*
  (a "photo du bien" column **is** an image column — uploading a PDF
  through another surface would break its meaning, not merely its
  admissibility), unlike text formats, which are presentation checks
  and stay surface-side (§2.6). Casts mirror the §2.11 enum rules:
  broadening the accept set or raising the limit is free; narrowing is
  **checked**, with the impact report counting the records whose files
  violate. The kernel checks the cell's *claims* (content type, size)
  with zero IO; the Tier 5 store verifies claims against bytes at
  ingest. DN's "natures" (RIB, titre d'identité…) are **document
  kinds**, not container formats — they become **published blocks**
  (constrained attachment + label + help), the SIRET/civilité
  resolution again: the kernel stays international, the French
  administration ships as content.
- **Not kernel**: previews and thumbnails, URL signing, storage
  tiering, retention schedules (platform policy over kernel erasure
  ops), MIME sniffing and semantic document validation ("is this
  really a RIB").

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
| unit changed within a dimension | checked — exact-or-fail on the target representation (§2.14) |
| block included / removed / **bumped to another version** | the block's columns follow the rows above; the impact report groups them under one named block change (§2.1) |
| unit added | free — values unchanged, reported as a semantic change (§2.14) |
| unit removed | lossy — values unchanged, meaning dropped (§2.14) |
| unit dimension changed | breaking |
| attachment accept set broadened / size limit raised | free (§2.15) |
| attachment accept set narrowed / size limit lowered | checked — impact report counts records whose files violate |

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

Fuzzers and a property-test corpus from day one: `proptest` suites in
every crate that has laws to state, and cargo-fuzz targets under `fuzz/`
(wire reader totality and write∘read fixpoint, logic canonical decode,
GeoJSON parsing, entry decode) — see README.

### 4.1 First design pass, from the DN implementation

Read from DN's `Logic::` module (`ChampColumnValue` current;
`ChampValue` legacy, ignored), its visibility evaluation, routing engine
and ineligibility rules. The extracted expression corpus (§12.5) must
validate this pass; until then it is grounded in the implementation, not
the usage.

**What DN's language actually is — smaller than assumed.** A condition
is `and`/`or` over **atomic comparisons**. Terms are column references,
constants, and empty. No arithmetic, no predicate nesting inside terms,
and **no logical negation**: negatives are separate operators (`NotEq`,
`Exclude`, `NotIn*`). Only ~13 champ types are conditionable — text and
dates cannot participate at all — and integer/decimal unify into one
`number` comparison type.

**Absence always loses — and negation is not negation.** DN evaluates a
hidden, blank, or missing source to nil, and every operator collapses
nil to **false** — including the negative operators: `NotEq(hidden, x)`
is *false*, not true. So DN's negatives are not `Not(Eq)`; they are
independent atoms that also lose on absence. Adopted as kernel
semantics, deliberately:

> Atoms are two-valued. A source that is absent, empty, or unreachable
> makes the atom **false** (except `is_empty`, which it makes true).
> There is **no general `Not` combinator** — negated comparisons are
> their own atoms with the same absence-loses rule. This keeps
> admissibility binary (§2.8 rule 3), makes evaluation total, and
> imports DN rules without changing their meaning.

Adopted on merit, not heritage — examined against the alternatives:

- **Not an approximation.** With `is_empty`/`is_filled` first-class,
  any function over `{absent} ∪ values` is expressible — "visible
  unless explicitly no" is `or(is_empty(x), not_eq(x, "no"))`. The
  semantics is a *default* plus explicit absence tests, not a
  restriction; it is exactly as expressive as three-valued logic.
- **The rival is the famous footgun.** SQL-style three-valued logic
  (unknown propagation) is the most notorious surprise source in
  databases; importing it would hand those bugs to form authors.
  "A comparison is true only when the field is filled with a matching
  value" is one sentence an author can hold.
- **Progressive disclosure falls out**: on a pristine record every
  comparison atom is false, so only unconditioned content shows.
- **The `unreachable → absent` leg is forced**, not chosen: hidden
  never deletes (§2.4), so hidden columns retain stale values — any
  semantics that let conditions read them would have ghost values
  driving visibility.
- **The one real footgun** — `not_eq(x, "no")` written expecting it to
  match unanswered forms — gets a **lint**, not a semantics change:
  §4.3's solver detects conditions false on pristine records and
  suggests the `or(is_empty(x), …)` form. Add to the §12.5 checklist:
  has this pattern generated author confusion in DN support history?

**The AST (v1):**

```
Expr  ::= and(Expr…) | or(Expr…) | Atom
Atom  ::= eq | not_eq | lt | le | gt | ge     (typed comparison)
        | is_empty | is_filled
        | contains | excludes                  (arity-many enum)
        | pending | not_pending (resolver)     (§2.8 rule 3; paired
                                                negative — "required
                                                unless pending" needs it,
                                                found building surfaces)
Term  ::= column(column_id, field?)            (field: nomenclature
                                                extra-field projection)
        | const(typed literal)
```

`Expr` nests arbitrarily — **settled from institutional memory**: DN's
single-level and/or is a UI limitation with standing demand against it,
not a semantic choice. The kernel ships full nesting; DN rules import
as the degenerate one-level case. Empty combinators are the identities:
`and()` is *true*, `or()` is *false* — `and()` is how a surface spells
"always required", `or()` "never"; both are legal, neither is an error.

**DN's geo operators dissolve.** `InDepartement(commune_col, "01")` is a
field projection through the commune nomenclature's extra fields
(§2.12) followed by `eq`: `eq(column(c, "departement"), "01")`. Four
special operators become zero; the mechanism already existed.

**Scopes, confirmed exactly.** DN's `champs_for_condition`: an
item-scoped rule reads its own item's columns plus record columns; a
record-scoped rule reads record columns only; repetition children
inherit hiddenness from their group's own visibility. That is §4's
two-scope model verbatim. Record-scoped rules can never reference
item-scoped columns (DN's evaluation would silently pick the first row;
the kernel typechecker rejects it instead).

**Acyclicity is the kernel rule; "upstream only" is a surface rule.**
DN's editor restricts condition sources to champs *above* the current
one in document order — a presentation-order constraint that guarantees
acyclicity. The kernel keeps only the invariant: the dependency graph
(column → its visibility rule's sources) must be **acyclic**, checked
at publication like the depth policy (§2.3 style: an error with a
message, not a type). Evaluation is topological; a hidden source reads
as absent, so visibility cascades deterministically. Surfaces may
impose the stronger upstream-only authoring rule; the kernel does not
care about document order.

**Attachment points, all sharing one AST:**

- **Visibility / requiredness** — per surface node (§2.6).
- **Ineligibility** — a record-scoped admissibility predicate on the
  submission surface (+ message): DN blocks submission when it holds.
  Nothing new in the kernel — it is a surface admissibility rule.
- **Routing** — an ordered list of record-scoped predicates,
  first-match wins (DN: per instructor group, with duplicate-rule
  detection). The kernel evaluates predicates; what a route target *is*
  stays platform-side (§6).

**Typing — the conditionability matrix (settled from demand).** Atoms
typecheck against a revision through the existing machinery; per scalar
type, v1 allows:

| type | atoms |
|---|---|
| boolean, number (int/decimal via widening; unit-aware on exact rationals, §2.14), **date, datetime** | full comparisons |
| enum | `eq`/`not_eq` on option ids (statically checked, §2.12), field projections |
| enum arity `many` | `contains`/`excludes` |
| **text** | **presence only** (`is_empty`/`is_filled`) |
| **attachment, geometry** | **presence only** |

Date/datetime comparisons are **in** — DN lacks them and the demand is
standing. Text *comparisons* are **out of v1** — free-text equality is
too easy to author into an impossible condition (case, whitespace,
accents make exact match near-meaningless); presence checks carry none
of that risk and stay. Attachment/geometry presence checks go beyond
DN's conditionable set — also standing demand. All three restrictions
are **publication-time atom policy, not grammar** (the same relaxation
path as column-to-column comparisons): text comparisons, if ever
enabled, arrive with normalized matching, not by lifting the policy
alone.

**Static analysis → `varve-impact`.** A rule breaks when: a source
column is removed; a source is retyped so the atom no longer
typechecks; an enum constant references a **removed option id** (the
§2.11/§3 flagged case, now reaching rules); a projected nomenclature
field disappears. These mirror DN's own rule-error taxonomy
(`not_available`, `incompatible`, `not_included`) and become the
"broken rule references" section of the impact report — built:
`varve_impact::broken_rules` re-typechecks caller-supplied rules
against the new revision and classifies the verdicts, telling a rule
the transition broke from one that was already broken.

### 4.2 Computed values (in scope — strong standing demand)

DN's logic ships predicates only, but the demand for computed values is
real and strong (institutional memory) — the canonical case being a
total over a repetition ("sum of the amounts"). Design sketch:

- **A computed column is a schema object**: declared expression, typed
  target — the resolver-declaration pattern with the record itself as
  the source. Statically typechecked at publication like a mapping
  (§2.7).
- **Virtual, never stored.** A computed cell is a *view*, like group
  values (§2.5): derived at read, never in the log, never in wire
  cells, recomputed on import. No staleness, no provenance ambiguity —
  its origin is its declaration.
- **Value expressions** extend the AST with typed operations: numeric
  arithmetic (`+ − ×` on number; division needs a total-semantics
  decision — absent from v1 until settled), text concatenation, and
  conditional selection (`if(Expr, ValueExpr, ValueExpr)`).
- **Value expressions need their own absence rule — and it is not
  "loses".** Predicates collapse absence to false; values must not
  silently collapse it to 0 or "". Proposed: scalar operations
  *propagate* absence (`a + b` with `b` absent is absent), aggregates
  *skip* absent items (`sum` over a repetition with one unfilled amount
  sums the filled ones, `count` counts items, `count_filled` counts
  values). Decide before implementation; silent-zero is exactly the
  approximate behavior to refuse.
- **Aggregates cross scopes upward, and only upward**: `count(group)`,
  `sum/min/max(item-scoped column)` yield record-scoped values — the
  "sum of amounts" case. Bounded by the item list, so totality is
  preserved; no aggregate may appear item-scoped over its own scope.
- **Acyclicity extends** to computed columns: the publication-time
  dependency check covers rule sources *and* computed inputs in one
  graph. Computed columns are readable by predicates (a visibility rule
  over a computed total is expected usage).

### 4.3 Satisfiability and the visibility space (standing demands)

Two more recurring demands, both served by one small solver, both
tractable because of a structural gift: **atoms constrain one column
against constants** (never column-to-column), so satisfiability is
boolean structure over atoms whose only interactions are per-column.
Each column gets a small abstract domain — `{absent/empty}` ∪ intervals
(numbers, dates) or option-id sets (enums) — and the absence-loses rule
is an asset: absence makes every comparison atom on a column false at
once, and `is_empty` true. No SMT dependency; property-test the checker
against brute-force enumeration over small domains.

**Absurdity detection** (beyond typechecking), the findings taxonomy:

- **Never-true** — `and(a == 2, a == 3)`: a dead visibility rule (the
  column can never appear), a dead routing branch, an ineligibility
  that never fires.
- **Always-true**, evaluated over the absence element too —
  `or(a == true, a == false)` is *not* a tautology, absence falsifies
  both: a pointless condition, or an ineligibility that blocks every
  submission.
- **Redundant conjunct/disjunct** — `a > 3` inside `and(a > 5, …)`:
  simplification hints.
- **Routing shadowing** — an earlier group's rule subsumes a later
  one's; first-match makes the later group unreachable. DN today
  detects only exact duplicates.
- **Never-true visibility × required-on-surface** = the "statically
  unreachable required columns" §7 promises the impact report —
  this is the algorithm behind that promise, and the promise waits on
  it (open question 15).

**The presentation-tree graph is statically computable.** The visible
set is a function of the finite atom valuation, propagated through the
(acyclic) rule DAG: the space of presentation trees is finite,
enumerable, and pruned by the same satisfiability check (no branch for
an impossible state). Honest caveat: the global count is a **product
over connected components** of the dependency graph — exponential in
the worst case. The product structure is also the display answer:
conditions cluster into small components (one cascade per topic);
enumerate each component's reachable states, render one decision tree
per cluster, never one tree of 2^n leaves. Whether real procedures stay
small per-component is measurable from the §12.5 extraction — cascade
depth and component size join the checklist below.

Placement: the solver and enumeration live in `varve-logic`; consumed
by `varve-impact` (dead rules, unreachable required columns, routing
shadowing) and by platform tooling for display.

**Extension path: column-to-column comparisons.** The AST already
represents them (`lt(column(end), column(start))` — atoms compare two
Terms); the constant-side requirement is a **publication-time policy**,
not a grammar rule — the depth-1 pattern again. Enabling them later
changes no stored rule and no wire shape. Solver cost, when it comes:
conjunctions of order/equality atoms between columns stay polynomial
(constraint graph, strict-cycle check, union-find); boolean structure
on top is handled by enumerating atom polarities with the polytime
check per candidate — lazy-SMT-in-miniature, still no external solver.
The genuine cost lands on visibility-space enumeration: column-column
atoms **fuse connected components**, so the per-component budget clause
stops being theoretical. Likely rollout, from the shape of the demand
(`date_end ≥ date_start`): enabled for validation predicates first —
where admissibility barely exercises the solver — before visibility.

**Deferred:** a general `Not` combinator — rejected, not postponed (it
would silently change imported rule semantics; see the absence-loses
rule). **To validate against the extracted corpus (§12.5) and feature-
request history:** operator frequencies; whether any rule depends on
the negative-operator-on-absence subtlety; the ~21k desugared implicit
rules (otherOption, linked dropdowns) counted in; a census of the
computed-value demands — which operations and aggregates were actually
asked for fixes the v1 operation set from evidence rather than taste;
and condition-graph shape — cascade depth and connected-component size
— which decides whether §4.3's per-component enumeration is always
cheap in practice.

## 5. Wire format

**Tagged JSONL.** The point is not just streamability — it's that the stream can
be **heterogeneous**, carrying schema, records, items, attachments and history
in one file in dependency order.

```
{"k":"header", ...}                 // format ver, source instance, mode, intent, manifest (revision ids, record count, attachments mode)
{"k":"revision", "id":"...", "schema":{...}}   // writer schema travels with the data (Avro property); resolver declarations ride inside it
{"k":"block", "id":"...", "version":1, "group":{...}, "resolvers":[...]}   // schema-side block (§2.1); travels like a nomenclature. Its surface defaults travel with surfaces (§10 Q14)
{"k":"nomenclature", "id":"...", "version":1, "rows":[...]}   // versioned (id, label, ...fields) table (§2.12); travels like a block
{"k":"record", "id":"...", "lens":"...", "cells":{...}}   // snapshot mode: ROOT cells; lens = fold revision, not a record property (§2.9)
{"k":"item", "record":"...", "group":"...", "parent":[...], "id":"...", "ord":0, "cells":{...}}   // one item's cells; follows its record line
{"k":"entry", "record":"...", "seq":0, "prev":"...", "ops":[...], ...}  // history mode: one log entry (§2.9)
{"k":"attachment", "hash":"sha256:...", "byte_size":..., "content_type":"..."}   // describes a blob (§2.15); algorithm-tagged (§2.13)
```

Not yet on the wire — **open question 14**: `resolution` instances,
`checkpoint`s, payload `snapshot` descriptions and the bundled blob
sidecar. Until then a history export is lossless for the *log*.

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

### Import modes (settles open question 6)

Modes are distinguished by **stream kind, never by flag** — the two line
kinds cannot mix (above), so one format can never do both:

- **History import** — `entry` lines: migration. Verify and adopt each
  record's chain, continue appending (§6: one-way, one-time).
- **Snapshot import** — `record`/`item` cell lines: **whole-record
  replace**. A stream is authoritative for the full state of each record
  it contains; within a record, absent means unset. Finer bulk updates
  ("these three columns on 10,000 records") use the op form — column-
  scoped replace declarations are not needed (confirmed from DN
  practice) and are cut, not deferred.
- **No in-band unchanged-sentinel, ever.** "Unchanged" is absence (op
  form) or not-in-stream (snapshot form). CSV is non-importable (above),
  so the tabular input class that needs sentinels never reaches the
  kernel. REDCap's `NEW` sentinel is likewise unnecessary: `add_item`
  ops always carry explicit ids, minted by the Tier 5 importer.
- **Import is never a side door.** A snapshot import into a live record
  reduces to `diff(current state, imported state)` appended as an
  **ordinary log entry** — actor supplied by the importer,
  `base_version` = current. LWW, conflict detection, provenance,
  checkpoint enforcement and the audit trail apply to bulk imports
  exactly as to human edits, with zero import-specific machinery.
- **The manifest declares record-level intent**: `create-only`,
  `update-only`, or `upsert`. Unknown ids under update-only and
  colliding ids under create-only reject the stream on line 1 semantics
  (§5 constraints) — a typo'd record id fails loudly instead of
  silently creating a duplicate case file.

### Stored state on the wire

- key absent from `cells` → absent
- key present, `null` → empty
- key present, value → value: a **scalar object** (`{"text":"…"}`,
  `{"integer":"42"}`, `{"option":"…"}`, `{"geometry":{…}}`, …) for arity
  `one`, an **array of scalar objects** for arity `many`

The same encoding serves the `state` of a `set` op — one serializer for
cells and ops, so the two cannot drift. One state, one encoding (§2.4): a
blank `many` cell is `null`, never `[]`; an item list is never empty. The
reader refuses both.

Reachability is never serialized — always derived on read from surface +
revision (§2.4). Stored values are transmitted even when unreachable on every
current surface: "hidden never deletes" implies hidden must round-trip.

### Constraints

- **Lines must be bounded.** One-record-per-line breaks at 5,000 items. Hence
  `record` (root cells) and `item` lines are separate. **Settled
  (2026-08-17): the contiguity rule, no terminator.** A record's `item`
  lines immediately follow its `record` line, parents before children
  (`parent` names the root or an item already seen for that record), in
  list order (`ord` is the position and is checked in sequence); any other
  line kind, or the end of the stream, closes the record. A stream names a
  record once — a second `record` line for the same id is malformed. Depth
  1 today; `parent` keeps the grammar depth-N ready (§2.3).
- **Line 1 must carry everything needed to fail fast**: format version,
  source instance ID, mode, intent, manifest of revision IDs, record count,
  whether attachments are bundled or referenced. (Schemas travel as
  `revision` lines, so group ids are not repeated in the manifest; the
  format version is the compatibility contract, so no separate kernel
  version.) Import rejects on line 1 or commits to the whole stream. Apply
  into staging, then atomic swap — never stream into live tables.
- **Canonical serialization required** for content-addressed checkpoints.
  Settled: JCS (RFC 8785) — the full design is §2.13; hash the canonical
  bytes, never the emitted line.
- **No exact numbers in JSON numbers.** Strings for integers and exact
  decimals (JCS numbers are doubles — §2.13 decision 3), RFC 3339 for
  instants; JSON numbers carry only JCS-safe structural counts and
  geometry coordinates.

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
cast table (§2.x, now in `varve-schema`).

Conflict cases needing declared policy:

| conflict | policy options |
|---|---|
| retyped incompatibly (no join exists) | widen to opaque text / split / omit |
| removed then re-added with a different type, same ID | split by revision range |
| **scope moved** (root ↔ `many` group) | must split — cannot share a header |
| cardinality changed one ↔ many | widen to `many` where possible |

Every aggregation emits an **AggregateReport** listing which columns hit which
policy. Same shape as the impact report.

> **Implementation notes (`varve-schema::cast`, found while building it).**
> (1) `Text` is the top of the widening order for every text-renderable
> scalar, so most retype pairs *have* a join; a join that lands on `Text`
> with neither input `Text` (e.g. integer ∨ date, two different units of
> one dimension, inline vs published enums) is tagged **`ViaText`** and
> must appear in the AggregateReport — that is the "widen to opaque text"
> policy applied where it is the genuine LUB, reported instead of silent.
> The tag is a per-step report; the aggregate ORs it over its fold, since
> the *type* is associative but the path is not. Two inline enums join
> row-wise by id — a shared id with two labels is a rename (§2.11), so
> the later label wins, never `Text`. True `Incompatible` is reserved for
> attachment/geometry against anything else: split or omit, never coerce.
> The lattice laws (idempotent, commutative, associative, upper bound,
> **least**) are property-tested; leastness is what forced the §2.14 unit
> asymmetry.
> (2) Same-nomenclature enum joins take the higher version on the §2.11
> assumption that nomenclature versions are **append-only** (removal is
> deprecation; ids are never deleted). That assumption is now load-bearing
> and must hold in `varve-revision`'s nomenclature publication rules.
> (3) A cast is a set of orthogonal properties (lossy / checked /
> needs-lens), not one class: `decimal-many → integer-one` is lossy *and*
> checked at once.

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

- type **join / LUB** → `varve-schema` (dual of the cast table)
- `aggregate(revision_dag) -> Revision + AggregateReport` → `varve-revision`
- consumed by `varve-projection`

## 6. Deliberately cut or deferred

| item | reason |
|---|---|
| record fork / merge / rebase (branches) | 10× cost multiplier for a use case that can't yet be named. Real stories (draft vs submitted, prefill-from-record, agent-proposed corrections, post-decision correction) are all served by **revisions + proposed-changes**. |
| general-purpose record locks | the real need is covered by checkpoints, since defined (§2.8–2.9): freeze the checkpointed surface's cells, enumerate expected late writes, report the rest. Locking beyond that still needs a user story before a design. |
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
- `varve-core` — IDs (column, group, record, item, revision), row path, scalar
  primitives (exact decimal, RFC 3339 instants), canonical serialization,
  content hashing. **Canonical encoding is erasure-tolerant (§2.10): hashes
  commit to salted or encrypted value encodings, never plaintext.** Depends on
  nothing.

**Tier 1**
- `varve-schema` — types, arity, groups, cardinality, schema-side blocks
  (§2.1: shell + declarations, `Block::include_into` pastes with
  provenance), nomenclatures (§2.12), structural constraints, depth
  policy. **Includes the cast table** — the compatibility
  relation between two types is a property of the type system itself — **and its
  dual, the type join / least upper bound** used to build aggregate revisions
  (§5.5). Canonical hash → revision ID. **The `Revision` object itself — an
  immutable, hashable schema snapshot — lives here in Tier 1**; `varve-revision`
  (Tier 3) owns only the DAG, publication and merge. That is what lets
  `varve-logic` (Tier 2) type-check against a revision without a Tier 3
  dependency.
- `varve-value` — cells, items, typed conformance, structural diff and patch.
  Pure and stateless. *(Narrowed: the record log moved out to
  `varve-record`.)*

**Tier 2**
- `varve-logic` — expression AST (with its canonical JSON form — the wire
  and hash shape; there is no textual syntax and none is intended, rules
  are authored structurally and a syntax would be a tool), type checker
  against a revision, total evaluator, dependency-graph acyclicity.
- `varve-projection` — records viewed and edited through a revision they
  weren't written on. Casts applied, lossiness reported.
- `varve-impact` — what does publishing revision N+1 do? Change classification
  (safe / lossy / breaking), the §2.8 resolver questions, broken rule
  references (§4.1), count of records whose cells fail the new cast, records
  with pending resolutions against a removed resolver. **Statically
  unreachable required columns** need the §4.3 solver — not built (open
  question 15). *(Name settled — see open question 2.)*

`projection` and `impact` both depend on `schema`.
*Correction, found in implementation: `impact` also depends on
`projection` — counting the records whose cells fail a transition **is**
running the projection over them, and duplicating the per-cell cast
execution would drift — and on `logic`, since "broken rule references"
**is** re-typechecking rules against the new revision. Both are Tier 2;
the DAG stays acyclic; the reverse directions remain absent. Rules and
per-record pending resolutions are inputs to `impact` — surfaces (Tier 3)
and resolution instances (Tier 3) hand them down; the crate never looks
up.*

**Tier 3**
- `varve-surface` — presentation + admissibility tree, and the surface-side
  block defaults (`BlockDefaults`, referencing a schema-side block by
  version). Depends on schema + logic. **Nothing depends on it** — that's
  the proof that "form isn't core" — which is why a block is two objects,
  not one.
- `varve-revision` — revision DAG, publication, block and nomenclature
  publication (registries: version numbering, validation), three-way
  schema merge, **aggregate revision construction (§5.5)**.
- `varve-record` — the log (§2.9): entries, fold, snapshots, checkpoints,
  concurrency detection, resolution instances (§2.8). Depends on `value` +
  `schema`. Still deterministic — no clock, no IO; timestamps are inputs.

**Tier 4**
- `varve-wire` — tagged JSONL. Reader, writer, header/manifest, patch ops,
  apply.

**Tier 5 — IO appears here for the first time**
- `varve-store` (traits, async), `varve-store-postgres`, `varve-files`
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
  `tools/m0` over `varve-core` + `varve-schema`. `varve-value` exists
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
  *Machinery built: `varve-projection` and `varve-impact` implement
  classification (§3 table), the §2.8 resolver questions (removed →
  orphaned columns and records pending; result type changed → broken
  mappings; mapping changed → stale and orphaned columns; input and
  version changes), the §4.1 broken-rule section (rules re-typechecked
  against the new revision, classified as source removed / retyped /
  option removed / field removed, with "already broken" told apart), and
  record assessment (failed cells per column; records whose cells have
  no cast at all under a breaking change; pending resolutions against
  removed resolvers), tested on synthetic transitions. Not built:
  statically unreachable required columns — the §4.3 solver, open
  question 15. The falsification itself awaits the DN revision-history
  extract (§12.4).*

- **M2** (`logic`) — **Rule expressibility.** Can every existing conditional and
  routing rule be expressed and type-checked? The residue is either a needed
  language feature or complexity to refuse to carry forward.
  *Predicate core implemented (`varve-logic`): the §4.1 AST with
  policy-rejected column-to-column operands, typechecker
  (conditionability matrix, scope prefix rule, enum membership, unit
  dimensions), total evaluator with absence-loses semantics and exact
  rational unit comparison, `sources()`, and the publication-time
  acyclicity check. Awaiting the §12.5 extraction for falsification;
  §4.2 computed values and the §4.3 solver are not yet implemented.*

- **M3** (`wire`) — **Round-trip.** The corpus in and out, byte-stable.
  *Machinery built (`varve-wire`): tagged JSONL where every line is the
  JCS canonical bytes (one serializer — byte-stability is a property of
  the canonical form); both export modes, the line-1 manifest with
  intent, fail-fast reading, mode-mixing rejection, history import as
  chain adoption (tamper detected on import), snapshot import as an
  ordinary log entry. Round-trip and reader-totality are property-tested.
  **Corpus run passed** (`corpus/M3-round-trip.md`): all 42,723 schemas
  emitted, read back and re-emitted byte-identically, every revision id
  recomputed from the decoded schema — and content-addressing revealed
  19.7% of procedures are structurally identical to another. Record-side
  corpus round trip awaits DN record data.*

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

1. ~~Name.~~ **Resolved: `varve`** (2026-08-16; `varve` and the whole
   `varve-*` family were free on crates.io — verify again before M3
   publish, plus a trademark search). A varve is an annual sediment
   layer used to reconstruct the past layer by layer — the kernel's
   subject, in one word, and pronounceable identically in French and
   English. The original "strata" was taken on crates.io. Also
   considered and rejected: `chartrier` (deep case-file resonance, free
   GitHub org, but hard for English speakers), `cadastre` (names a real
   government institution), `strate` (reads as a typo), `terrane`
   (misreads as terrain), `greffe`/`feuillet` (fail internationally);
   `palimpsest`, `cartulary`, `lamina`, `tephra`, `accrete`, `loess`,
   `moraine` (taken on crates.io). Earlier: rejected the `-DB` suffix —
   invites comparison to Dolt/TerminusDB on axes that are lost while
   hiding the actual differentiator.
2. ~~`varve-impact` name.~~ **Resolved: `impact` confirmed.** Three
   grounds: "change impact analysis" is the discipline's own term for
   exactly this activity; *étude d'impact* is a formal French
   administrative instrument, so the name is domain-authentic to its
   actual audience (the nomenclature/varve argument again); and the
   crate is named after its artifact — DESIGN.md calls the flagship
   deliverable "the impact report" throughout, and renaming the crate
   away from its own output would be a permanent seam. Considered and
   rejected: `preflight` (best challenger — right tense, but connotes a
   boolean checklist and undersells the report), `assay` (precise but
   obscure), `triage` (implies emergency and chosen neglect),
   `forecast`/`prognosis` (imply estimation; the counts are exact),
   `audit` (retrospective), `fallout`/`blast-radius` (negative-only —
   an all-Safe verdict is a common, happy outcome), `tremor` (cute).
   Earlier rejections hold: `resolve`, `transit`, `morphism`, `compat`
   (legacy-shim connotation), `migrate` (one-way, destructive),
   `evolve` (forward-only).
3. ~~Attachments / files.~~ **Resolved (§2.15).** Cell element =
   id + tagged content hash + filename/type/size; blobs hash plain (a
   deliberate exception — dedup is the goal); one blob machinery shared
   with resolver payloads; kernel contributes `referenced_blobs` roots,
   Tier 5 sweeps; wire bundles as a sidecar archive keyed by hash
   (single-file base64 rejected); scan lifecycle kernel-modeled like
   resolutions so admissibility can gate on it.
4. ~~Depth-1 demand.~~ **Resolved from DN experience.** The corpus cannot
   answer this — DN never supported nested repetition, so no demand signal
   exists in the data. The few requests over the years were refused without
   much pushback. Depth-1 stands as policy; `row_path` staying a sequence
   (§2.3) is the entire accommodation.
5. ~~Group-level atomic validation.~~ **Resolved by the assembled
   design; placement and pinning settled 2026-08-17.** A published block
   guarantees its **structure** (columns, types, units, constraints,
   inline nomenclatures — §2.1, versioned and content-addressed like
   nomenclatures), its **paired declarations** (a resolver, per the §2.7
   SIRET example), and its **bundled defaults** (§4: visibility and
   requiredness rules, prompts, formats, write policy over its own
   columns). Validation predicates — `date_fin ≥ date_debut`, the §4.3
   column-to-column comparison — arrive with §4.3 and blocks will carry
   them then; an earlier wording promised them now, which neither
   surfaces nor the publication policy support. Casting a block between
   versions is the per-column cast machinery over the block's columns
   together (§2.5: a block's value is a view). Violating a block rule
   produces **non-admissibility with respect to a surface** (§2.6), never
   global invalidity: block rules are surface-level rules the block *ships
   as defaults*, the way a "RIB" block ships an accept set. Hence a block
   is **two objects along the tier boundary**: the schema-side `Block`
   (`varve-schema`, hashed plain, `block` wire line, published through a
   registry) and `BlockDefaults` (`varve-surface`, referencing the block
   by `(id, version)`, validated against it, travelling with surfaces —
   Q14). An earlier implementation put both halves in `varve-surface`,
   which the wire could not carry without breaking "nothing depends on
   `varve-surface`" (§7); moving surface types down a tier to make one
   wire object was considered and rejected — it would erode "form isn't
   core" to satisfy a line kind. **Rules pin to the block version because
   inclusion pastes with provenance**: `Group.included_from` records
   `(block, version)`, identity-bearing (§2.13 decision 7); the alternative
   — resolving block references through a table wherever a schema is
   read, the published-nomenclature pattern — was rejected for its blast
   radius. That provenance is what makes an impact report over block
   bumps meaningful (§3 row; `varve-impact` names included / removed /
   bumped / detached / attached).
6. ~~Import modes.~~ **Resolved (§5 "Import modes").** Modes are
   distinguished by stream kind, never by flag; snapshot import is
   whole-record replace (column-scoped replace cut — DN practice does
   not need it; the op form covers partial bulk updates); no in-band
   sentinels ever (CSV is non-importable, so the input class needing
   them never reaches the kernel); imports land as ordinary log entries
   — never a side door; the manifest declares create-only / update-only
   / upsert so id mismatches fail loudly.
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
    `varve-core` invariant from day one. Resolver snapshots fall under the
    same blob refcounting, so "keep the payload forever for re-mapping" is
    bounded by the referencing records' own retention. Residual choice —
    crypto-shredding vs externalized values for intra-record horizons —
    deferred to corpus data (§12.11).
12. ~~Checkpoint scope.~~ **Resolved (§2.8, 2026-08-17): surface-scoped
    freeze; kernel reports, platform enforces.** Found by audit: §2.8's
    "freezes entered cells … anything else is rejected" and the code that
    implemented it literally (every non-resolver write after a checkpoint
    flagged) contradicted §2.9's multi-actor case file and §2.7's
    back-office override. Three candidates were weighed — freeze the
    surface's writable set; freeze the cells present at the checkpoint
    regardless of surface; pin only, no freeze — and settled by design
    argument (freeze what the checkpointed surface could write, the same
    surface-relativity as reachability and admissibility) plus DN practice
    (instruction locks the applicant form, annotations stay open, "back to
    construction" is a superseding checkpoint). Reporting rather than
    gating follows from "no permission model": `validate_after_checkpoint`
    is pure; append never consults it.
13. ~~Absent vs empty.~~ **Resolved (§2.4, 2026-08-17): the distinction
    is provenance.** Found by audit: §2.4 defined `absent` as "never
    written" although `unset` returns a cell to absent, and never said
    what `empty` adds beyond `absent` given that logic reads both as
    absence — while `Many([])` and empty item lists were further blank
    encodings with distinct canonical bytes. Settled by design argument
    from existing machinery: the fold keeps an origin for a `set` blank
    and drops it on `unset`, so `empty` is a blank with an author and
    `absent` is the lack of one; the extra encodings are refused (apply,
    conformance, wire reader) so each state has one canonical form.
    Collapsing to two states was considered and rejected — it would
    erase the "left blank by whom" fact that audit and diff rely on.
14. **Record-side (and surface) wire completeness.** Not yet on the wire
    (found by audit, 2026-08-17): `resolution` instances (§2.8 lifecycle,
    landed snapshot ref), `checkpoint`s (§2.9: entry hash, reading
    revision, expected resolutions, frozen set), payload `snapshot`
    descriptions (hash/size/type, like `attachment`), the bundled blob
    **sidecar** (§2.15), and **surfaces** — including block defaults,
    which travel with them — so today a history export is lossless for
    the log only, a procedure's surfaces do not migrate, and
    "an imported record remains fully meaningful on an instance with no
    access to INSEE" (§2.8) is a goal, not a property. Deliberately
    routed here rather than built piecemeal: payload blobs and attachment
    blobs share one sidecar, and `resolution`/`checkpoint` lines should
    land with it so import restores a record whole. Decide with §12.7
    (deferred-resolution frequency) in hand.
15. **The §4.3 solver — absurdity detection and statically unreachable
    required columns.** Promised by §7 for the impact report and by
    §4.3 as the algorithm behind it; not built (found by audit,
    2026-08-17 — earlier wording read as if it were). Everything else
    the impact report promises is implemented; this one needs the small
    per-column abstract-domain solver over the acyclic rule DAG. Build
    it after the §12.5 rule extraction shows cascade depth and component
    size — the same data that decides whether per-component enumeration
    is always cheap.
16. ~~Does a record's chain commit to its record id?~~ **Resolved
    (§2.9, 2026-08-17): yes — genesis = H("varve:genesis:" ‖ record id).**
    Found by audit: with a global genesis a stored log could be
    transplanted under another record's id and verify. Settled by design
    argument: tamper-evidence is the chain's purpose, binding costs one
    string in the genesis, stays per-record (no global commitment, §2.10
    intact), and the only thing it constrains — re-minting record ids on
    migration — §5 already forbids (adoption keeps ids). Considered and
    rejected: leaving it to the store (the row a log lives in), which
    would make the chain's meaning depend on storage.

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

Only after that: start `varve-core` and `varve-schema`.
