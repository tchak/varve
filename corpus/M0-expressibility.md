# M0 expressibility run

Output of `cargo run --release -p m0` over the data.gouv.fr snapshot
`20260815041004-demarches.json` (see `M0-type-frequency.md` for dataset
provenance). Exit criterion (§8 M0): every procedure expressible; the
residue, and why, is the type-system requirements document.

**Result: 42,723 / 42,723 procedures express and validate. Residue: none.**

```
procedures:                42723
schemas valid:             42723
columns emitted:         1639948
groups emitted:            71503
resolver declarations:     38253
surface-only dropped:     329879

desugarings:
  otherOption → enum + text:   14763
  linked dropdown → enums:      6486
  dossier link → text:          2746
  pre-rempli → surface r/o:        1

warnings:
  empty enums:                   469
  linked orphan secondaries:       0

validation errors: none

residue: NONE — every procedure is expressible
```

Notes:

- Counts cross-check against the Python analysis in
  `M0-type-frequency.md`: surface-only 329,879, otherOption 14,763,
  linked 6,486, dossier links 2,746, resolver-fed 38,253 — all exact.
- Zero `--primary--` parsing anomalies across all 6,486 linked dropdowns.
- 469 dropdowns declare zero options (authoring sloppiness; expressible as
  a degenerate empty enum, flagged as a warning, not an error).
- Zero depth-policy violations, independently confirming resolved Q4.
- `PreRempliChampDescriptor` (1 occurrence) resolved from institutional
  memory: a read-only champ filled only through external data — a plain
  column whose read-only-ness is a surface write policy (§2.9) and whose
  filling is prefill (§2.7). No type needed.
- Resolver mappings are synthesized from DN's fixed per-champ-type
  semantics (institutional knowledge) — the public dataset does not carry
  them.
