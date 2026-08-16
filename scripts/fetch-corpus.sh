#!/usr/bin/env bash
# Download the DN corpus (public data.gouv.fr dataset) into corpus/data/
# (gitignored). Idempotent: skips if the file is already present.
#
# The dataset publishes dated snapshots; this resolves the current one
# through the API. Note that numbers in corpus/*.md were produced from
# the 2026-08-15 snapshot — a newer snapshot may differ slightly.
set -euo pipefail

DATASET="descriptif-des-demarches-publiees-sur-demarche-numerique-gouv-fr"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/corpus/data/demarches.json"

if [[ -f "$OUT" ]]; then
  echo "already present: $OUT"
  exit 0
fi

mkdir -p "$(dirname "$OUT")"
echo "resolving current snapshot…"
URL="$(curl -fsSL "https://www.data.gouv.fr/api/1/datasets/$DATASET/" \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(next(r['url'] for r in d['resources'] if r['format']=='json.gz'))")"
echo "downloading $URL"
curl -fSL --progress-bar -o "$OUT.gz" "$URL"
gunzip "$OUT.gz"
echo "ready: $OUT ($(du -h "$OUT" | cut -f1))"
