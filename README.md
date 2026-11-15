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
