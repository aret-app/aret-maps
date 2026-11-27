#!/usr/bin/env bash
# Downloads the TINITALY 1.1 elevation tiles (INGV, 10 m, CC BY 4.0) covering an area — aret-maps#5.
#
# Usage:  scripts/fetch-dem.sh          tiles for the pilot region (BBOX), ~15 tiles, ~1.3 GB
#         scripts/fetch-dem.sh test     tiles for the Pania test area (TEST_BBOX), 1 tile
# Output: data/dem/tinitaly/<tile>.tif (git-ignored). Tiles already there are kept: TINITALY
#         rarely changes, so this downloads each tile once.
#
# Credit required by the licence (also in the README and in routes.geojson / index.json):
#   Tarquini S. et al. (2023). TINITALY, a digital elevation model of Italy with a 10 meters cell
#   size (Version 1.1). INGV. https://doi.org/10.13127/tinitaly/1.1

set -euo pipefail
cd "$(dirname "$0")/.."
source config/pilot-region.env

case "${1:-}" in
  "")   AREA="$BBOX" ;;
  test) AREA="$TEST_BBOX" ;;
  *)    echo "usage: $0 [test]"; exit 1 ;;
esac

BASE="https://tinitaly.pi.ingv.it/data_1.1"
USER_AGENT="${USER_AGENT:-aret-maps map build (+https://github.com/aret-app/aret-maps)}"
OUT="data/dem/tinitaly"
mkdir -p "$OUT"

tiles=$(cargo run --quiet --release --package route-catalogue -- dem-tiles "$AREA")
echo "==> TINITALY tiles for $AREA: $(echo "$tiles" | wc -l | tr -d ' ')"

for tile in $tiles; do
  if [[ -f "$OUT/$tile.tif" ]]; then
    echo "    $tile: already here"
    continue
  fi
  zip="$OUT/$tile.zip"
  status=$(curl -sL -A "$USER_AGENT" --retry 5 --retry-delay 30 --connect-timeout 30 \
    -o "$zip" -w '%{http_code}' "$BASE/$tile/$tile.zip" || echo 000)
  case "$status" in
    200)
      unzip -q -o -j "$zip" "*.tif" -d "$OUT"
      rm -f "$zip"
      echo "    $tile: $(du -h "$OUT/$tile.tif" | cut -f1)"
      ;;
    404)
      # Squares with no land have no tile
      rm -f "$zip"
      echo "    $tile: not published (sea), skipped"
      ;;
    *)
      rm -f "$zip"
      echo "!!  $tile: download failed (HTTP $status)" >&2
      exit 1
      ;;
  esac
done
echo "    total: $(du -sh "$OUT" | cut -f1)"
