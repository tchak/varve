# Varve

[![CI](https://github.com/tchak/varve/actions/workflows/ci.yml/badge.svg)](https://github.com/tchak/varve/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust 2024](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](Cargo.toml)
![Status: pre-publication, 0.1.0 placeholder](https://img.shields.io/badge/status-pre--publication-lightgrey.svg)

A generic, embeddable Rust kernel for **versioned, multi-party case
files**, extracted from ~10 years of building a large French
public-service platform (administrations publish *procedures* — schemas —
and citizens submit *dossiers* — records; millions of them).

> A *varve* is an annual layer of lake sediment: an append-only sequence
> of timestamped layers whose value is that history can be read back from
> them.

## The thesis

The valuable extraction is **not the form builder** — good open-source
form builders exist. It is the versioning kernel underneath:

- **schema revisions applied to live records** — hundreds of thousands of
  them, with a per-column compatibility relation (Avro-style
  reader/writer resolution) instead of freeze-or-corrupt;
- **records as long-lived case files** — appended to by many actors over
  time (applicant, instructors, third parties, external data sources),
  as an append-only, hash-chained, tamper-evident log;
- **provenance as a kernel concept** — every cell knows whether it was
  entered by a human, derived from an authoritative source (with the
  retained payload), or overridden — the *données déclaratives* vs
  *référentiel authentique* distinction, mechanized;
- **an impact report** shown to an administration *before* it publishes
  a revision: what breaks, what loses information, and exactly which
  records fail — the artifact no form platform offers.

`DESIGN.md` is the full design document and single source of truth,
including every decision's rationale and the open questions.

## Status

Pre-publish (`publish = false` everywhere; nothing on crates.io).

| milestone | state |
|---|---|
| **M0 — expressibility** | ✅ all 42,723 published DN procedures express and validate with **zero residue** (`corpus/M0-expressibility.md`) |
| **M1 — falsification** | machinery built (`varve-projection`, `varve-impact`); awaits historical revision data |
| **M2 — logic language** | predicate core built (`varve-logic`); awaits the rule corpus for falsification |
| **M3 — wire round-trip** | ✅ all 42,723 schemas round-trip byte-stably through `varve-wire` (`corpus/M3-round-trip.md`); record-side awaits DN data |

## Layout

- `crates/varve-core` — ids, row paths, scalar primitives, canonical
  bytes and content addresses (JCS, SHA-256, salted commitments)
- `crates/varve-schema` — types, groups, blocks (schema side),
  nomenclatures, resolver declarations, validation, the cast table and
  type join
- `crates/varve-value` — cells, typed conformance, structural diff/patch
- `crates/varve-logic` — the predicate language: AST and canonical form,
  typechecker, total evaluator, rule-graph acyclicity
- `crates/varve-projection` — records viewed through revisions they
  weren't written on; casts applied, lossiness reported
- `crates/varve-impact` — the impact report: change classification,
  resolver questions, broken rule references, record assessment
- `crates/varve-record` — the append-only record log: entries, fold,
  provenance, chain verification, snapshots, checkpoints, resolutions, scans
- `crates/varve-surface` — presentation and admissibility: reachability,
  requiredness, formats, write policy, block defaults
- `crates/varve-revision` — revision DAG and publication, block and
  nomenclature registries, three-way schema merge, aggregate revisions
- `crates/varve-wire` — tagged JSONL: writer, reader, history and
  snapshot import
- `tools/m0` — the corpus harness (oracle over the public DN dataset)
- `fuzz/` — cargo-fuzz targets (see below)
- `corpus/` — corpus analyses and results
- `DESIGN.md` — the design document

Everything below the storage tier is deterministic: no IO, no clock, no
async — timestamps and salts are inputs.

## Developing

Version control is [jj](https://github.com/jj-vcs/jj) (colocated git).

```sh
cargo test --workspace              # 277 tests, incl. property suites
cargo clippy --workspace --all-targets
cargo fmt --all --check
scripts/check-layering.sh           # DESIGN §13.5: no runtime/web/ORM crate below Tier 5
scripts/fetch-corpus.sh             # download the DN corpus (~124 MB gz)
cargo run --release -p m0           # M0 harness over the corpus
```

CI (`.github/workflows/ci.yml`) runs the first four plus `cargo doc`
with warnings denied and a fuzz regression pass on every push and PR.
A second workflow (`fuzz.yml`) fuzzes each target for ten minutes
weekly (or on demand, with a chosen duration), merges new coverage
into the seeds and opens a pull request with them.

Fuzz targets live in `fuzz/` (excluded from the workspace; needs
`cargo install cargo-fuzz` and a nightly toolchain):

```sh
cd fuzz
cargo +nightly fuzz run wire_read corpus/wire_read seeds/wire_read   # also: logic_canon, value_feature, record_entry
cargo +nightly fuzz run wire_read seeds/wire_read -- -runs=0         # regression only: each seed once
cargo +nightly fuzz run wire_read seeds/wire_read corpus/wire_read -- -merge=1   # fold new coverage into the seeds
```

`fuzz/seeds/<target>/` is the tracked regression corpus: a minimized
(`-merge=1`) set of inputs, each adding coverage, which CI replays with
`-runs=0` so a decoder change that starts rejecting, accepting or
crashing on a known input fails the build. The working corpora under
`fuzz/corpus/` (where a run writes what it finds) are gitignored; after
a local fuzzing session, or after a fix that changes what a decoder
accepts, merge them back into the seeds with the last command above —
the weekly workflow does the same and proposes the result as a PR.

The corpus is public data —
[Descriptif des démarches publiées](https://www.data.gouv.fr/datasets/descriptif-des-demarches-publiees-sur-demarche-numerique-gouv-fr)
(data.gouv.fr) — fetched into the gitignored `corpus/data/`. Snapshots
are dated; numbers in `corpus/*.md` come from the 2026-08-15 snapshot.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
