# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repository is

**Varve**: a generic, embeddable Rust kernel for versioned, multi-party
case files, extracted from ~10 years of building DN, a French
public-service platform (administrations publish "procedures"/schemas,
citizens submit "dossiers"/records). The thesis: the valuable extraction
is the **versioning kernel** (schema revisions over live records, records
as long-lived case files, a pure typed logic language, impact reports),
not the form builder. (A varve is an annual sediment layer — history read
back layer by layer.)

`DESIGN.md` is the design document and single source of truth. The
workspace holds every crate below the IO tier (`varve-core`, `-schema`,
`-value`, `-logic`, `-projection`, `-impact`, `-record`, `-surface`,
`-revision`, `-wire`) plus `tools/m0`, the corpus harness; `corpus/`
holds the analyses (M0: all 42,723 published DN procedures express with
zero residue; M3: they round-trip byte-stably through the wire). M1 and
M2 machinery is built and awaits DN-internal data (revision history,
rule extraction). Run `cargo test --workspace` (262 tests incl. property
suites) and `cargo clippy --workspace --all-targets` before committing.

## Version control: jj, not git

This is a colocated jj (Jujutsu) repository; `.git` exists for tool interop.
Use `jj` commands: `jj commit -m`, `jj describe`, `jj log`, `jj diff`,
`jj git push`. The working copy is always a mutable change; `jj commit`
finalizes it and opens a new one.

## Conventions in DESIGN.md

- **Open questions are never deleted.** A resolved §10 question keeps its
  number, gets struck through (`~~...~~`) with a bold **Resolved** note and a
  pointer to the section that settles it. Numbering is stable — other
  sections cross-reference it.
- Decisions settled inside a section are marked in place: **Settled (was open
  question N)** or **RATIFIED**.
- Record *how* a question was resolved: corpus data, institutional memory
  (e.g. "DN never supported this, requests were refused"), or design
  argument. The corpus can only show demand for features DN already had.
- Every new feature must route its unknowns to §10 (open questions) or §12
  (corpus questions). The explicitly managed risk is second-system syndrome:
  features earn their place by appearing in the real DN corpus.
- Cross-references use § numbers; keep them valid when inserting sections.

## Pre-publish stage: no backward compatibility

Nothing is published (`publish = false` everywhere, gated on M3). Until
then there is no API stability contract and no backward-compatibility
concern between crates: **refactor ruthlessly**. Move types to their right
crate, rename freely, and update all callers in the same change. Never add
re-exports, aliases, or deprecation shims "for compatibility" — there are
no external consumers to be compatible with, and shims at this stage only
obscure where things live.

## Design invariants already decided (do not re-litigate casually)

From DESIGN.md — these constrain any code written here:

- Strict crate DAG (§7): Tiers 0–4 are deterministic — no IO, no async, no
  clock (timestamps are inputs). IO first appears in Tier 5.
- `#![forbid(unsafe_code)]`; `serde` on wire types only; all crates `0.x`;
  nothing published until M3 (corpus round-trip).
- **Hidden never deletes**; reachability is derived and surface-relative,
  never stored (§2.4).
- `required` is a surface property, not a schema property (§2.6).
- The kernel never fetches (§2.7) and has no permission model (§2.9);
  authorization reduces to surface assignment.
- Per-record hash chains with **no global commitment over record logs**, and
  canonical hashing commits to salted/encrypted value encodings, never
  plaintext — both are erasure guarantees (§2.10) and cannot be retrofitted.
- Records never branch; migration between instances is one-way and one-time
  (§5, §6).
- Milestones are corpus-first (§8): M0 expressibility → M1 falsification →
  M2 logic → M3 wire round-trip; only then surface, store, service. The
  first deliverable is an oracle over the DN corpus, not a library.
