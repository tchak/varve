#!/usr/bin/env bash
# §13.5 layering guard: the §7 crate DAG direction, enforced mechanically.
#
# Two checks over the Tier 0–4 (kernel) crates:
#
#   1. The normal-dependency closure of every kernel crate contains no
#      runtime / ORM / web / storage crate (§9 "no async below Tier 5").
#      Tier 5 (`varve-files`, …) and `platform/` may pull in tokio and
#      friends; a kernel crate reaching one of them through any path —
#      even a "harmless" helper — is the layering bug this catches.
#
#   2. `serde` / `serde_json` appear as *direct* dependencies only where
#      §9 allows ("serde on wire types only"): `varve-wire` (the wire is
#      JSON) and `varve-value` (GeoJSON is JSON — the parser of a domain
#      format, not wire coupling; see its Cargo.toml). Checked on direct
#      deps because the closure of everything above `varve-value`
#      legitimately contains serde_json.
#
# Plain `cargo tree` with no extra tooling; exits non-zero on a violation.
set -euo pipefail
cd "$(dirname "$0")/.."

KERNEL=(varve-core varve-schema varve-value varve-logic varve-projection
        varve-impact varve-surface varve-revision varve-record varve-wire)

# Anything that is a runtime, an HTTP stack, a database layer, or a
# storage/IO backend. Substring match on crate names (so `tokio` also
# catches `tokio-util`, `sqlx` catches `sqlx-core`, …).
FORBIDDEN=(tokio async-std smol futures hyper axum tower reqwest http
           async-graphql toasty topcoat sqlx diesel sea-orm rusqlite
           object_store age tempfile rand getrandom
           platform) # `platform` catches every platform-* crate: the
                     # §7 DAG points platform -> kernel, never back.

SERDE_ALLOWED=(varve-wire varve-value)

fail=0

closure() { cargo tree -p "$1" -e normal --prefix none 2>/dev/null | awk '{print $1}' | sort -u; }
direct()  { cargo tree -p "$1" -e normal --prefix none --depth 1 2>/dev/null | awk '{print $1}' | sort -u; }

for crate in "${KERNEL[@]}"; do
  deps="$(closure "$crate")"
  for bad in "${FORBIDDEN[@]}"; do
    # Exact name or name with a `-`/`_` suffix: `http` must not match
    # `httparse`-style false friends only by accident — list both forms.
    hits="$(grep -E "^${bad}([-_].*)?$" <<<"$deps" || true)"
    if [[ -n "$hits" ]]; then
      echo "LAYERING: $crate (kernel) reaches forbidden dependency: $(tr '\n' ' ' <<<"$hits")" >&2
      cargo tree -p "$crate" -e normal -i "$(head -1 <<<"$hits")" >&2 || true
      fail=1
    fi
  done

  case " ${SERDE_ALLOWED[*]} " in *" $crate "*) continue ;; esac
  serde_hits="$(direct "$crate" | grep -E '^serde([-_].*)?$' || true)"
  if [[ -n "$serde_hits" ]]; then
    echo "LAYERING: $crate depends directly on $(tr '\n' ' ' <<<"$serde_hits")— §9: serde on wire types only" >&2
    fail=1
  fi
done

#   3. License gate: every crate here is MIT OR Apache-2.0 (§9), and
#      the kernel's embeddability depends on staying that way — no
#      copyleft license may enter a kernel-crate closure. A license
#      counts as copyleft only if *every* SPDX alternative is (an OR
#      with a permissive branch is fine: we take that branch). Only
#      kernel closures are checked; what platform/ may depend on is
#      its own affair.
kernel_closure="$(for c in "${KERNEL[@]}"; do closure "$c"; done | sort -u)"
license_hits="$(cargo metadata --format-version 1 2>/dev/null | python3 -c "
import json, sys
names = set(\"\"\"$kernel_closure\"\"\".split())
copyleft = ('GPL', 'SSPL', 'EUPL', 'OSL')
for pkg in json.load(sys.stdin)['packages']:
    if pkg['name'] not in names:
        continue
    lic = pkg.get('license') or ''
    alts = [a.strip() for chunk in lic.split(' OR ') for a in chunk.split('/')]
    if alts and all(any(c in a for c in copyleft) for a in alts):
        print(f\"{pkg['name']} {pkg['version']}: {lic}\")
")"
if [[ -n "$license_hits" ]]; then
  echo "LAYERING: copyleft license inside a kernel closure — the kernel stays MIT OR Apache-2.0 (§9):" >&2
  echo "$license_hits" >&2
  fail=1
fi

if [[ $fail -eq 0 ]]; then
  echo "layering ok: ${#KERNEL[@]} kernel crates, no forbidden dependency or copyleft license in any closure"
fi
exit $fail
