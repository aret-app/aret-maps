//! Reads hiking route relations, their ways and their nodes from an OpenStreetMap PBF file.
//!
//! Three parallel passes over the file (relations, then the ways they use, then those ways' nodes),
//! keeping only what the routes need: a 2 GB country extract fits in a few hundred MB of memory.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use osmpbf::{Element, ElementReader, RelMemberType};

use crate::places::{self, Place};

/// OpenStreetMap tags the catalogue keeps from each route relation.
pub const KEPT_TAGS: &[&str] = &[
    "name",
    "name:it",
    "ref",
    "network",
    "operator",
    "cai_scale",
    "sac_scale",
    "from",
    "to",
    "osmc:symbol",
    // What mappers recorded, used only to check our own numbers (aret-maps#5)
    "ascent",
    "descent",
    "duration",
];

/// Node ids of each way, by way id.
pub type WayNodes = HashMap<i64, Vec<i64>>;
/// Position of each node, (longitude, latitude), by node id.
pub type NodePositions = HashMap<i64, (f64, f64)>;

/// Result of the node pass: positions of the wanted nodes, and the places mapped as nodes.
type NodesAndPlaces = (Vec<(i64, (f64, f64))>, Vec<Place>);
/// Result of the way pass: route member ways, and places mapped as ways (with their nodes).
type WaysAndPlaces = (Vec<(i64, Vec<i64>)>, Vec<(Place, Vec<i64>)>);

/// A `type=route` + `route=hiking` relation.
#[derive(Debug, Clone, PartialEq)]
pub struct RouteRelation {
    /// OpenStreetMap relation id.
    pub id: i64,
    /// The tags in [`KEPT_TAGS`] that are present.
    pub tags: HashMap<String, String>,
    /// Member way ids, in member order (guideposts and other node members are skipped).
    pub way_ids: Vec<i64>,
    /// Whether it has relation members (a "super-route" made of sub-routes).
    pub has_sub_routes: bool,
}

/// Everything needed to assemble the routes.
pub struct RouteData {
    /// All hiking route relations in the file.
    pub relations: Vec<RouteRelation>,
    /// Node ids of each member way.
    pub ways: WayNodes,
    /// Position of each node of those ways.
    pub nodes: NodePositions,
    /// Every place (peak, pass, hut, water, trailhead, car park, settlement) in the file.
    pub places: Vec<Place>,
}

/// Reads all hiking route relations of `path` with their ways and nodes.
pub fn read(path: &Path) -> Result<RouteData, osmpbf::Error> {
    let relations: Vec<RouteRelation> = ElementReader::from_path(path)?.par_map_reduce(
        |element| match element {
            Element::Relation(relation) => hiking_route(&relation).into_iter().collect(),
            _ => Vec::new(),
        },
        Vec::new,
        concat,
    )?;

    let wanted_ways: HashSet<i64> = relations
        .iter()
        .flat_map(|relation| relation.way_ids.iter().copied())
        .collect();
    let (route_ways, way_places): WaysAndPlaces = ElementReader::from_path(path)?.par_map_reduce(
        |element| {
            let Element::Way(way) = element else {
                return (Vec::new(), Vec::new());
            };
            let route_way = if wanted_ways.contains(&way.id()) {
                vec![(way.id(), way.refs().collect::<Vec<i64>>())]
            } else {
                Vec::new()
            };
            let tags: Vec<(&str, &str)> = way.tags().collect();
            let place = place("way", way.id(), (0.0, 0.0), &tags)
                .map(|place| (place, way.refs().collect::<Vec<i64>>()));
            (route_way, place.into_iter().collect())
        },
        || (Vec::new(), Vec::new()),
        |(a1, b1), (a2, b2)| (concat(a1, a2), concat(b1, b2)),
    )?;
    let ways: WayNodes = route_ways.into_iter().collect();

    let wanted_nodes: HashSet<i64> = ways
        .values()
        .flatten()
        .chain(way_places.iter().flat_map(|(_, refs)| refs.iter()))
        .copied()
        .collect();
    let (node_list, mut places): NodesAndPlaces = ElementReader::from_path(path)?.par_map_reduce(
        |element| {
            let (id, position, tags): (i64, (f64, f64), Vec<(&str, &str)>) = match element {
                Element::DenseNode(node) => {
                    (node.id(), (node.lon(), node.lat()), node.tags().collect())
                }
                Element::Node(node) => (node.id(), (node.lon(), node.lat()), node.tags().collect()),
                _ => return (Vec::new(), Vec::new()),
            };
            let wanted = if wanted_nodes.contains(&id) {
                vec![(id, position)]
            } else {
                Vec::new()
            };
            (
                wanted,
                place("node", id, position, &tags).into_iter().collect(),
            )
        },
        || (Vec::new(), Vec::new()),
        |(a1, b1), (a2, b2)| (concat(a1, a2), concat(b1, b2)),
    )?;
    let nodes: NodePositions = node_list.into_iter().collect();

    // Places mapped as ways (huts as buildings, car parks as areas): the centre of their nodes
    for (mut place, refs) in way_places {
        let points: Vec<(f64, f64)> = refs
            .iter()
            .filter_map(|node| nodes.get(node).copied())
            .collect();
        if points.is_empty() {
            continue;
        }
        let n = points.len() as f64;
        place.position = (
            points.iter().map(|p| p.0).sum::<f64>() / n,
            points.iter().map(|p| p.1).sum::<f64>() / n,
        );
        places.push(place);
    }

    Ok(RouteData {
        relations,
        ways,
        nodes,
        places,
    })
}

/// The relation as a [`RouteRelation`] if it is a hiking route.
fn hiking_route(relation: &osmpbf::Relation<'_>) -> Option<RouteRelation> {
    let mut is_route = false;
    let mut is_hiking = false;
    let mut tags = HashMap::new();
    for (key, value) in relation.tags() {
        match key {
            "type" => is_route = value == "route",
            "route" => is_hiking = value == "hiking",
            _ if KEPT_TAGS.contains(&key) => {
                tags.insert(key.to_owned(), value.to_owned());
            }
            _ => {}
        }
    }
    if !(is_route && is_hiking) {
        return None;
    }
    let mut way_ids = Vec::new();
    let mut has_sub_routes = false;
    for member in relation.members() {
        match member.member_type {
            RelMemberType::Way => way_ids.push(member.member_id),
            RelMemberType::Relation => has_sub_routes = true,
            RelMemberType::Node => {}
        }
    }
    Some(RouteRelation {
        id: relation.id(),
        tags,
        way_ids,
        has_sub_routes,
    })
}

/// When the OpenStreetMap data in the file was current (header `osmosis_replication_timestamp`),
/// in seconds since 1970, if the file says.
pub fn extract_timestamp(path: &Path) -> Option<i64> {
    let mut blobs = osmpbf::BlobReader::from_path(path).ok()?;
    let header = blobs.next()?.ok()?.to_headerblock().ok()?;
    header.osmosis_replication_timestamp()
}

/// The feature as a [`Place`] if its tags make it one.
fn place(
    osm_type: &'static str,
    id: i64,
    position: (f64, f64),
    tags: &[(&str, &str)],
) -> Option<Place> {
    let (kind, subkind) = places::classify(tags)?;
    let kept = tags
        .iter()
        .filter(|(key, _)| places::KEPT_TAGS.contains(key))
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    Some(Place {
        osm_type,
        id,
        kind,
        subkind,
        tags: kept,
        position,
    })
}

fn concat<T>(mut a: Vec<T>, mut b: Vec<T>) -> Vec<T> {
    if a.len() < b.len() {
        std::mem::swap(&mut a, &mut b);
    }
    a.append(&mut b);
    a
}
