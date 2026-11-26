//! Turns a route relation's member ways into lines, without ever bridging a gap.
//!
//! Mappers add ways to a relation in any order and direction, so the member order can't be
//! trusted. Instead:
//! - **pieces**: ways touching at any node belong to the same piece. One piece means the route is
//!   connected; more means it has gaps (the relation is broken in OpenStreetMap, or not finished).
//! - **lines**: ways are joined end to end wherever exactly two ways meet, like PostGIS
//!   `ST_LineMerge`. Junctions (a branch, a variant) and gaps end a line.
//!
//! Nothing is invented: every output point is a point of a member way, and two lines are never
//! connected where the ways themselves don't touch (ADR-0015: a false shortcut on a mountain is a
//! safety risk).

use std::collections::{HashMap, HashSet};

/// OpenStreetMap node id.
pub type NodeId = i64;

/// A member way: its id and its nodes in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Way {
    /// OpenStreetMap way id.
    pub id: i64,
    /// Node ids, in the way's own direction.
    pub nodes: Vec<NodeId>,
}

/// The assembled geometry of one route, as node ids.
#[derive(Debug, Clone, PartialEq)]
pub struct Assembly {
    /// Merged lines; a connected route without junctions has exactly one.
    pub lines: Vec<Vec<NodeId>>,
    /// Number of disconnected pieces; 1 means no gaps, 0 means no usable ways.
    pub pieces: usize,
}

/// Assembles `ways` (in member order; duplicates and ways with fewer than two nodes are ignored).
pub fn assemble(ways: &[Way]) -> Assembly {
    let mut seen_ids = HashSet::new();
    let ways: Vec<&Way> = ways
        .iter()
        .filter(|way| way.nodes.len() >= 2 && seen_ids.insert(way.id))
        .collect();
    Assembly {
        lines: merge_lines(&ways),
        pieces: count_pieces(&ways),
    }
}

/// Number of groups of ways connected through shared nodes (union-find).
fn count_pieces(ways: &[&Way]) -> usize {
    let mut parent: Vec<usize> = (0..ways.len()).collect();
    fn root(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    let mut first_way_at: HashMap<NodeId, usize> = HashMap::new();
    for (index, way) in ways.iter().enumerate() {
        for &node in &way.nodes {
            let other = *first_way_at.entry(node).or_insert(index);
            let (a, b) = (root(&mut parent, index), root(&mut parent, other));
            if a != b {
                parent[a] = b;
            }
        }
    }
    (0..ways.len())
        .filter(|&index| root(&mut parent, index) == index)
        .count()
}

/// Joins ways end to end through nodes where exactly two way ends meet.
fn merge_lines(ways: &[&Way]) -> Vec<Vec<NodeId>> {
    // Way ends at each node: (way index, whether it's the way's first node)
    let mut ends: HashMap<NodeId, Vec<usize>> = HashMap::new();
    for (index, way) in ways.iter().enumerate() {
        for node in [way.nodes[0], way.nodes[way.nodes.len() - 1]] {
            ends.entry(node).or_default().push(index);
        }
    }
    // The other way ending at `node`, if exactly two way ends meet there
    let continuation = |node: NodeId, current: usize| -> Option<usize> {
        match ends.get(&node).map(Vec::as_slice) {
            Some([a, b]) if *a == current && *b != current => Some(*b),
            Some([a, b]) if *b == current && *a != current => Some(*a),
            _ => None,
        }
    };

    let mut used = vec![false; ways.len()];
    let mut lines = Vec::new();
    for start in 0..ways.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut line: Vec<NodeId> = ways[start].nodes.clone();

        // Forward from the line's last node
        let mut current = start;
        while let Some(&end) = line.last() {
            let Some(next) = continuation(end, current).filter(|&next| !used[next]) else {
                break;
            };
            used[next] = true;
            line.extend(oriented_from(&ways[next].nodes, end).skip(1));
            current = next;
        }
        // Backward from the line's first node
        let mut current = start;
        while let Some(&begin) = line.first() {
            let Some(previous) = continuation(begin, current).filter(|&prev| !used[prev]) else {
                break;
            };
            used[previous] = true;
            let mut prefix: Vec<NodeId> = oriented_from(&ways[previous].nodes, begin).collect();
            prefix.reverse();
            prefix.pop(); // `begin` is already the line's first node
            prefix.extend(line);
            line = prefix;
            current = previous;
        }
        lines.push(line);
    }
    lines
}

/// The way's nodes starting at `node`, which must be one of its two ends.
fn oriented_from(nodes: &[NodeId], node: NodeId) -> Box<dyn Iterator<Item = NodeId> + '_> {
    if nodes[0] == node {
        Box::new(nodes.iter().copied())
    } else {
        Box::new(nodes.iter().rev().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn way(id: i64, nodes: &[NodeId]) -> Way {
        Way {
            id,
            nodes: nodes.to_vec(),
        }
    }

    #[test]
    fn joins_ways_in_order() {
        let result = assemble(&[way(1, &[1, 2, 3]), way(2, &[3, 4]), way(3, &[4, 5, 6])]);
        assert_eq!(result.lines, vec![vec![1, 2, 3, 4, 5, 6]]);
        assert_eq!(result.pieces, 1);
    }

    #[test]
    fn handles_reversed_and_shuffled_members() {
        // Members listed out of order, and way 2 drawn in the opposite direction
        let result = assemble(&[way(3, &[4, 5, 6]), way(1, &[1, 2, 3]), way(2, &[4, 3])]);
        assert_eq!(result.lines, vec![vec![1, 2, 3, 4, 5, 6]]);
        assert_eq!(result.pieces, 1);
    }

    #[test]
    fn never_bridges_a_gap() {
        // Node 3 and node 10 may be 50 m apart, but the ways don't touch: two pieces
        let result = assemble(&[way(1, &[1, 2, 3]), way(2, &[10, 11, 12])]);
        assert_eq!(result.lines, vec![vec![1, 2, 3], vec![10, 11, 12]]);
        assert_eq!(result.pieces, 2);
    }

    #[test]
    fn a_branch_is_one_piece_with_several_lines() {
        // Main line 1-2-3 and a variant from node 2 to 20: connected, but not one line
        let result = assemble(&[way(1, &[1, 2]), way(2, &[2, 3]), way(3, &[2, 20])]);
        assert_eq!(result.pieces, 1);
        assert_eq!(result.lines.len(), 3);
    }

    #[test]
    fn a_way_touching_another_in_the_middle_is_connected() {
        // Way 2 starts at node 2, an inner node of way 1 (common for a spur to a summit)
        let result = assemble(&[way(1, &[1, 2, 3]), way(2, &[2, 30])]);
        assert_eq!(result.pieces, 1);
        assert_eq!(result.lines.len(), 2);
    }

    #[test]
    fn closes_a_loop() {
        let result = assemble(&[way(1, &[1, 2, 3]), way(2, &[3, 4, 1])]);
        assert_eq!(result.pieces, 1);
        assert_eq!(result.lines.len(), 1);
        let line = &result.lines[0];
        assert_eq!(line.first(), line.last(), "a loop ends where it starts");
        assert_eq!(line.len(), 5);
    }

    #[test]
    fn ignores_duplicate_and_degenerate_ways() {
        let result = assemble(&[
            way(1, &[1, 2]),
            way(1, &[1, 2]),
            way(2, &[7]),
            way(3, &[2, 3]),
        ]);
        assert_eq!(result.lines, vec![vec![1, 2, 3]]);
        assert_eq!(result.pieces, 1);
    }

    #[test]
    fn no_ways_no_pieces() {
        let result = assemble(&[]);
        assert!(result.lines.is_empty());
        assert_eq!(result.pieces, 0);
    }
}
