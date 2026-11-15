#!/usr/bin/env bash
# Builds vector tiles for the Aret pilot region with Planetiler (OpenMapTiles schema).
#   Area:   ADR-0014, settings in config/pilot-region.env
#   Format: ADR-0005, a folder of z/x/y.pbf tiles for Cloudflare Pages
#
# Usage:  scripts/build-pilot.sh [pmtiles|files|both]     (default: both)
#   pmtiles → out/pilot.pmtiles  one file, easy to inspect at https://pmtiles.io
#   files   → out/tiles/         z/x/y.pbf folder, what we deploy
#
# Needs: planetiler (brew install planetiler). First run downloads ~3 GB into data/
# (Italy OSM extract + sea polygons + Natural Earth); later runs reuse it.

set -euo pipefail
cd "$(dirname "$0")/.."
source config/pilot-region.env

FORMAT="${1:-both}"
case "$FORMAT" in pmtiles|files|both) ;; *) echo "usage: $0 [pmtiles|files|both]"; exit 1 ;; esac
command -v planetiler >/dev/null || { echo "planetiler not found: brew install planetiler"; exit 1; }

# Homebrew's planetiler launcher uses $JAVA_HOME when it is set; if that points to an
# old Java (e.g. Java 8 for other projects) it fails. Unset it for planetiler only, so
# the launcher uses its bundled Homebrew Java. The launcher already gives Java 50% of RAM.
planetiler() { env -u JAVA_HOME planetiler "$@"; }
mkdir -p data out

COMMON=(--download --area="$OSM_AREA" --bounds="$BBOX" --maxzoom="$MAXZOOM")

if [[ "$FORMAT" == pmtiles || "$FORMAT" == both ]]; then
  echo "==> Building out/pilot.pmtiles"
  planetiler "${COMMON[@]}" --output=out/pilot.pmtiles --force
  echo "    size: $(du -h out/pilot.pmtiles | cut -f1)"
fi

if [[ "$FORMAT" == files || "$FORMAT" == both ]]; then
  echo "==> Building out/tiles/ (z/x/y.pbf, uncompressed)"
  rm -rf out/tiles
  # Trailing slash = folder output. Uncompressed tiles avoid Content-Encoding
  # header issues on Cloudflare Pages, which compresses responses itself.
  planetiler "${COMMON[@]}" --output=out/tiles/ --tile_compression=none --force

  count=$(find out/tiles -name '*.pbf' | wc -l | tr -d ' ')
  too_big=$(find out/tiles -name '*.pbf' -size +25M | wc -l | tr -d ' ')
  echo "    tiles: $count   total size: $(du -sh out/tiles | cut -f1)"
  echo "    per zoom:"
  for z in $(ls out/tiles | grep -E '^[0-9]+$' | sort -n); do
    printf '      z%-2s %s\n' "$z" "$(find "out/tiles/$z" -name '*.pbf' | wc -l | tr -d ' ')"
  done

  # Cloudflare Pages limits (ADR-0005): 20,000 files and 25 MB per file per deployment
  if (( count > 20000 )); then
    echo "!!  $count tiles exceed Cloudflare Pages' 20,000-file limit: lower MAXZOOM or revisit ADR-0005" >&2
    exit 1
  fi
  if (( too_big > 0 )); then
    echo "!!  $too_big tiles are larger than 25 MB, over Cloudflare Pages' per-file limit" >&2
    exit 1
  fi
  echo "    ✓ within Cloudflare Pages limits"
fi
