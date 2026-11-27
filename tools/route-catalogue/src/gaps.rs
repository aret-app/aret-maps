//! Trail gaps: marked trails that OpenStreetMap has as paths but not as hiking route relations,
//! so the catalogue (ADR-0015) and the tours built on it can't see them.
//!
//! ```text
//! route-catalogue trail-gaps <input.osm.pbf> <west,south,east,north> <output-dir>
//! ```
//!
//! Writes `trail-gaps.md` (totals and the longest missing trails, with links to fix them in OSM) and
//! `trail-gaps.geojson` (every missing trail, to look at on a map). Lengths count only the part
//! inside the area.
//!
//! A path counts as **marked** when its tags say so: a waymarking symbol (`osmc:symbol`), a CAI
//! grade (`cai_scale`), `trailblazed=yes`, or a trail number (`ref`) on a path, footway, bridleway
//! or steps. A path with only a SAC grade (`sac_scale`) is counted apart: graded, but not
//! necessarily marked.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use geo::{Distance, Geodesic, Point};
use osmpbf::{Element, ElementReader, RelMemberType};
use serde_json::{Value, json};

use crate::catalogue::BoundingBox;

/// Paths and tracks a hiking trail can follow.
const TRAIL_HIGHWAYS: &[&str] = &["path", "footway", "bridleway", "steps", "track"];
/// Where a `ref` alone means a trail number (on a track it's often a forest road's number).
const REF_HIGHWAYS: &[&str] = &["path", "footway", "bridleway", "steps"];
/// Missing trails listed in the report; the GeoJSON has all of them.
const REPORT_TOP: usize = 60;

/// Tags kept from each missing way.
const KEPT_TAGS: &[&str] = &[
    "highway",
    "ref",
    "name",
    "osmc:symbol",
    "cai_scale",
    "sac_scale",
    "operator",
    "trailblazed",
];

/// Kilometres by category, inside the area.
#[derive(Debug, Default, Clone, PartialEq)]
struct Totals {
    /// Ways in hiking route relations (any highway): what the catalogue uses.
    in_hiking_relations: f64,
    /// Trail ways in `route=foot` relations only.
    in_foot_relations: f64,
    /// Marked trail ways in no route relation.
    marked_missing: f64,
    /// Of those, by the evidence they carry (a way can have several).
    with_osmc: f64,
    with_cai_scale: f64,
    with_ref: f64,
    with_trailblazed: f64,
    /// Trail ways in no relation, unmarked but with a SAC grade.
    graded_only: f64,
    /// Other paths, footways, bridleways and steps in no relation.
    other_paths: f64,
    /// Other tracks in no relation.
    other_tracks: f64,
}

impl Totals {
    fn merge(mut self, other: Self) -> Self {
        self.in_hiking_relations += other.in_hiking_relations;
        self.in_foot_relations += other.in_foot_relations;
        self.marked_missing += other.marked_missing;
        self.with_osmc += other.with_osmc;
        self.with_cai_scale += other.with_cai_scale;
        self.with_ref += other.with_ref;
        self.with_trailblazed += other.with_trailblazed;
        self.graded_only += other.graded_only;
        self.other_paths += other.other_paths;
        self.other_tracks += other.other_tracks;
        self
    }
}

/// A marked way in no route relation.
#[derive(Debug, Clone, PartialEq)]
struct MissingWay {
    id: i64,
    tags: HashMap<String, String>,
    nodes: Vec<i64>,
    /// Pieces inside the area, (longitude, latitude).
    lines: Vec<Vec<(f64, f64)>>,
    length_m: f64,
}

/// Connected missing ways with the same trail number (or none): one trail to add to OSM.
#[derive(Debug, Clone, PartialEq)]
struct MissingTrail {
    reference: Option<String>,
    names: Vec<String>,
    osmc: Vec<String>,
    grades: Vec<String>,
    way_ids: Vec<i64>,
    lines: Vec<Vec<(f64, f64)>>,
    length_m: f64,
}

/// Node positions inside the area, sorted by id (compact: a pilot region has millions).
struct Positions(Vec<(i64, f32, f32)>);

impl Positions {
    fn get(&self, id: i64) -> Option<(f64, f64)> {
        let index = self.0.binary_search_by_key(&id, |&(n, _, _)| n).ok()?;
        let (_, lon, lat) = self.0[index];
        Some((f64::from(lon), f64::from(lat)))
    }
}

/// Measures the trail gaps of `area` in `input` and writes the report and GeoJSON to `output`.
pub fn run(
    input: &Path,
    area: &BoundingBox,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = std::time::Instant::now();
    eprintln!("reading route relations …");
    let (hiking, foot): (HashSet<i64>, HashSet<i64>) = ElementReader::from_path(input)?
        .par_map_reduce(
            |element| {
                let Element::Relation(relation) = element else {
                    return (HashSet::new(), HashSet::new());
                };
                let tags: HashMap<&str, &str> = relation.tags().collect();
                if tags.get("type") != Some(&"route") {
                    return (HashSet::new(), HashSet::new());
                }
                let ways: HashSet<i64> = relation
                    .members()
                    .filter(|member| member.member_type == RelMemberType::Way)
                    .map(|member| member.member_id)
                    .collect();
                match tags.get("route") {
                    Some(&"hiking") => (ways, HashSet::new()),
                    Some(&"foot") => (HashSet::new(), ways),
                    _ => (HashSet::new(), HashSet::new()),
                }
            },
            || (HashSet::new(), HashSet::new()),
            |(mut h1, mut f1), (h2, f2)| {
                h1.extend(h2);
                f1.extend(f2);
                (h1, f1)
            },
        )?;
    eprintln!(
        "  ways in hiking relations: {}, in foot relations: {} ({:.0} s)",
        hiking.len(),
        foot.len(),
        started.elapsed().as_secs_f64()
    );

    eprintln!("reading node positions inside the area …");
    // A small margin so a way crossing the edge keeps its first outside point for the check
    let mut positions: Vec<(i64, f32, f32)> = ElementReader::from_path(input)?.par_map_reduce(
        |element| {
            let (id, lon, lat) = match element {
                Element::DenseNode(node) => (node.id(), node.lon(), node.lat()),
                Element::Node(node) => (node.id(), node.lon(), node.lat()),
                _ => return Vec::new(),
            };
            if area.contains((lon, lat)) {
                vec![(id, lon as f32, lat as f32)]
            } else {
                Vec::new()
            }
        },
        Vec::new,
        concat,
    )?;
    positions.sort_unstable_by_key(|&(id, _, _)| id);
    let positions = Positions(positions);
    eprintln!(
        "  {} nodes ({:.0} s)",
        positions.0.len(),
        started.elapsed().as_secs_f64()
    );

    eprintln!("measuring ways …");
    let (totals, missing): (Totals, Vec<MissingWay>) = ElementReader::from_path(input)?
        .par_map_reduce(
            |element| {
                let Element::Way(way) = element else {
                    return (Totals::default(), Vec::new());
                };
                measure_way(&way, &hiking, &foot, &positions)
            },
            || (Totals::default(), Vec::new()),
            |(t1, m1), (t2, m2)| (t1.merge(t2), concat(m1, m2)),
        )?;
    let trails = group(missing);
    eprintln!(
        "  {:.0} km marked but in no relation, in {} trails ({:.0} s)",
        totals.marked_missing / 1000.0,
        trails.len(),
        started.elapsed().as_secs_f64()
    );

    std::fs::create_dir_all(output)?;
    std::fs::write(output.join("trail-gaps.md"), report(&totals, &trails, area))?;
    std::fs::write(
        output.join("trail-gaps.geojson"),
        serde_json::to_vec(&to_geojson(&trails))?,
    )?;
    eprintln!(
        "wrote {} and trail-gaps.geojson",
        output.join("trail-gaps.md").display()
    );
    Ok(())
}

/// The way's contribution to the totals, and the way itself if it's a missing marked trail.
fn measure_way(
    way: &osmpbf::Way<'_>,
    hiking: &HashSet<i64>,
    foot: &HashSet<i64>,
    positions: &Positions,
) -> (Totals, Vec<MissingWay>) {
    let mut totals = Totals::default();
    let tags: HashMap<&str, &str> = way.tags().collect();
    let highway = tags.get("highway").copied().unwrap_or_default();
    let in_hiking = hiking.contains(&way.id());
    let is_trail = TRAIL_HIGHWAYS.contains(&highway);
    if !in_hiking && !is_trail {
        return (totals, Vec::new());
    }

    // Pieces inside the area: consecutive nodes that both have a position
    let nodes: Vec<i64> = way.refs().collect();
    let mut lines: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    for &node in &nodes {
        match positions.get(node) {
            Some(point) => current.push(point),
            None => {
                if current.len() >= 2 {
                    lines.push(std::mem::take(&mut current));
                }
                current.clear();
            }
        }
    }
    if current.len() >= 2 {
        lines.push(current);
    }
    let length_m: f64 = lines
        .iter()
        .flat_map(|line| line.windows(2))
        .map(|pair| Geodesic.distance(Point::from(pair[0]), Point::from(pair[1])))
        .sum();
    if length_m <= 0.0 {
        return (totals, Vec::new());
    }

    if in_hiking {
        totals.in_hiking_relations = length_m;
        return (totals, Vec::new());
    }
    if foot.contains(&way.id()) {
        totals.in_foot_relations = length_m;
        return (totals, Vec::new());
    }
    let osmc = tags.contains_key("osmc:symbol");
    let cai_scale = tags.contains_key("cai_scale");
    let reference = tags.contains_key("ref") && REF_HIGHWAYS.contains(&highway);
    let trailblazed = tags.get("trailblazed") == Some(&"yes");
    if osmc || cai_scale || reference || trailblazed {
        totals.marked_missing = length_m;
        let share = |has: bool| if has { length_m } else { 0.0 };
        totals.with_osmc = share(osmc);
        totals.with_cai_scale = share(cai_scale);
        totals.with_ref = share(reference);
        totals.with_trailblazed = share(trailblazed);
        let kept = tags
            .iter()
            .filter(|(key, _)| KEPT_TAGS.contains(key))
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        let missing = MissingWay {
            id: way.id(),
            tags: kept,
            nodes,
            lines,
            length_m,
        };
        return (totals, vec![missing]);
    }
    if tags.contains_key("sac_scale") {
        totals.graded_only = length_m;
    } else if highway == "track" {
        totals.other_tracks = length_m;
    } else {
        totals.other_paths = length_m;
    }
    (totals, Vec::new())
}

/// Joins missing ways that share a node and have the same trail number (or both none).
fn group(ways: Vec<MissingWay>) -> Vec<MissingTrail> {
    let key = |way: &MissingWay| way.tags.get("ref").cloned().unwrap_or_default();
    let mut parent: Vec<usize> = (0..ways.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut first_at: HashMap<(String, i64), usize> = HashMap::new();
    for (index, way) in ways.iter().enumerate() {
        for &node in &way.nodes {
            match first_at.get(&(key(way), node)) {
                Some(&other) => {
                    let (a, b) = (root(&mut parent, index), root(&mut parent, other));
                    parent[a] = b;
                }
                None => {
                    first_at.insert((key(way), node), index);
                }
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for index in 0..ways.len() {
        groups
            .entry(root(&mut parent, index))
            .or_default()
            .push(index);
    }
    let mut trails: Vec<MissingTrail> = groups
        .into_values()
        .map(|members| {
            let distinct = |tag: &str| -> Vec<String> {
                let mut values: Vec<String> = members
                    .iter()
                    .filter_map(|&i| ways[i].tags.get(tag).cloned())
                    .collect();
                values.sort();
                values.dedup();
                values
            };
            let mut grades = distinct("cai_scale");
            grades.extend(distinct("sac_scale"));
            MissingTrail {
                reference: distinct("ref").into_iter().next(),
                names: distinct("name"),
                osmc: distinct("osmc:symbol"),
                grades,
                way_ids: members.iter().map(|&i| ways[i].id).collect(),
                lines: members
                    .iter()
                    .flat_map(|&i| ways[i].lines.iter().cloned())
                    .collect(),
                length_m: members.iter().map(|&i| ways[i].length_m).sum(),
            }
        })
        .collect();
    trails.sort_by(|a, b| b.length_m.total_cmp(&a.length_m));
    trails
}

fn km(metres: f64) -> String {
    format!("{:.0} km", metres / 1000.0)
}

fn report(totals: &Totals, trails: &[MissingTrail], area: &BoundingBox) -> String {
    let mut out = String::new();
    let marked_total = totals.in_hiking_relations + totals.marked_missing;
    let share = |part: f64| {
        if marked_total > 0.0 {
            part / marked_total * 100.0
        } else {
            0.0
        }
    };
    let _ = writeln!(out, "# Trail gaps\n");
    let _ = writeln!(
        out,
        "Area {},{},{},{}. Marked trails that OpenStreetMap has as paths but not in any hiking route \
         relation: the catalogue and the tours can't use them (ADR-0015). Lengths inside the area.\n",
        area.west, area.south, area.east, area.north
    );
    let _ = writeln!(out, "| | Length | Share of marked trails |");
    let _ = writeln!(out, "|---|---:|---:|");
    let _ = writeln!(
        out,
        "| In hiking route relations (used now) | {} | {:.0}% |",
        km(totals.in_hiking_relations),
        share(totals.in_hiking_relations)
    );
    let _ = writeln!(
        out,
        "| **Marked, in no relation (missing)** | **{}** | **{:.0}%** |",
        km(totals.marked_missing),
        share(totals.marked_missing)
    );
    let _ = writeln!(
        out,
        "| … with a waymark symbol (`osmc:symbol`) | {} | |",
        km(totals.with_osmc)
    );
    let _ = writeln!(
        out,
        "| … with a CAI grade (`cai_scale`) | {} | |",
        km(totals.with_cai_scale)
    );
    let _ = writeln!(
        out,
        "| … with a trail number (`ref`) on a path | {} | |",
        km(totals.with_ref)
    );
    let _ = writeln!(
        out,
        "| … with `trailblazed=yes` | {} | |",
        km(totals.with_trailblazed)
    );
    let _ = writeln!(out, "\nFor scale (not counted as marked trails):\n");
    let _ = writeln!(out, "| | Length |");
    let _ = writeln!(out, "|---|---:|");
    let _ = writeln!(
        out,
        "| Trails in `route=foot` relations only | {} |",
        km(totals.in_foot_relations)
    );
    let _ = writeln!(
        out,
        "| Unmarked paths with a SAC grade | {} |",
        km(totals.graded_only)
    );
    let _ = writeln!(
        out,
        "| Other paths, footways, steps | {} |",
        km(totals.other_paths)
    );
    let _ = writeln!(out, "| Other tracks | {} |", km(totals.other_tracks));

    let long = trails.iter().filter(|t| t.length_m >= 1000.0).count();
    let _ = writeln!(
        out,
        "\n## Longest missing trails\n\n{} missing trails in all ({} of 1 km or more); connected ways with \
         the same number count as one. All of them are in `trail-gaps.geojson`.\n",
        trails.len(),
        long
    );
    let _ = writeln!(out, "| Number | Name | Length | Grade | Symbol | Where |");
    let _ = writeln!(out, "|---|---|---:|---|---|---|");
    for trail in trails.iter().take(REPORT_TOP) {
        let (lon, lat) = trail
            .lines
            .first()
            .and_then(|line| line.get(line.len() / 2))
            .copied()
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "| {} | {} | {:.1} km | {} | {} | [map](https://www.openstreetmap.org/?mlat={lat:.5}&mlon={lon:.5}#map=15/{lat:.5}/{lon:.5}), [way](https://www.openstreetmap.org/way/{}) |",
            trail.reference.as_deref().unwrap_or("–"),
            trail.names.join(" / ").replace('|', "/"),
            trail.length_m / 1000.0,
            trail.grades.join(", "),
            trail.osmc.first().map_or("", String::as_str),
            trail.way_ids[0],
        );
    }
    out
}

fn to_geojson(trails: &[MissingTrail]) -> Value {
    let round = |v: f64| (v * 1e6).round() / 1e6;
    let features: Vec<Value> = trails
        .iter()
        .map(|trail| {
            let coordinates: Vec<Vec<[f64; 2]>> = trail
                .lines
                .iter()
                .map(|line| line.iter().map(|&(x, y)| [round(x), round(y)]).collect())
                .collect();
            json!({
                "type": "Feature",
                "properties": {
                    "ref": trail.reference,
                    "name": trail.names.first(),
                    "names": trail.names,
                    "osmc_symbol": trail.osmc.first(),
                    "grades": trail.grades,
                    "length_m": trail.length_m.round(),
                    "ways": trail.way_ids.len(),
                    "way_ids": trail.way_ids,
                },
                "geometry": { "type": "MultiLineString", "coordinates": coordinates },
            })
        })
        .collect();
    json!({
        "type": "FeatureCollection",
        "attribution": "© OpenStreetMap contributors",
        "license": "ODbL-1.0",
        "features": features,
    })
}

fn concat<T>(mut a: Vec<T>, mut b: Vec<T>) -> Vec<T> {
    if a.len() < b.len() {
        std::mem::swap(&mut a, &mut b);
    }
    a.append(&mut b);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn way(id: i64, reference: Option<&str>, nodes: &[i64]) -> MissingWay {
        let mut tags = HashMap::new();
        if let Some(r) = reference {
            tags.insert("ref".to_owned(), r.to_owned());
        }
        MissingWay {
            id,
            tags,
            nodes: nodes.to_vec(),
            lines: vec![vec![(10.0, 44.0), (10.001, 44.0)]],
            length_m: 80.0,
        }
    }

    #[test]
    fn joins_connected_ways_with_the_same_number_only() {
        let trails = group(vec![
            way(1, Some("7"), &[1, 2]),
            way(2, Some("7"), &[2, 3]),
            way(3, Some("8"), &[3, 4]), // touches trail 7 but has another number
            way(4, Some("7"), &[10, 11]), // same number, not connected
        ]);
        let mut found: Vec<(Option<String>, Vec<i64>)> = trails
            .iter()
            .map(|t| {
                let mut ids = t.way_ids.clone();
                ids.sort_unstable();
                (t.reference.clone(), ids)
            })
            .collect();
        found.sort();
        assert_eq!(
            found,
            vec![
                (Some("7".to_owned()), vec![1, 2]),
                (Some("7".to_owned()), vec![4]),
                (Some("8".to_owned()), vec![3]),
            ]
        );
        assert_eq!(trails[0].length_m, 160.0, "longest first");
    }
}
