//! route-catalogue: pre-built hiking routes from OpenStreetMap (ADR-0015, aret-maps#4).
//!
//! ```text
//! route-catalogue <input.osm.pbf> <west,south,east,north> <output-dir>
//! ```
//!
//! Reads every `type=route` + `route=hiking` relation, assembles its ways into lines without ever
//! bridging a gap, keeps the routes touching the area, and writes:
//! - `routes.geojson`: the catalogue (ODbL, "© OpenStreetMap contributors")
//! - `index.json`: the same without geometry, plus each route's bounding box (published)
//! - `report.md`: complete/incomplete counts and the incomplete CAI routes to fix in OSM
//!
//! Normally run by `scripts/build-routes.sh`.

mod assemble;
mod catalogue;
mod osm;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use crate::catalogue::{BoundingBox, Route};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, bbox, output] = args.as_slice() else {
        eprintln!("usage: route-catalogue <input.osm.pbf> <west,south,east,north> <output-dir>");
        return ExitCode::from(2);
    };
    let Some(area) = BoundingBox::parse(bbox) else {
        eprintln!("bounding box must be west,south,east,north, e.g. 9.29,43.79,11.38,44.80");
        return ExitCode::from(2);
    };
    match run(&PathBuf::from(input), &area, &PathBuf::from(output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("route-catalogue: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(
    input: &std::path::Path,
    area: &BoundingBox,
    output: &std::path::Path,
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
    let report = catalogue::report(&routes, area);
    std::fs::write(output.join("report.md"), &report)?;

    let summary: String = report
        .lines()
        .skip(4)
        .take(4)
        .collect::<Vec<&str>>()
        .join("\n");
    eprintln!("{summary}");
    eprintln!(
        "wrote {} routes to {}, index.json and report.md ({:.0} s)",
        routes.len(),
        geojson_path.display(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
