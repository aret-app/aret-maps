//! route-catalogue: pre-built hiking routes from OpenStreetMap (ADR-0015, aret-maps#4).
//!
//! ```text
//! route-catalogue <input.osm.pbf> <west,south,east,north> <output-dir> [<elevation-tiles-dir>]
//! route-catalogue dem-tiles <west,south,east,north>
//! route-catalogue trail-gaps <input.osm.pbf> <west,south,east,north> <output-dir>
//! ```
//!
//! Reads every `type=route` + `route=hiking` relation, assembles its ways into lines without ever
//! bridging a gap, keeps the routes touching the area, and writes:
//! - `routes.geojson`: the catalogue (ODbL, "© OpenStreetMap contributors")
//! - `index.json`: the same without geometry, plus each route's bounding box (published)
//! - `report.md`: complete/incomplete counts and the incomplete CAI routes to fix in OSM
//! - `../places/places.geojson`: peaks, passes, huts, water, trailheads, settlements (aret-maps#6)
//!
//! With a folder of TINITALY tiles (aret-maps#5), each route also gets ascent, descent, lowest and
//! highest point, a profile and a walking time, and the report compares them with what mappers
//! recorded in OSM. `dem-tiles` prints the TINITALY tiles an area needs, for `scripts/fetch-dem.sh`.
//! `trail-gaps` measures the marked trails missing from the catalogue because OSM has no route
//! relation for them ([`gaps`]).
//!
//! Normally run by `scripts/build-routes.sh`.

mod assemble;
mod catalogue;
mod dem;
mod elevation;
mod gaps;
mod osm;
mod places;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use crate::catalogue::{BoundingBox, ElevationCheck, Route};
use crate::dem::Dem;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: route-catalogue <input.osm.pbf> <west,south,east,north> <output-dir> \
                 [<elevation-tiles-dir>]\n       route-catalogue dem-tiles <west,south,east,north>\n       \
                 route-catalogue trail-gaps <input.osm.pbf> <west,south,east,north> <output-dir>";
    let parse_area = |text: &str| {
        BoundingBox::parse(text)
            .ok_or("bounding box must be west,south,east,north, e.g. 9.29,43.79,11.38,44.80")
    };
    let result = match args.as_slice() {
        [command, bbox] if command == "dem-tiles" => parse_area(bbox)
            .map_err(Into::into)
            .and_then(print_dem_tiles),
        [command, input, bbox, output] if command == "trail-gaps" => parse_area(bbox)
            .map_err(Into::into)
            .and_then(|area| gaps::run(&PathBuf::from(input), &area, &PathBuf::from(output))),
        [input, bbox, output, rest @ ..] if rest.len() <= 1 => {
            parse_area(bbox).map_err(Into::into).and_then(|area| {
                run(
                    &PathBuf::from(input),
                    &area,
                    &PathBuf::from(output),
                    rest.first().map(PathBuf::from),
                )
            })
        }
        _ => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("route-catalogue: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Prints the TINITALY tile names (UTM zone 32, 50 km squares) covering `area`, one per line.
fn print_dem_tiles(area: BoundingBox) -> Result<(), Box<dyn std::error::Error>> {
    for name in dem::tile_names(&area)? {
        println!("{name}");
    }
    Ok(())
}

fn run(
    input: &std::path::Path,
    area: &BoundingBox,
    output: &std::path::Path,
    dem_folder: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    eprintln!("reading {} …", input.display());
    let data = osm::read(input)?;
    eprintln!(
        "  {} hiking relations, {} ways, {} nodes ({:.0} s)",
        data.relations.len(),
        data.ways.len(),
        data.nodes.len(),
        started.elapsed().as_secs_f64()
    );

    let mut routes: Vec<Route> = data
        .relations
        .iter()
        .filter_map(|relation| catalogue::build_route(relation, &data.ways, &data.nodes, area))
        .collect();
    routes.sort_by_key(|route| route.relation.id);

    let dem: Option<Dem> = dem_folder.as_deref().map(Dem::open).transpose()?;
    let check = match &dem {
        Some(dem) => {
            eprintln!("adding elevation from {} tiles …", dem.tile_count());
            for route in &mut routes {
                route.add_elevation(dem);
            }
            Some(ElevationCheck::new(&routes, &data.places, dem, area))
        }
        None => {
            eprintln!("no elevation tiles given: routes without ascent, descent and time");
            None
        }
    };

    // Places (aret-maps#6) go next to the routes: out/routes → out/places
    let places = places::select(data.places, &routes, area);
    let places_folder = output.with_file_name("places");
    std::fs::create_dir_all(&places_folder)?;
    std::fs::write(
        places_folder.join("places.geojson"),
        serde_json::to_vec(&places::to_geojson(&places, dem.as_ref()))?,
    )?;

    std::fs::create_dir_all(output)?;
    let geojson_path = output.join("routes.geojson");
    std::fs::write(
        &geojson_path,
        serde_json::to_vec(&catalogue::to_geojson(&routes))?,
    )?;
    std::fs::write(
        output.join("index.json"),
        serde_json::to_vec(&catalogue::to_index(&routes))?,
    )?;
    let mut report = catalogue::report(&routes, area, check.as_ref());
    report.push('\n');
    report.push_str(&places::report(&places));
    std::fs::write(output.join("report.md"), &report)?;

    let summary: String = report
        .lines()
        .skip(4)
        .take(4)
        .collect::<Vec<&str>>()
        .join("\n");
    eprintln!("{summary}");
    eprintln!(
        "wrote {} routes to {}, index.json and report.md; {} places to {} ({:.0} s)",
        routes.len(),
        geojson_path.display(),
        places.len(),
        places_folder.join("places.geojson").display(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
