#!/usr/bin/env bash
# Builds the pre-built route catalogue for the pilot region — ADR-0015, aret-maps#4.
#
# Usage:  scripts/build-routes.sh          whole pilot region (BBOX), ~3,500 routes
#         scripts/build-routes.sh test     small area around Pania della Croce (TEST_BBOX), ~30 routes
# Output: out/routes/routes.geojson  full catalogue, ODbL (build output, too big to publish)
#         out/routes/index.json      every route's fields + bounding box, no geometry (published)
#         out/routes/report.md       complete/incomplete counts, incomplete CAI routes to fix in OSM,
#                                    elevation checks, places counts
#         out/places/places.geojson  places index: peaks, passes, huts, water, trailheads,
#                                    settlements (aret-maps#6)
#         out/catalogue/             both, gzip-compressed, + version.json: published for the
#                                    backend's places-svc (aret-backend#24)
#         out/routes/tiles/          vector tiles, layer "routes", z9–14, clipped to the area (published)
#
# Needs: data/sources/italy.osm.pbf (downloaded by scripts/build-pilot.sh), Rust (rustup),
#        tippecanoe (brew install tippecanoe), and the elevation tiles from
#        scripts/fetch-dem.sh (same "test" argument). Area: config/pilot-region.env.

set -euo pipefail
cd "$(dirname "$0")/.."
source config/pilot-region.env

case "${1:-}" in
  "")   AREA="$BBOX" ;;
  test) AREA="$TEST_BBOX"; echo "==> Test area only: $AREA" ;;
  *)    echo "usage: $0 [test]"; exit 1 ;;
esac

INPUT="data/sources/${OSM_AREA}.osm.pbf"
[[ -f "$INPUT" ]] || { echo "$INPUT not found: run  scripts/build-pilot.sh  first (it downloads it)"; exit 1; }
command -v tippecanoe >/dev/null || { echo "tippecanoe not found: brew install tippecanoe"; exit 1; }

echo "==> Route catalogue (Rust, tools/route-catalogue)"
rm -rf out/routes
DEM="data/dem/tinitaly"
[[ -n "$(ls "$DEM"/*.tif 2>/dev/null)" ]] || { echo "no elevation tiles in $DEM: run  scripts/fetch-dem.sh ${1:-}  first"; exit 1; }
cargo run --quiet --release --package route-catalogue -- "$INPUT" "$AREA" out/routes "$DEM"

echo "==> Vector tiles: out/routes/tiles/ (layer 'routes')"
# Only the fields the map needs; the rest is in index.json. Low zooms drop the shortest routes
# if a tile gets too big; z14 keeps everything. Uncompressed, like the basemap (Cloudflare
# compresses responses itself).
tippecanoe --force --quiet \
  --output-to-directory=out/routes/tiles \
  --layer=routes \
  --minimum-zoom=9 --maximum-zoom="$MAXZOOM" \
  --clip-bounding-box="$AREA" \
  --drop-smallest-as-needed \
  --no-tile-compression \
  --attribution='<a href="https://www.openstreetmap.org/copyright">© OpenStreetMap contributors</a>; elevation <a href="https://doi.org/10.13127/tinitaly/1.1">TINITALY/INGV</a> (CC BY 4.0)' \
  -y osm_id -y name -y ref -y network -y cai -y difficulty -y complete -y osmc_symbol -y length_m \
  -y ascent_m -y descent_m -y ele_max -y duration_min -y duration_reverse_min \
  out/routes/routes.geojson

echo "==> Catalogue files for the backend: out/catalogue/ (aret-backend#24)"
# The full catalogue is over Pages' 25 MB file limit as plain GeoJSON, but ~11 MB compressed.
# version.json lets places-svc see a new build and check the downloads.
rm -rf out/catalogue && mkdir -p out/catalogue
gzip -9 -c out/routes/routes.geojson > out/catalogue/routes.geojson.gz
gzip -9 -c out/places/places.geojson > out/catalogue/places.geojson.gz
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
size() { wc -c < "$1" | tr -d ' '; }
jq -n \
  --slurpfile meta out/routes/meta.json \
  --arg built_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg routes_sha "$(sha out/catalogue/routes.geojson.gz)" --argjson routes_size "$(size out/catalogue/routes.geojson.gz)" \
  --arg places_sha "$(sha out/catalogue/places.geojson.gz)" --argjson places_size "$(size out/catalogue/places.geojson.gz)" \
  '{
    built_at: $built_at,
    osm_data_at: (if $meta[0].osm_timestamp then ($meta[0].osm_timestamp | todate) else null end),
    area: $meta[0].area,
    license: "ODbL-1.0",
    attribution: "© OpenStreetMap contributors",
    files: {
      routes: { path: "routes.geojson.gz", count: $meta[0].routes, bytes: $routes_size, sha256: $routes_sha },
      places: { path: "places.geojson.gz", count: $meta[0].places, bytes: $places_size, sha256: $places_sha }
    }
  }' > out/catalogue/version.json
echo "    $(du -h out/catalogue/routes.geojson.gz | cut -f1) routes, $(du -h out/catalogue/places.geojson.gz | cut -f1) places, OSM data of $(jq -r .osm_data_at out/catalogue/version.json)"

count=$(find out/routes/tiles -name '*.pbf' | wc -l | tr -d ' ')
echo "    tiles: $count   total size: $(du -sh out/routes/tiles | cut -f1)"
echo "    index.json: $(du -h out/routes/index.json | cut -f1)   routes.geojson: $(du -h out/routes/routes.geojson | cut -f1)"
echo "    report: out/routes/report.md"
