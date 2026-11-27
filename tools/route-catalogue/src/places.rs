//! Places index from OpenStreetMap: peaks, passes, huts, water, trailheads and settlements
//! (aret-maps#6, ADR-0017). What hikers search for by name, e.g. "Pania della Croce".
//!
//! Names are never invented: a place without a `name` stays in the index (an unnamed spring is
//! still useful on a map) but is marked `searchable: false`. Car parks are kept only near the end
//! of a route, where they serve as trailheads.

use std::collections::HashMap;
use std::fmt::Write as _;

use geo::{Distance, Haversine, Point};
use serde_json::{Value, json};

use crate::catalogue::{BoundingBox, Route};
use crate::dem::Dem;

/// OpenStreetMap tags kept for each place.
pub const KEPT_TAGS: &[&str] = &[
    "name",
    "name:it",
    "alt_name",
    "ele",
    "operator",
    "capacity",
    "drinking_water",
];

/// Car parks farther than this from every route end are not trailheads.
const TRAILHEAD_PARKING_M: f64 = 300.0;

/// What kind of place it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// Summit.
    Peak,
    /// Mountain pass or saddle.
    Pass,
    /// Staffed refuge, unstaffed bivouac or shelter.
    Hut,
    /// Spring or drinking water.
    Water,
    /// Trailhead, or a car park at the end of a route.
    Trailhead,
    /// Car park, before the trailhead filter.
    Parking,
    /// Town, village or hamlet.
    Settlement,
}

impl Kind {
    /// Name used in the output.
    pub fn name(self) -> &'static str {
        match self {
            Self::Peak => "peak",
            Self::Pass => "pass",
            Self::Hut => "hut",
            Self::Water => "water",
            Self::Trailhead | Self::Parking => "trailhead",
            Self::Settlement => "settlement",
        }
    }
}

/// Kind and finer type (e.g. `alpine_hut`) of a feature with these tags, if it is a place.
pub fn classify(tags: &[(&str, &str)]) -> Option<(Kind, &'static str)> {
    let tag = |key: &str| {
        tags.iter()
            .find(|(k, _)| *k == key)
            .map(|(_, value)| *value)
    };
    match (tag("natural"), tag("tourism"), tag("amenity")) {
        (Some("peak"), _, _) => return Some((Kind::Peak, "peak")),
        (Some("volcano"), _, _) => return Some((Kind::Peak, "volcano")),
        (Some("saddle"), _, _) => return Some((Kind::Pass, "saddle")),
        _ => {}
    }
    if tag("mountain_pass") == Some("yes") {
        return Some((Kind::Pass, "pass"));
    }
    match (tag("tourism"), tag("amenity")) {
        (Some("alpine_hut"), _) => return Some((Kind::Hut, "alpine_hut")),
        (Some("wilderness_hut"), _) => return Some((Kind::Hut, "wilderness_hut")),
        // Bus stop and picnic shelters are not mountain shelters
        (_, Some("shelter"))
            if !matches!(
                tag("shelter_type"),
                Some("public_transport" | "picnic_shelter")
            ) =>
        {
            return Some((Kind::Hut, "shelter"));
        }
        _ => {}
    }
    match (tag("natural"), tag("amenity")) {
        (Some("spring"), _) => return Some((Kind::Water, "spring")),
        (_, Some("drinking_water")) => return Some((Kind::Water, "drinking_water")),
        (_, Some("parking")) => return Some((Kind::Parking, "parking")),
        _ => {}
    }
    if tag("highway") == Some("trailhead") {
        return Some((Kind::Trailhead, "trailhead"));
    }
    match tag("place") {
        Some(kind @ ("city" | "town" | "village" | "hamlet")) => Some((
            Kind::Settlement,
            match kind {
                "city" => "city",
                "town" => "town",
                "village" => "village",
                _ => "hamlet",
            },
        )),
        _ => None,
    }
}

/// A place as read from OpenStreetMap.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    /// `node` or `way`.
    pub osm_type: &'static str,
    /// OpenStreetMap id.
    pub id: i64,
    /// What it is.
    pub kind: Kind,
    /// Finer type, e.g. `alpine_hut`, `spring`, `village`.
    pub subkind: &'static str,
    /// The tags in [`KEPT_TAGS`] that are present.
    pub tags: HashMap<String, String>,
    /// (longitude, latitude): the node, or the centre of the way's nodes.
    pub position: (f64, f64),
}

impl Place {
    /// A non-empty tag.
    pub fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
    }

    /// The `ele` tag, if it is a number (`1859`, `1859 m`).
    pub fn mapped_ele(&self) -> Option<f64> {
        self.tag("ele")?
            .trim()
            .trim_end_matches('m')
            .trim()
            .replace(',', ".")
            .parse()
            .ok()
    }

    fn feature(&self, dem: Option<&Dem>) -> Value {
        let (ele, ele_source) = match (
            self.mapped_ele(),
            dem.and_then(|dem| dem.elevation(self.position)),
        ) {
            (Some(ele), _) => (Some(ele.round()), Some("osm")),
            (None, Some(ele)) => (Some(ele.round()), Some("dem")),
            (None, None) => (None, None),
        };
        let name = self.tag("name:it").or(self.tag("name"));
        json!({
            "type": "Feature",
            "id": format!("{}/{}", self.osm_type, self.id),
            "geometry": {
                "type": "Point",
                "coordinates": [round6(self.position.0), round6(self.position.1)],
            },
            "properties": {
                "osm_url": format!("https://www.openstreetmap.org/{}/{}", self.osm_type, self.id),
                "kind": self.kind.name(),
                "type": self.subkind,
                "name": name,
                "alt_name": self.tag("alt_name"),
                "searchable": name.is_some(),
                "ele": ele,
                "ele_source": ele_source,
                "operator": self.tag("operator"),
                "capacity": self.tag("capacity"),
                "drinking_water": self.tag("drinking_water"),
            },
        })
    }
}

fn round6(value: f64) -> f64 {
    (value * 1e6).round() / 1e6
}

/// The places inside `area`, with car parks kept only within [`TRAILHEAD_PARKING_M`] of a route
/// end (and then listed as trailheads), sorted by kind and name.
pub fn select(places: Vec<Place>, routes: &[Route], area: &BoundingBox) -> Vec<Place> {
    let ends = RouteEnds::new(routes);
    let mut selected: Vec<Place> = places
        .into_iter()
        .filter(|place| area.contains(place.position))
        .filter_map(|mut place| {
            if place.kind == Kind::Parking {
                if !ends.within(place.position, TRAILHEAD_PARKING_M) {
                    return None;
                }
                place.kind = Kind::Trailhead;
            }
            Some(place)
        })
        .collect();
    selected.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.tag("name").cmp(&b.tag("name")))
            .then(a.id.cmp(&b.id))
    });
    selected
}

/// First and last points of every route line, in a grid of ~1 km cells for quick lookups.
struct RouteEnds {
    cells: HashMap<(i64, i64), Vec<(f64, f64)>>,
}

impl RouteEnds {
    const CELL_DEG: f64 = 0.01;

    fn new(routes: &[Route]) -> Self {
        let mut cells: HashMap<(i64, i64), Vec<(f64, f64)>> = HashMap::new();
        let ends = routes
            .iter()
            .flat_map(|route| route.lines.iter())
            .flat_map(|line| [line.first(), line.last()])
            .flatten();
        for &point in ends {
            cells.entry(Self::cell(point)).or_default().push(point);
        }
        Self { cells }
    }

    fn cell((lon, lat): (f64, f64)) -> (i64, i64) {
        (
            (lon / Self::CELL_DEG).floor() as i64,
            (lat / Self::CELL_DEG).floor() as i64,
        )
    }

    /// Whether a route end is within `metres` of `point` (`metres` must be under one cell).
    fn within(&self, point: (f64, f64), metres: f64) -> bool {
        let (x, y) = Self::cell(point);
        (x - 1..=x + 1)
            .flat_map(|cx| (y - 1..=y + 1).map(move |cy| (cx, cy)))
            .filter_map(|cell| self.cells.get(&cell))
            .flatten()
            .any(|&end| Haversine.distance(Point::from(point), Point::from(end)) <= metres)
    }
}

/// The places as a GeoJSON FeatureCollection (ODbL, derived from OpenStreetMap).
pub fn to_geojson(places: &[Place], dem: Option<&Dem>) -> Value {
    json!({
        "type": "FeatureCollection",
        "license": "ODbL-1.0",
        "attribution": "© OpenStreetMap contributors",
        "elevation_source": dem.map(|_| crate::dem::CITATION),
        "features": places.iter().map(|place| place.feature(dem)).collect::<Vec<Value>>(),
    })
}

/// The places part of the report: how many of each kind, and how many are searchable by name.
pub fn report(places: &[Place]) -> String {
    let mut counts: std::collections::BTreeMap<&str, (usize, usize)> =
        std::collections::BTreeMap::new();
    for place in places {
        let entry = counts.entry(place.kind.name()).or_default();
        entry.0 += 1;
        if place.tag("name").is_some() || place.tag("name:it").is_some() {
            entry.1 += 1;
        }
    }
    let mut text = String::new();
    let _ = writeln!(text, "## Places\n");
    let _ = writeln!(
        text,
        "| Kind | Places | With a name (searchable) |\n|---|---|---|"
    );
    for (kind, (total, named)) in &counts {
        let _ = writeln!(text, "| {kind} | {total} | {named} |");
    }
    let peaks_without_ele = places
        .iter()
        .filter(|place| place.kind == Kind::Peak && place.mapped_ele().is_none())
        .count();
    let _ = writeln!(
        text,
        "\nPeaks without an `ele` tag (elevation from TINITALY instead): {peaks_without_ele}. \
         Trailheads include car parks within {TRAILHEAD_PARKING_M:.0} m of a route end."
    );
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_mountain_places() {
        assert_eq!(classify(&[("natural", "peak")]), Some((Kind::Peak, "peak")));
        assert_eq!(
            classify(&[("mountain_pass", "yes"), ("highway", "path")]),
            Some((Kind::Pass, "pass"))
        );
        assert_eq!(
            classify(&[("tourism", "alpine_hut")]),
            Some((Kind::Hut, "alpine_hut"))
        );
        assert_eq!(
            classify(&[("amenity", "shelter")]),
            Some((Kind::Hut, "shelter"))
        );
        assert_eq!(
            classify(&[("amenity", "shelter"), ("shelter_type", "public_transport")]),
            None,
            "a bus stop is not a mountain shelter"
        );
        assert_eq!(
            classify(&[("natural", "spring")]),
            Some((Kind::Water, "spring"))
        );
        assert_eq!(
            classify(&[("amenity", "parking")]),
            Some((Kind::Parking, "parking"))
        );
        assert_eq!(
            classify(&[("place", "hamlet")]),
            Some((Kind::Settlement, "hamlet"))
        );
        assert_eq!(classify(&[("place", "locality")]), None);
        assert_eq!(classify(&[("highway", "path")]), None);
    }

    fn place(kind: Kind, position: (f64, f64), name: Option<&str>) -> Place {
        Place {
            osm_type: "node",
            id: 1,
            kind,
            subkind: "x",
            tags: name
                .map(|n| HashMap::from([("name".to_owned(), n.to_owned())]))
                .unwrap_or_default(),
            position,
        }
    }

    #[test]
    fn keeps_only_car_parks_near_route_ends() {
        use crate::osm::RouteRelation;
        let route = Route {
            relation: RouteRelation {
                id: 7,
                tags: HashMap::new(),
                way_ids: Vec::new(),
                has_sub_routes: false,
            },
            lines: vec![vec![(10.30, 44.00), (10.31, 44.01)]],
            pieces: 1,
            missing_ways: 0,
            length_m: 1400.0,
            elevation: None,
        };
        let area = BoundingBox::parse("9.29,43.79,11.38,44.80").expect("bbox");
        let near = place(Kind::Parking, (10.301, 44.0005), None); // ~90 m from the start
        let far = place(Kind::Parking, (10.35, 44.05), None);
        let outside = place(Kind::Peak, (7.0, 45.0), Some("Elsewhere"));
        let selected = select(vec![near, far, outside], &[route], &area);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].kind, Kind::Trailhead);
    }

    #[test]
    fn nameless_places_are_kept_but_not_searchable() {
        let spring = place(Kind::Water, (10.3, 44.0), None);
        let feature = spring.feature(None);
        assert_eq!(feature["properties"]["searchable"], false);
        assert_eq!(feature["properties"]["name"], Value::Null);
        let peak = Place {
            tags: HashMap::from([
                ("name".to_owned(), "Pania della Croce".to_owned()),
                ("ele".to_owned(), "1859 m".to_owned()),
            ]),
            ..place(Kind::Peak, (10.324, 44.034), None)
        };
        let feature = peak.feature(None);
        assert_eq!(feature["properties"]["searchable"], true);
        assert_eq!(feature["properties"]["ele"], 1859.0);
        assert_eq!(feature["properties"]["ele_source"], "osm");
        assert_eq!(feature["id"], "node/1");
    }
}
