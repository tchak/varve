# M0 type frequency report — DN published procedures

First corpus deliverable (§8 M0, §12.1–12.3 of DESIGN.md).

**Source:** data.gouv.fr, "Descriptif des démarches publiées sur
demarche.numerique.gouv.fr", snapshot `20260815041004-demarches.json.gz`
(124 MB gz / 535 MB JSON). Published procedures only, **current revision
only**, structure only — **no conditional logic, no historical revisions, no
records** are in this dataset. §12.4–12.10 need other sources.

## Headline numbers

- **42,723 procedures**, all with ≥1 dossier, **18.9 M dossiers** total
- **1,804,144 champ descriptors**, 36 distinct types
- Fields per procedure: median 33, p90 84, p99 167, max 648
- **Maximum nesting depth is 1. Zero repetitions inside repetitions** —
  empirical confirmation of resolved Q4 (depth-1 policy)
- 30.1% of procedures use at least one repetition; children per repetition:
  median 2, p90 8, max 185
- Coverage: top 5 types = 60.7% of occurrences, top 10 = 81.2%,
  **top 15 = 92.9%**, top 20 = 98.3% — DESIGN.md's "a handful of types
  cover ~90%" prediction (§8 M0) holds

## Kernel-type buckets

| bucket | occurrences | % |
|---|---:|---:|
| text (incl. textarea, formatted) | 474,069 | 26.3 |
| **presentation-only (header sections, explications)** | **329,879** | **18.3** |
| attachment (arity `many`) | 259,783 | 14.4 |
| enum (dropdown, civilité, linked) | 161,510 | 9.0 |
| boolean (yes-no, checkbox) | 156,286 | 8.7 |
| number (integer, decimal, legacy) | 119,981 | 6.7 |
| format-constrained text (phone, email, iban) | 98,592 | 5.5 |
| date / datetime | 54,566 | 3.0 |
| enum-set (multi-dropdown, arity `many`) | 47,092 | 2.6 |
| resolver-fed (address, siret, rna, rnf, annuaire, référentiel, cojo) | 38,253 | 2.1 |
| group `many` (repetition) | 33,250 | 1.8 |
| admin referential enums (pays, région, département, commune, epci) | 26,237 | 1.5 |
| record reference (dossier link) | 2,746 | 0.2 |
| geometry (carte) | 1,899 | 0.1 |
| other (pré-rempli) | 1 | 0.0 |

## Full type table

`%occ` = share of all descriptors; `%procs` = procedures using the type;
`doss-wt%` = share weighted by the procedure's dossier count (≈ share of
cell instances in production); `%req` = required=true ratio; `inRep` =
occurrences inside a repetition.

| type | occ | %occ | %procs | doss-wt% | %req | inRep |
|---|---:|---:|---:|---:|---:|---:|
| Text | 349,627 | 19.4 | 94.2 | 17.6 | 65 | 35,113 |
| PieceJustificative | 259,783 | 14.4 | 81.7 | 15.1 | 51 | 27,210 |
| HeaderSection | 225,686 | 12.5 | 85.6 | 13.7 | 0 | 1,524 |
| DropDownList | 137,863 | 7.6 | 74.8 | 8.8 | 82 | 12,515 |
| Textarea | 121,538 | 6.7 | 50.9 | 3.0 | 60 | 8,420 |
| Explication | 104,193 | 5.8 | 60.2 | 7.6 | 0 | 2,218 |
| YesNo | 93,070 | 5.2 | 51.2 | 5.7 | 83 | 4,499 |
| Checkbox | 63,216 | 3.5 | 53.1 | 3.6 | 73 | 1,486 |
| IntegerNumber | 57,008 | 3.2 | 30.5 | 2.1 | 72 | 6,854 |
| Date | 52,996 | 2.9 | 54.3 | 5.8 | 70 | 5,150 |
| Phone | 51,222 | 2.8 | 68.7 | 2.3 | 69 | 1,869 |
| MultipleDropDownList | 47,092 | 2.6 | 41.9 | 2.2 | 70 | 3,108 |
| Email | 45,435 | 2.5 | 67.2 | 2.0 | 81 | 1,842 |
| Number (legacy) | 33,754 | 1.9 | 16.2 | 0.9 | 66 | 1,509 |
| Repetition | 33,250 | 1.8 | 30.1 | 1.8 | 46 | 0 |
| Address | 30,607 | 1.7 | 45.3 | 2.1 | 75 | 1,813 |
| DecimalNumber | 29,219 | 1.6 | 17.3 | 1.0 | 68 | 3,803 |
| Civilite | 17,161 | 1.0 | 27.5 | 0.9 | 80 | 1,203 |
| Commune | 11,081 | 0.6 | 16.6 | 0.7 | 73 | 1,297 |
| Pays | 9,068 | 0.5 | 11.1 | 1.2 | 80 | 1,397 |
| LinkedDropDownList | 6,486 | 0.4 | 9.3 | 0.4 | 71 | 712 |
| Siret | 5,730 | 0.3 | 11.1 | 0.3 | 65 | 220 |
| Departement | 5,327 | 0.3 | 9.9 | 0.6 | 79 | 329 |
| Formatted | 2,904 | 0.2 | 2.7 | 0.4 | 77 | 210 |
| DossierLink | 2,746 | 0.2 | 5.3 | 0.1 | 54 | 59 |
| Iban | 1,935 | 0.1 | 4.4 | 0.1 | 88 | 34 |
| Carte | 1,899 | 0.1 | 4.0 | 0.0 | 33 | 44 |
| Datetime | 1,570 | 0.1 | 1.8 | 0.1 | 80 | 128 |
| AnnuaireEducation | 1,502 | 0.1 | 3.1 | 0.1 | 70 | 102 |
| Region | 633 | 0.0 | 1.3 | 0.0 | 76 | 34 |
| RNA | 289 | 0.0 | 0.7 | 0.0 | 48 | 19 |
| Epci | 128 | 0.0 | 0.3 | 0.0 | 81 | 23 |
| RNF | 76 | 0.0 | 0.2 | 0.0 | 87 | 9 |
| Referentiel | 44 | 0.0 | 0.1 | 0.0 | 77 | 11 |
| COJO | 5 | 0.0 | 0.0 | 0.0 | 100 | 0 |
| PreRempli | 1 | 0.0 | 0.0 | 0.0 | 0 | 0 |

## What the data confirms in the design

- **Schema/surface split (§2.6).** 18.3% of all "fields" (header sections +
  explications) carry no data — they are surface nodes, not columns. Nearly
  a fifth of DN's schema payload is presentation.
- **Depth-1 (§2.3, resolved Q4).** Max depth 1, zero nested repetitions,
  across 42,723 procedures.
- **Attachments as list-valued column (§2.2).** Attachments are the #2 field
  type (14.4%), and 27,210 of them sit *inside* repetitions — the
  "multi-file inside a repetition block stays at depth 1" case is common,
  not hypothetical.
- **Arity `many` on columns (§2.2).** MultipleDropDownList: 47k occurrences,
  41.9% of procedures. The two-multiplicities model matches real usage.
- **`required` is surface-appropriate (§2.6).** Required ratios vary 33–88%
  by type with no structural pattern — requiredness is policy, not typing.
- **Resolver pluggability (open Q8) — first real signal.**
  `ReferentielChampDescriptor` (44 uses) is DN's *generic pluggable
  connector*, and `COJO` (5 uses) is a dead one-off (Paris 2024 Olympics).
  DN itself needed both a plugin mechanism and one-off integrations, on top
  of the shared référentiels (SIRET 11.1% of procedures, Address 45.3%,
  RNA, RNF, AnnuaireEducation). Leans strongly toward *pluggable from day
  one*, plus a story for retiring dead resolvers.

## Residue — decisions the type table forces (§12.3)

1. ~~Enum-or-text sum.~~ **Resolved (institutional memory): pre-conditional-
   logic artifact.** `otherOption` predates conditional logic in DN. It
   desugars to an enum column plus a text column under a surface visibility
   rule (`visible iff enum = "Autre"`). No sum type in the kernel; ship the
   desugaring as a published-block pattern. **M2 note:** these 14,763
   constructs are *implicit* conditional rules — any rule census undercounts
   unless it includes them.
2. ~~Format-constrained text: types or predicates?~~ **Resolved: neither
   types nor schema predicates — surface constraints.** The schema type is
   plain `text`; the format check (phone/email/iban/regex) is admissibility
   and lives in the surface (§2.6). Casts stay trivial (text is text),
   strictness may differ per surface, and a mis-formatted value makes a
   record non-admissible with respect to a surface — never globally
   invalid.
3. ~~Referential-backed enums.~~ **Resolved: nomenclatures (§2.12 of
   DESIGN.md).** Pays/région/département/commune/epci (1.5%) are closed
   external codelists — and the enum-size tail (p99 = 367 options, max =
   **47,738**) shows administrations stuffing referentials into hand-made
   dropdowns. A **nomenclature** is a versioned, content-addressed
   `(id, label, …fields)` table, published standalone or inline (inline
   options are the degenerate case); every enum is nomenclature-backed. It
   is simultaneously the static-source fourth flavour of the resolver
   mechanism — fully portable (travels like a block), synchronous, and
   statically typecheckable where API resolvers cannot be.
4. ~~Hierarchical enum.~~ **Resolved (institutional memory): same story.**
   LinkedDropDownList predates conditional logic. It desugars to a primary
   enum plus per-primary-value secondary enums under visibility rules. No
   dependent-enum type, and no new logic-language feature — surface
   visibility already covers it. Same M2 undercount note as residue 1
   (6,486 more implicit rules).
5. ~~Record reference.~~ **On ice (institutional memory).** Many DN uses of
   DossierLink are better served by **multiple surfaces over one schema** —
   the same case file seen by different actors — rather than by records
   pointing at records. No `record_ref` scalar until demand is proven from
   record-side data. Until then a legacy DossierLink imports as `text`
   holding an opaque id: nothing is lost, and promoting it later to a real
   type is a checked cast — the reverse (retiring a type) is the expensive
   direction.
6. ~~Geometry.~~ **Resolved: first-class scalar = one GeoJSON Feature;
   multiplicity via arity `many`.** Mirrors attachments exactly: a
   list-valued column whose element IDs (GeoJSON's native feature `id`)
   provide value-internal identity for fine-grained diff ("feature 2
   moved") without paying for a scope. A FeatureCollection is not a kernel
   value — it is the render/export aggregation of a many-arity geometry
   cell. DN's Carte maps to `geometry`, arity `many`.
7. ~~Civilité.~~ **Resolved: enum via a published block; no kernel type.**

## Proposed kernel scalar set (from this data)

`text`, `boolean`, `integer`, `decimal`, `date`, `datetime`, `enum`
(declared options, versioned codelist-backed variant per residue 3),
`attachment`, `geometry` (one GeoJSON Feature; arity `many` for feature
sets). Format constraints (phone/email/iban/regex) are surface
admissibility over `text`, not types (residue 2); record references are on
ice (residue 5). Everything else in the corpus is: a surface node, a
group, arity `many` on one of the above, or a §2.7 resolver-fed composite
mapping *into* these scalars.

## Reproduction

Dataset snapshot in scratchpad (not committed — 535 MB). Analysis: Python
stdlib only; scripts inline in session history. Re-fetch via the data.gouv
API (dataset id: `descriptif-des-demarches-publiees-sur-demarche-numerique-gouv-fr`).
