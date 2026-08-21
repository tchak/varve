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

`DESIGN.md` is the design document and single source of truth for the
kernel; `PLATFORM.md` (same conventions, P.x numbering) designs the
platform above it — the DN-successor web app and GraphQL API (§13
fixes the boundary; platform crates will live in `platform/`). The
workspace holds the deterministic kernel crates (`varve-core`,
`-schema`, `-value`, `-logic`, `-projection`, `-impact`, `-record`,
`-surface`, `-revision`, `-wire`), the Tier 5 crates
(`varve-files`: encrypted content-addressed blob store over
`object_store` backends; `varve-bundle`: the export-bundle joins;
`varve-store`: async persistence traits + in-memory reference impl),
and `tools/m0`, the corpus harness;
`corpus/` holds the analyses (M0: all 42,723 published DN procedures
express with zero residue; M3: they round-trip byte-stably through the
wire). M1 and M2 machinery is built and awaits DN-internal data
(revision history, rule extraction). Before committing run what CI
runs (`.github/workflows/ci.yml`): `cargo test --workspace` (307 tests
incl. property suites), `cargo clippy --workspace --all-targets`,
`cargo fmt --all --check` (rustfmt defaults are the style authority),
and `scripts/check-layering.sh` (the §13.5 guard — no runtime/web/ORM
crate in any Tier 0–4 closure, serde direct only in `-wire`/`-value`);
CI also denies rustdoc warnings and replays the tracked fuzz seeds
(`fuzz/seeds/`, minimized corpora; `fuzz.yml` fuzzes weekly and opens
a PR with new-coverage seeds — locally, the `-merge=1` command in
README).

## Platform test policy (platform/ crates)

Three levels; always use the **lowest level that can prove the
behavior**.

1. **Component tests** — `#[cfg(test)]` beside the component: plain
   `#[test]`, no runtime — but component futures resolve at view
   *build*, not render, so use the shared helper
   `components::testing::render` (a `CxTestBuilder` Cx + a noop-waker
   `block_on` driving the `view!` build, the pattern topcoat-view's
   own unit tests use). Only for *our*
   pure presentational components (props in → HTML out, no IO); they
   own our components' markup contracts — aria wiring, slots, class
   merging. Never test vendored registry components: `registry_sync`
   pins them byte-for-byte and their behavior is upstream's.
2. **Router-level tests** — `tests/app/`: `Router::handle`, no
   listener, no browser. They own route/status/redirect contracts,
   form handling, session mechanics as headers, page markup and
   strings, locale resolution from headers. Fast — the default home
   for new behavior.
3. **Browser e2e** — `tests/e2e/`: full journeys on every installed
   Playwright engine. They own only what a real browser proves —
   cookie acceptance, engine divergence, form encoding, redirect
   following. Keep them few and journey-shaped.

Both test dirs are one binary each: `main.rs` (doc + module list),
`harness.rs` (all shared machinery), one module per **subject**
(journeys/features: auth, i18n, shell, …) — never per page; a new
subject is a new module. A journey may exist at router *and* e2e level
only when e2e adds real-browser semantics; never re-assert details a
lower level already owns. DB-backed and browser tests gate on
`VARVE_TEST_DATABASE_URL` (+ installed engines) and skip vacuously.

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
concern between crates: **refactor ruthlessly**. Move types to their
right crate, rename freely, and update all callers in the same change.
Never add re-exports, aliases, or deprecation shims "for compatibility"
— there are no external consumers to be compatible with, and shims at
this stage only obscure where things live.

The repository being public on GitHub changes nothing here —
visibility is not publication. All crates carry `0.1.0`, a placeholder,
not a release; the stability contract begins only when a crate is
published to crates.io at a version above 0.1.0. Until that day this
rule holds in full.

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
