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
`brew install tippecanoe`, plus `data/sources/italy.osm.pbf` from `scripts/build-pilot.sh` and the
elevation tiles from `scripts/fetch-dem.sh`.
```bash
scripts/fetch-dem.sh test && scripts/build-routes.sh test   # ~30 routes around Pania della Croce
scripts/fetch-dem.sh && scripts/build-routes.sh             # whole pilot region, ~3,500 routes
```
Output in `out/routes/`:

| File | What | Published |
|---|---|---|
| `routes.geojson` | Full catalogue: MultiLineString per route + all fields + elevation profiles | no (38 MB, over Pages' 25 MB limit) |
| `index.json` | Every route's fields + `bbox`, no geometry (list and search) | `/routes/index.json` |
| `tiles/` | Vector tiles, layer `routes`, z9–14, clipped to the area | `/routes/{z}/{x}/{y}.pbf` |
| `report.md` | Complete/incomplete counts, incomplete CAI routes with OSM links | no |

Fields: `osm_id`, `osm_url`, `name`, `ref` (CAI number), `network` (`lwn`/`rwn`/`nwn`/`iwn`),
`operator`, `cai`, `difficulty` (+ `difficulty_source`), `from`, `to`, `osmc_symbol` (waymark),
`length_m` (WGS 84 ellipsoid), `pieces`, `missing_ways`, `has_sub_routes`, `complete`. Ascent and
descent come later, with elevation data (DEM).

`scripts/deploy-pages.sh` publishes the routes together with the basemap tiles when `out/routes/`
exists. A deploy replaces the whole site, so build both first.

## Places index (aret-maps#6)

The same run of `scripts/build-routes.sh` also writes **`out/places/places.geojson`**: the places hikers
search for by name (ADR-0017), from the same OpenStreetMap read, so it costs about 10 seconds more.

| Kind (`kind`) | From OpenStreetMap (`type`) |
|---|---|
| `peak` | `natural=peak`, `natural=volcano` |
| `pass` | `mountain_pass=yes`, `natural=saddle` |
| `hut` | `tourism=alpine_hut` (staffed *rifugio*), `tourism=wilderness_hut` (*bivacco*), `amenity=shelter` (not bus stop or picnic shelters) |
| `water` | `natural=spring`, `amenity=drinking_water` (`drinking_water=yes/no` kept: it matters) |
| `trailhead` | `highway=trailhead`, and car parks within 300 m of a route end |
| `settlement` | `place=city/town/village/hamlet` |

- **Huts and car parks drawn as areas** become a point at their centre.
- **Each place** has `name`, `alt_name`, `ele` with `ele_source` (`osm` tag, else `dem` from TINITALY), and
  `osm_url`; huts also have `operator` and `capacity`.
- **Names are never invented:** places without a name stay in the file for the map, with
  `searchable: false`.
- **Not deduplicated:** features with the same name close together are usually different things (two
  points of a pass, vents of a mud volcano).
- **Not published:** the file (ODbL) is for the backend's place search (aret-app/aret-backend#24). At
  7.4 MB it would fit on Pages if needed.

Pilot region (2026-11-27): 22,089 places, of which searchable by name are 3,009 peaks, 512 passes, 233 huts,
725 water points, 339 trailheads and 7,708 settlements. Spot-checked: Pania della Croce, Monte Cimone,
Rifugio Enrico Rossi, Rifugio Cesare Battisti, Foce di Mosceta, Passo del Vestito, Levigliani, 46 bivouacs.

## Trail gaps: marked trails the catalogue can't see

The catalogue (and the backend's tours) only knows trails that are in a hiking **route relation**
(ADR-0015). `trail-gaps` measures what that leaves out:

```bash
source config/pilot-region.env
cargo run --release --package route-catalogue -- trail-gaps data/sources/italy.osm.pbf "$BBOX" out/gaps
```

It writes `out/gaps/trail-gaps.md` (totals, and the 60 longest missing trails with links to fix them in
OpenStreetMap) and `out/gaps/trail-gaps.geojson` (all of them, to view on a map). A path counts as
**marked** if its tags say so: `osmc:symbol`, `cai_scale`, `trailblazed=yes`, or a trail number (`ref`)
on a path, footway, bridleway or steps (on a track a `ref` is often a forest road's number). Connected
paths with the same number count as one trail. About 50 s and 450 MB for the pilot region.

Pilot region, 2026-11-27 (OSM data of 2026-11-15):

| | Length |
|---|---:|
| Trails in hiking route relations (what the catalogue uses) | 15,609 km |
| **Marked paths in no relation** | **39 km** (88 trails, 8 of them 1 km or more) |
| Unmarked paths with a SAC grade | 1,668 km |
| Other paths, footways and steps | 10,666 km |
| Other tracks | 19,567 km |

So the relations hold practically all marked trails here: what's missing is mostly **unmarked** paths,
which only matter for routing over every path (ADR-0006, "design your own route"). The longest missing
marked trails are short local paths (Marmitte dei Giganti 3 km, Sentiero del Catino 2 km) worth adding
to OSM. Tours can also lose a stretch where a relation has a gap: see the incomplete routes in
`out/routes/report.md`.

## Weekly rebuild (GitHub Actions)

[`.github/workflows/publish-tiles.yml`](.github/workflows/publish-tiles.yml) rebuilds and republishes the
pilot region **every Monday at 03:00 UTC** from a fresh OpenStreetMap extract (ADR-0017), so fixes made in
OpenStreetMap reach Aret within a week. It runs the same three scripts as above, with pinned Planetiler,
tippecanoe and Rust.

- **Downloads:** the Italy extract once a week from Geofabrik, with retries and an MD5 check. If
  Geofabrik stays unreachable (it has refused GitHub's runners for hours at a time), the same daily extract
  comes from OpenStreetMap France; the run summary says which. Every download sends a User-Agent naming
  this project. Sea polygons and Natural Earth are cached for a month.
- **Dry runs reuse the extract** of the latest weekly build from the Actions cache, so testing a change
  doesn't download Italy again.
- **Safety:** it won't publish if the new build has more than 10% fewer routes than the live site. The
  build summary shows the route report; `report.md` and `index.json` are kept as artifacts for 30 days.
- **Manual run:** Actions → *Publish map tiles* → *Run workflow*. Untick **deploy** for a dry run.
- **Pull requests** that change the build scripts, `config/`, `site/` or the workflow get a dry run.
- **Secrets** (Settings → Secrets and variables → Actions): `CLOUDFLARE_API_TOKEN`, a token with only
  the *Cloudflare Pages: Edit* permission, and `CLOUDFLARE_ACCOUNT_ID`. Without them, publishing fails
  with a clear message; dry runs don't need them.

**Licence:** the catalogue is derived from OpenStreetMap, so as a database it is **ODbL**, and every map
or export showing routes credits "© OpenStreetMap contributors".

### Elevation: ascent, descent, walking time (aret-maps#5)
Each route gets **ascent and descent, lowest and highest point, start and end elevation, a walking time up
and back, and an elevation profile** from the TINITALY 1.1 elevation model of Italy.

- **Data:** [TINITALY 1.1](https://tinitaly.pi.ingv.it/) (INGV): a 10 m model of the ground, not
  the treetops, so forest doesn't add fake climbing. CC BY 4.0, credit required:
  > Tarquini S., Isola I., Favalli M., Battistini A., Dotta G. (2023). TINITALY, a digital elevation model
  > of Italy with a 10 meters cell size (Version 1.1). INGV. https://doi.org/10.13127/tinitaly/1.1

  `scripts/fetch-dem.sh [test]` downloads the tiles the area needs once, into git-ignored
  `data/dem/tinitaly/` (15 tiles, ~1.4 GB for the pilot region). `route-catalogue dem-tiles <bbox>`
  lists them.
- **Direction:** a route that is one line starts at its **lower end** (usually the valley); its ascent and
  `duration_min` are for going up, `duration_reverse_min` for coming back.
- **Ascent/descent:** profile sampled every 10 m, smoothed over 30 m, changes counted from 3 m, so paths
  crossing steep slopes don't collect fake climbing from a few metres of position error. Calibrated
  against the 1,812 pilot routes that carry a mapped `ascent` (ours −3% on median) and 53 with a mapped
  `duration`, mostly CAI sign times (ours −5% on median, 8% apart); the table is in
  [`elevation.rs`](tools/route-catalogue/src/elevation.rs).
- **Walking time:** DIN 33466, as used by the Alpine clubs: 4 km/h on the flat, 300 m/h up, 500 m/h down;
  the larger of distance and climbing time counts fully, the other half. An estimate without breaks.
  Only for complete single-line routes: no time across gaps or summed over branches.
- **Check:** `report.md` compares the model with every named summit that has an `ele` tag, and our
  ascent and time with routes that carry `ascent`/`duration` tags.

New fields: `ascent_m`, `descent_m`, `ele_min`, `ele_max`, `ele_start`, `ele_end`, `duration_min`,
`duration_reverse_min` (all `null` when the route leaves the model's area); `routes.geojson` also has
`profiles` (per line, `[distance m, elevation m]` every 50 m). The tiles carry ascent, descent, highest
point and both times.

### Last build (2026-11-26, extract of 2026-11-15)
| | |
|---|---|
| Routes touching the pilot area | 3,487 (23,332 km): 3,396 complete (97%), 91 incomplete |
| CAI routes | 3,164: 3,101 complete (98%), 63 incomplete |
| Difficulty | E 2,448 · T 477 · EE 193 · EEA 5 · unknown 354 |
| Vector tiles | 5,607 tiles, 28 MB, largest 189 KB (z14: 3,942 · z13: 1,185 · z12: 340 · z11: 97 · z10: 31 · z9: 12) |
| With the basemap | ~14,040 files: ✓ under Cloudflare Pages' 20,000-file limit |
| Elevation (2026-11-27) | 3,464 routes with ascent/descent, 3,165 with a walking time; highest route point 2,162 m (Monte Cimone) |
| Elevation checks | 2,861 summits: model within 6.4 m of the mapped height (median); 1,812 mapped ascents: ours −3% (median); 53 mapped times: ours −5% (median, 8% apart) |
| Run time | ~80 s with elevation, ~1 GB memory (Apple Silicon) |
