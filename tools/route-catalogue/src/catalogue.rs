//! Builds catalogue entries from assembled routes and writes them as GeoJSON and a report.

use std::fmt::Write as _;

use geo::{Geodesic, Length, LineString};
use serde_json::{Value, json};

use crate::assemble::{self, Way};
use crate::osm::{NodePositions, RouteRelation, WayNodes};

/// Area to keep, in degrees: west, south, east, north.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    /// Westernmost longitude.
    pub west: f64,
    /// Southernmost latitude.
    pub south: f64,
    /// Easternmost longitude.
    pub east: f64,
    /// Northernmost latitude.
    pub north: f64,
}

impl BoundingBox {
    /// Parses `west,south,east,north`, the format of `BBOX` in `config/pilot-region.env`.
    pub fn parse(text: &str) -> Option<Self> {
        let values: Vec<f64> = text
            .split(',')
            .map(|part| part.trim().parse().ok())
            .collect::<Option<Vec<f64>>>()?;
        let [west, south, east, north] = values.as_slice() else {
            return None;
        };
        (west < east && south < north).then_some(Self {
            west: *west,
            south: *south,
            east: *east,
            north: *north,
        })
    }

    fn contains(&self, (lon, lat): (f64, f64)) -> bool {
        (self.west..=self.east).contains(&lon) && (self.south..=self.north).contains(&lat)
    }
}

/// One catalogue entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// The OpenStreetMap relation.
    pub relation: RouteRelation,
    /// Merged lines as (longitude, latitude) points.
    pub lines: Vec<Vec<(f64, f64)>>,
    /// Disconnected pieces: 1 means no gaps.
    pub pieces: usize,
    /// Member ways missing from the file, or with nodes missing (route crosses the extract's edge).
    pub missing_ways: usize,
    /// Total length of all lines on the WGS 84 ellipsoid, in metres.
    pub length_m: f64,
}

impl Route {
    /// Whether the route is one connected line with all its ways present.
    pub fn is_complete(&self) -> bool {
        self.pieces == 1 && self.missing_ways == 0 && !self.relation.has_sub_routes
    }

    /// Whether the Club Alpino Italiano maintains it: CAI operator, or a `cai_scale` grade.
    pub fn is_cai(&self) -> bool {
        let operator = self.tag("operator").unwrap_or_default().to_lowercase();
        operator.contains("cai")
            || operator.contains("club alpino italiano")
            || self.tag("cai_scale").is_some()
    }

    /// Difficulty and the tag it comes from, only ever from tags (ADR-0015): `cai_scale` first,
    /// then `sac_scale`; otherwise `("unknown", None)`, never guessed from length or climb.
    pub fn difficulty(&self) -> (&str, Option<&'static str>) {
        if let Some(grade) = self.tag("cai_scale") {
            (grade, Some("cai_scale"))
        } else if let Some(grade) = self.tag("sac_scale") {
            (grade, Some("sac_scale"))
        } else {
            ("unknown", None)
        }
    }

    fn tag(&self, key: &str) -> Option<&str> {
        self.relation
            .tags
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
    }

    /// The catalogue fields (everything but the geometry).
    fn properties(&self) -> Value {
        let (difficulty, difficulty_source) = self.difficulty();
        json!({
            "osm_id": self.relation.id,
            "osm_url": format!("https://www.openstreetmap.org/relation/{}", self.relation.id),
            "name": self.tag("name:it").or(self.tag("name")),
            "ref": self.tag("ref"),
            "network": self.tag("network"),
            "operator": self.tag("operator"),
            "cai": self.is_cai(),
            "difficulty": difficulty,
            "difficulty_source": difficulty_source,
            "from": self.tag("from"),
            "to": self.tag("to"),
            "osmc_symbol": self.tag("osmc:symbol"),
            "length_m": self.length_m.round(),
            "pieces": self.pieces,
            "missing_ways": self.missing_ways,
            "has_sub_routes": self.relation.has_sub_routes,
            "complete": self.is_complete(),
        })
    }

    /// Bounding box of all lines: [west, south, east, north].
    fn bbox(&self) -> Option<[f64; 4]> {
        let mut points = self.lines.iter().flatten();
        let &(lon, lat) = points.next()?;
        let mut bbox = [lon, lat, lon, lat];
        for &(lon, lat) in points {
            bbox = [
                bbox[0].min(lon),
                bbox[1].min(lat),
                bbox[2].max(lon),
                bbox[3].max(lat),
            ];
        }
        Some(bbox.map(round6))
    }

    /// The index entry: catalogue fields and bounding box, no geometry.
    pub fn to_index_entry(&self) -> Value {
        let mut entry = self.properties();
        entry["bbox"] = json!(self.bbox());
        entry
    }

    /// The GeoJSON feature: a MultiLineString with the catalogue fields as properties.
    pub fn to_feature(&self) -> Value {
        let coordinates: Vec<Vec<[f64; 2]>> = self
            .lines
            .iter()
            .map(|line| {
                line.iter()
                    .map(|&(lon, lat)| [round6(lon), round6(lat)])
                    .collect()
            })
            .collect();
        json!({
            "type": "Feature",
            "id": self.relation.id,
            "geometry": { "type": "MultiLineString", "coordinates": coordinates },
            "properties": self.properties(),
        })
    }
}

/// ~10 cm precision: enough for a trail, and a much smaller file.
fn round6(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

/// Assembles `relation` from the ways and nodes read; `None` if none of its points is in `area`.
pub fn build_route(
    relation: &RouteRelation,
    ways: &WayNodes,
    nodes: &NodePositions,
    area: &BoundingBox,
) -> Option<Route> {
    let mut members = Vec::new();
    let mut missing_ways = 0;
    for &way_id in &relation.way_ids {
        // A way, or any of its nodes, may be outside the extract
        let complete_way = ways
            .get(&way_id)
            .filter(|refs| refs.iter().all(|node| nodes.contains_key(node)));
        match complete_way {
            Some(refs) => members.push(Way {
                id: way_id,
                nodes: refs.clone(),
            }),
            None => missing_ways += 1,
        }
    }
    let inside = members
        .iter()
        .flat_map(|way| way.nodes.iter())
        .any(|node| nodes.get(node).is_some_and(|&point| area.contains(point)));
    if !inside {
        return None;
    }

    let assembly = assemble::assemble(&members);
    let lines: Vec<Vec<(f64, f64)>> = assembly
        .lines
        .iter()
        .map(|line| {
            line.iter()
                .filter_map(|node| nodes.get(node).copied())
                .collect()
        })
        .collect();
    let length_m = lines
        .iter()
        .map(|line| Geodesic.length(&LineString::from(line.clone())))
        .sum();
    Some(Route {
        relation: relation.clone(),
        lines,
        pieces: assembly.pieces,
        missing_ways,
        length_m,
    })
}

/// The catalogue as a GeoJSON FeatureCollection, with its licence and attribution (ADR-0015:
/// a database derived from OpenStreetMap stays ODbL).
pub fn to_geojson(routes: &[Route]) -> Value {
    json!({
        "type": "FeatureCollection",
        "license": "ODbL-1.0",
        "attribution": "© OpenStreetMap contributors",
        "features": routes.iter().map(Route::to_feature).collect::<Vec<Value>>(),
    })
}

/// The index: every route's catalogue fields and bounding box, without geometry. Small enough to
/// publish (the full GeoJSON is over Cloudflare Pages' 25 MB file limit); the map draws routes
/// from the vector tiles.
pub fn to_index(routes: &[Route]) -> Value {
    json!({
        "license": "ODbL-1.0",
        "attribution": "© OpenStreetMap contributors",
        "routes": routes.iter().map(Route::to_index_entry).collect::<Vec<Value>>(),
    })
}

/// A Markdown report: how many routes, and which CAI routes are incomplete (to fix in OSM).
pub fn report(routes: &[Route], area: &BoundingBox) -> String {
    let count = |filter: &dyn Fn(&Route) -> bool| routes.iter().filter(|r| filter(r)).count();
    let total = routes.len();
    let complete = count(&|r| r.is_complete());
    let cai = count(&|r| r.is_cai());
    let cai_complete = count(&|r| r.is_cai() && r.is_complete());
    let unknown = count(&|r| r.difficulty().1.is_none());
    let km: f64 = routes.iter().map(|r| r.length_m).sum::<f64>() / 1000.0;
    let percent = |part: usize, whole: usize| {
        if whole == 0 {
            0.0
        } else {
            100.0 * part as f64 / whole as f64
        }
    };

    let mut text = String::new();
    let _ = writeln!(text, "# Route catalogue report\n");
    let _ = writeln!(
        text,
        "Area `{},{},{},{}` · hiking route relations from OpenStreetMap (© OpenStreetMap contributors, ODbL)\n",
        area.west, area.south, area.east, area.north
    );
    let _ = writeln!(
        text,
        "| | Routes | Complete | Incomplete |\n|---|---|---|---|"
    );
    let _ = writeln!(
        text,
        "| All | {total} | {complete} ({:.0}%) | {} |",
        percent(complete, total),
        total - complete
    );
    let _ = writeln!(
        text,
        "| CAI | {cai} | {cai_complete} ({:.0}%) | {} |\n",
        percent(cai_complete, cai),
        cai - cai_complete
    );
    let _ = writeln!(
        text,
        "Total length: {km:.0} km. Difficulty unknown (no `cai_scale`/`sac_scale` tag): {unknown}.\n"
    );
    let _ = writeln!(
        text,
        "**Incomplete** means more than one piece (a gap the mappers haven't closed), a member way missing \
         from the extract, or a super-route made of sub-routes. Gaps are never bridged; fixing the relation \
         in OpenStreetMap fixes it here at the next rebuild.\n"
    );
    let _ = writeln!(
        text,
        "## Incomplete CAI routes\n\n| Ref | Name | Pieces | Missing ways | OSM |\n|---|---|---|---|---|"
    );
    let mut incomplete: Vec<&Route> = routes
        .iter()
        .filter(|r| r.is_cai() && !r.is_complete())
        .collect();
    incomplete.sort_by(|a, b| {
        a.tag("ref")
            .cmp(&b.tag("ref"))
            .then(a.relation.id.cmp(&b.relation.id))
    });
    for route in incomplete {
        let _ = writeln!(
            text,
            "| {} | {} | {} | {} | [{}](https://www.openstreetmap.org/relation/{}) |",
            route.tag("ref").unwrap_or("—"),
            route.tag("name:it").or(route.tag("name")).unwrap_or("—"),
            route.pieces,
            route.missing_ways,
            route.relation.id,
            route.relation.id
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    fn relation(tags: &[(&str, &str)], way_ids: &[i64]) -> RouteRelation {
        RouteRelation {
            id: 42,
            tags: tags
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            way_ids: way_ids.to_vec(),
            has_sub_routes: false,
        }
    }

    /// Two ways along a meridian near Pania della Croce, 0.001° (~111 m) per step.
    fn sample() -> (WayNodes, NodePositions) {
        let ways = HashMap::from([(1, vec![1, 2]), (2, vec![2, 3]), (3, vec![10, 11])]);
        let nodes = HashMap::from([
            (1, (10.3, 44.0)),
            (2, (10.3, 44.001)),
            (3, (10.3, 44.002)),
            (10, (10.31, 44.0)),
            (11, (10.31, 44.001)),
        ]);
        (ways, nodes)
    }

    const AREA: BoundingBox = BoundingBox {
        west: 9.29,
        south: 43.79,
        east: 11.38,
        north: 44.80,
    };

    #[test]
    fn parses_the_pilot_bbox() {
        assert_eq!(BoundingBox::parse("9.29,43.79,11.38,44.80"), Some(AREA));
        assert_eq!(BoundingBox::parse("9.29,43.79,11.38"), None);
        assert_eq!(
            BoundingBox::parse("11.38,43.79,9.29,44.80"),
            None,
            "west must be < east"
        );
    }

    #[test]
    fn a_connected_route_is_complete_with_its_length() {
        let (ways, nodes) = sample();
        let route = build_route(
            &relation(&[("cai_scale", "E")], &[1, 2]),
            &ways,
            &nodes,
            &AREA,
        )
        .expect("inside the area");
        assert!(route.is_complete());
        assert_eq!(route.lines.len(), 1);
        // 0.002° of latitude at 44° N is about 222 m on the ellipsoid
        assert!(
            (route.length_m - 222.0).abs() < 1.0,
            "length {}",
            route.length_m
        );
    }

    #[test]
    fn a_gap_or_missing_way_makes_it_incomplete() {
        let (ways, nodes) = sample();
        let gap = build_route(&relation(&[], &[1, 3]), &ways, &nodes, &AREA).expect("inside");
        assert_eq!(gap.pieces, 2);
        assert!(!gap.is_complete());

        let missing =
            build_route(&relation(&[], &[1, 2, 99]), &ways, &nodes, &AREA).expect("inside");
        assert_eq!(missing.pieces, 1);
        assert_eq!(missing.missing_ways, 1);
        assert!(!missing.is_complete());
    }

    #[test]
    fn routes_outside_the_area_are_skipped() {
        let (ways, nodes) = sample();
        let elsewhere = BoundingBox {
            west: 7.0,
            south: 45.0,
            east: 8.0,
            north: 46.0,
        };
        assert_eq!(
            build_route(&relation(&[], &[1, 2]), &ways, &nodes, &elsewhere),
            None
        );
    }

    #[test]
    fn difficulty_comes_only_from_tags() {
        let (ways, nodes) = sample();
        let build = |tags: &[(&str, &str)]| {
            build_route(&relation(tags, &[1, 2]), &ways, &nodes, &AREA).expect("inside")
        };
        assert_eq!(
            build(&[("cai_scale", "EE"), ("sac_scale", "mountain_hiking")]).difficulty(),
            ("EE", Some("cai_scale"))
        );
        assert_eq!(
            build(&[("sac_scale", "mountain_hiking")]).difficulty(),
            ("mountain_hiking", Some("sac_scale"))
        );
        assert_eq!(build(&[]).difficulty(), ("unknown", None));
        assert_eq!(build(&[("cai_scale", " ")]).difficulty(), ("unknown", None));
    }

    #[test]
    fn feature_has_the_catalogue_fields() {
        let (ways, nodes) = sample();
        let tags = [
            ("name", "Sentiero 1"),
            ("ref", "1"),
            ("operator", "CAI Lucca"),
            ("osmc:symbol", "red:red:white_bar:1:black"),
        ];
        let feature = build_route(&relation(&tags, &[1, 2]), &ways, &nodes, &AREA)
            .expect("inside")
            .to_feature();
        let properties = &feature["properties"];
        assert_eq!(feature["geometry"]["type"], "MultiLineString");
        assert_eq!(
            properties["osm_url"],
            "https://www.openstreetmap.org/relation/42"
        );
        assert_eq!(properties["ref"], "1");
        assert_eq!(properties["cai"], true);
        assert_eq!(properties["difficulty"], "unknown");
        assert_eq!(properties["difficulty_source"], Value::Null);
        assert_eq!(properties["complete"], true);
        assert_eq!(properties["from"], Value::Null);
        assert_eq!(properties["osmc_symbol"], "red:red:white_bar:1:black");
    }

    #[test]
    fn index_entry_has_fields_and_bbox_but_no_geometry() {
        let (ways, nodes) = sample();
        let entry = build_route(&relation(&[("ref", "1")], &[1, 2]), &ways, &nodes, &AREA)
            .expect("inside")
            .to_index_entry();
        assert_eq!(entry["ref"], "1");
        assert_eq!(entry["bbox"], json!([10.3, 44.0, 10.3, 44.002]));
        assert!(entry.get("geometry").is_none());
    }
}
