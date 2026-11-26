# aret-maps
Map tile pipeline, MapLibre styles, terrain and routing data for Aret

## Pilot-region tiles

Vector tiles for the pilot region **Alpi Apuane + Appennino tosco-emiliano**
([ADR-0014](https://github.com/aret-app/aret-docs/blob/main/docs/adr/0014-first-map-region.md)),
built with [Planetiler](https://github.com/onthegomap/planetiler) using the OpenMapTiles schema.
They are published as a folder of `z/x/y.pbf` files on Cloudflare Pages
([ADR-0005](https://github.com/aret-app/aret-docs/blob/main/docs/adr/0005-tile-format-and-hosting.md)).

### Requirements
- [Homebrew](https://brew.sh) Planetiler (bundles its own Java): `brew install planetiler`.
  If your shell sets `JAVA_HOME` to an older Java, the Homebrew launcher fails with
  *Improperly specified VM option 'MaxRAMPercentage=50'*. The build script handles this; to run
  planetiler by hand, use `env -u JAVA_HOME planetiler ...`.
- ~15 GB free disk for the first build, 8 GB RAM

### Build
```bash
scripts/build-pilot.sh pmtiles   # one file: out/pilot.pmtiles
scripts/build-pilot.sh files     # folder:   out/tiles/{z}/{x}/{y}.pbf
scripts/build-pilot.sh           # both
```
The first run downloads about 3 GB into `data/` (the Italy OpenStreetMap extract from Geofabrik,
sea polygons and Natural Earth data); later runs reuse it. `data/` and `out/` are git-ignored.

The area and maximum zoom are in [`config/pilot-region.env`](config/pilot-region.env).
`MAXZOOM=14` keeps the region at roughly 7,000 tiles, inside Cloudflare Pages' 20,000-file limit;
the script fails if a build goes over it.

### Inspect
Drag `out/pilot.pmtiles` onto [pmtiles.io](https://pmtiles.io) to browse the tiles and their layers.

### Deploy to Cloudflare Pages
The tiles are served from the Cloudflare Pages project **`aret-tiles`**
([ADR-0005](https://github.com/aret-app/aret-docs/blob/main/docs/adr/0005-tile-format-and-hosting.md)):

```
https://aret-tiles.pages.dev/tiles/{z}/{x}/{y}.pbf      (maxzoom 14)
https://aret-tiles.pages.dev/tiles/metadata.json
```

One-time setup on your computer:
```bash
npx wrangler login      # opens the browser; run it outside this repo if you can
cp .env.example .env    # private settings, git-ignored
```
Then, after `scripts/build-pilot.sh files`:
```bash
scripts/deploy-pages.sh
```
The script copies `out/tiles/` and [`site/_headers`](site/_headers) (content type, CORS, caching) into
`out/site/`, checks the Pages limits and runs Wrangler (version pinned in the script).
Never commit `.env`, `.wrangler/` or any API token.

### Attribution (required on every map using these tiles)
Tiles built with the [OpenMapTiles](https://github.com/openmaptiles/openmaptiles#license) schema are reusable
under CC-BY, and maps made with them **must display a visible credit**:

> © OpenMapTiles © OpenStreetMap contributors

Map data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright) (ODbL).
Code in this repository is BSD-3-Clause.

### Last build (2026-11-15)
| | |
|---|---|
| Area | `9.29,43.79,11.38,44.80`, zoom 0–14 |
| `out/pilot.pmtiles` | 127 MB |
| `out/tiles/` | 8,430 tiles, 195 MB (z14: 6,240 · z13: 1,584 · z12: 425 · z11: 117 · z0–z10: 64) |
| Cloudflare Pages limits | ✓ under 20,000 files and 25 MB per file |

## Pre-built routes

Ready-made hiking routes from OpenStreetMap route relations, mostly CAI trails
([ADR-0015](https://github.com/aret-app/aret-docs/blob/main/docs/adr/0015-prebuilt-routes-open-data.md),
aret-maps#4). The Rust tool [`tools/route-catalogue`](tools/route-catalogue/src/main.rs) reads the same
Italy extract as the tiles, keeps every `type=route` + `route=hiking` relation touching the area, and joins
each route's ways into lines.

**Safety rules (ADR-0015):** gaps are **never bridged**. A route whose ways don't all connect keeps its
pieces separate and is marked `complete: false`. Fix it in OpenStreetMap: the next build picks it up.
**Difficulty only from tags** (`cai_scale`, then `sac_scale`); otherwise `"unknown"`, never guessed.
Names are never invented: many CAI routes only have a `ref` (show them as "CAI 117").

### Build
Needs Rust ([rustup](https://rustup.rs); the version is pinned in `rust-toolchain.toml`) and
`brew install tippecanoe`, plus `data/sources/italy.osm.pbf` from `scripts/build-pilot.sh`.
```bash
scripts/build-routes.sh test   # ~30 routes around Pania della Croce (TEST_BBOX), to try things out
scripts/build-routes.sh        # whole pilot region, ~3,500 routes, ~1 minute
```
Output in `out/routes/`:

| File | What | Published |
|---|---|---|
| `routes.geojson` | Full catalogue: MultiLineString per route + all fields | no (32 MB, over Pages' 25 MB limit) |
| `index.json` | Every route's fields + `bbox`, no geometry (list and search) | `/routes/index.json` |
| `tiles/` | Vector tiles, layer `routes`, z9–14, clipped to the area | `/routes/{z}/{x}/{y}.pbf` |
| `report.md` | Complete/incomplete counts, incomplete CAI routes with OSM links | no |

Fields: `osm_id`, `osm_url`, `name`, `ref` (CAI number), `network` (`lwn`/`rwn`/`nwn`/`iwn`),
`operator`, `cai`, `difficulty` (+ `difficulty_source`), `from`, `to`, `osmc_symbol` (waymark),
`length_m` (WGS 84 ellipsoid), `pieces`, `missing_ways`, `has_sub_routes`, `complete`. Ascent and
descent come later, with elevation data (DEM).

`scripts/deploy-pages.sh` publishes the routes together with the basemap tiles when `out/routes/`
exists. A deploy replaces the whole site, so build both first.

## Weekly rebuild (GitHub Actions)

[`.github/workflows/publish-tiles.yml`](.github/workflows/publish-tiles.yml) rebuilds and republishes the
pilot region **every Monday at 03:00 UTC** from a fresh OpenStreetMap extract (ADR-0017), so fixes made in
OpenStreetMap reach Aret within a week. It runs the same three scripts as above, with pinned Planetiler,
tippecanoe and Rust.

- **Downloads:** the Italy extract once a week from Geofabrik. Sea polygons and Natural Earth are cached
  for a month.
- **Safety:** it won't publish if the new build has more than 10% fewer routes than the live site. The
  build summary shows the route report; `report.md` and `index.json` are kept as artifacts for 30 days.
- **Manual run:** Actions → *Publish map tiles* → *Run workflow*. Untick **deploy** for a dry run.
- **Pull requests** that change the build scripts, `config/`, `site/` or the workflow get a dry run.
- **Secrets** (Settings → Secrets and variables → Actions): `CLOUDFLARE_API_TOKEN`, a token with only
  the *Cloudflare Pages: Edit* permission, and `CLOUDFLARE_ACCOUNT_ID`. Without them, publishing fails
  with a clear message; dry runs don't need them.

**Licence:** the catalogue is derived from OpenStreetMap, so as a database it is **ODbL**, and every map
or export showing routes credits "© OpenStreetMap contributors".

### Last build (2026-11-26, extract of 2026-11-15)
| | |
|---|---|
| Routes touching the pilot area | 3,487 (23,332 km): 3,396 complete (97%), 91 incomplete |
| CAI routes | 3,164: 3,101 complete (98%), 63 incomplete |
| Difficulty | E 2,448 · T 477 · EE 193 · EEA 5 · unknown 354 |
| Vector tiles | 5,607 tiles, 28 MB, largest 189 KB (z14: 3,942 · z13: 1,185 · z12: 340 · z11: 97 · z10: 31 · z9: 12) |
| With the basemap | ~14,040 files: ✓ under Cloudflare Pages' 20,000-file limit |
| Run time | ~50 s, ~1 GB memory (Apple Silicon) |
