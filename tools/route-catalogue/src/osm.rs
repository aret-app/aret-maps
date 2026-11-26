//! Reads hiking route relations, their ways and their nodes from an OpenStreetMap PBF file.
//!
//! Three parallel passes over the file (relations, then the ways they use, then those ways' nodes),
//! keeping only what the routes need: a 2 GB country extract fits in a few hundred MB of memory.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use osmpbf::{Element, ElementReader, RelMemberType};

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
];

/// Node ids of each way, by way id.
pub type WayNodes = HashMap<i64, Vec<i64>>;
/// Position of each node, (longitude, latitude), by node id.
pub type NodePositions = HashMap<i64, (f64, f64)>;

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
    let ways: WayNodes = ElementReader::from_path(path)?
        .par_map_reduce(
            |element| match element {
                Element::Way(way) if wanted_ways.contains(&way.id()) => {
                    vec![(way.id(), way.refs().collect::<Vec<i64>>())]
                }
                _ => Vec::new(),
            },
            Vec::new,
            concat,
        )?
        .into_iter()
        .collect();

    let wanted_nodes: HashSet<i64> = ways.values().flatten().copied().collect();
    let nodes: NodePositions = ElementReader::from_path(path)?
        .par_map_reduce(
            |element| match element {
                Element::DenseNode(node) if wanted_nodes.contains(&node.id()) => {
                    vec![(node.id(), (node.lon(), node.lat()))]
                }
                Element::Node(node) if wanted_nodes.contains(&node.id()) => {
                    vec![(node.id(), (node.lon(), node.lat()))]
                }
                _ => Vec::new(),
            },
            Vec::new,
            concat,
        )?
        .into_iter()
        .collect();

    Ok(RouteData {
        relations,
        ways,
        nodes,
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

fn concat<T>(mut a: Vec<T>, mut b: Vec<T>) -> Vec<T> {
    if a.len() < b.len() {
        std::mem::swap(&mut a, &mut b);
    }
    a.append(&mut b);
    a
}
