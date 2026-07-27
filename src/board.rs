//! Board topology: an abstract directed graph of spaces, with no coordinates
//! or literal shape. A standard ring-plus-home-lanes board (the classic
//! *Mens erger je niet* / Ludo layout) is just one constructor over this
//! graph, not a special case baked into the types themselves.

use std::collections::HashMap;

/// Identifies a single space on the board.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SpaceId(pub u32);

/// Identifies one player's color, and by extension which pawns, yard
/// spaces, and home lane belong to them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PlayerColor(pub u8);

/// What role a space plays in a pawn's journey around the board.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpaceKind {
    Yard,
    Shared,
    HomeLane,
    Finish,
}

/// A directed connection from one space to another.
#[derive(Clone, Debug)]
pub struct Edge {
    pub to: SpaceId,
    /// `None` means any pawn may take this edge; `Some(color)` restricts it
    /// to that color, which is how a shared space can fork into one color's
    /// private home lane without affecting anyone else passing through.
    pub restricted_to: Option<PlayerColor>,
    /// If this edge is eligible for a color, every other edge from the same
    /// node is ignored for that color, even ones that would otherwise also
    /// be eligible.
    pub forced: bool,
}

/// One node in the board graph.
#[derive(Clone, Debug)]
pub struct SpaceNode {
    pub id: SpaceId,
    pub kind: SpaceKind,
    pub owner: Option<PlayerColor>,
    pub safe: bool,
    pub edges: Vec<Edge>,
}

/// What follows a given space for a given color: exactly one space, a
/// choice between several, or nowhere.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NextSpace {
    Single(SpaceId),
    Branch(Vec<SpaceId>),
    DeadEnd,
}

/// The board as a directed graph of spaces.
#[derive(Clone, Debug)]
pub struct BoardTopology {
    nodes: Vec<SpaceNode>,
    yard_spaces: HashMap<PlayerColor, Vec<SpaceId>>,
}

impl BoardTopology {
    /// The space with the given id.
    pub fn node(&self, id: SpaceId) -> &SpaceNode {
        &self.nodes[id.0 as usize]
    }

    /// The yard spaces belonging to a color, empty if that color has none.
    pub fn yard_spaces(&self, color: PlayerColor) -> &[SpaceId] {
        self.yard_spaces
            .get(&color)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// What a pawn of `owner`'s color may move to next from `from`.
    pub fn next_space(&self, from: SpaceId, owner: PlayerColor) -> NextSpace {
        let eligible = |e: &&Edge| e.restricted_to.is_none() || e.restricted_to == Some(owner);
        let node = self.node(from);
        if let Some(forced) = node.edges.iter().find(|e| e.forced && eligible(e)) {
            return NextSpace::Single(forced.to);
        }
        let candidates: Vec<SpaceId> = node.edges.iter().filter(eligible).map(|e| e.to).collect();
        match candidates.len() {
            0 => NextSpace::DeadEnd,
            1 => NextSpace::Single(candidates[0]),
            _ => NextSpace::Branch(candidates),
        }
    }

    /// Builds a symmetric ring-plus-home-lanes board: `num_players` colors
    /// evenly spaced around a shared ring of `ring_len` spaces, each with
    /// `pawns_per_player` yard spaces and a private home lane of
    /// `home_lane_len` spaces leading to that color's Finish.
    ///
    /// Custom or asymmetric boards are built the same way this one is —
    /// this is one recipe over `SpaceNode`/`Edge`, not a variant the types
    /// themselves need to know about.
    pub fn standard_ring(
        num_players: u8,
        ring_len: u16,
        home_lane_len: u16,
        pawns_per_player: u8,
    ) -> Self {
        assert!(num_players > 0, "a board needs at least one player");
        assert!(
            ring_len >= num_players as u16,
            "ring must fit at least one entry point per player"
        );

        fn push_node(
            nodes: &mut Vec<SpaceNode>,
            kind: SpaceKind,
            owner: Option<PlayerColor>,
            safe: bool,
        ) -> SpaceId {
            let id = SpaceId(nodes.len() as u32);
            nodes.push(SpaceNode {
                id,
                kind,
                owner,
                safe,
                edges: Vec::new(),
            });
            id
        }

        let mut nodes: Vec<SpaceNode> = Vec::new();
        let mut yard_spaces: HashMap<PlayerColor, Vec<SpaceId>> = HashMap::new();
        for p in 0..num_players {
            let color = PlayerColor(p);
            let slots = (0..pawns_per_player)
                .map(|_| push_node(&mut nodes, SpaceKind::Yard, Some(color), true))
                .collect();
            yard_spaces.insert(color, slots);
        }

        // Laid out as one contiguous block so entry/fork points are simple
        // modular offsets from a single starting id.
        let ring_start = push_node(&mut nodes, SpaceKind::Shared, None, false);
        for _ in 1..ring_len {
            push_node(&mut nodes, SpaceKind::Shared, None, false);
        }
        let ring_space = |offset: u16| SpaceId(ring_start.0 + (offset % ring_len) as u32);

        // Evenly spaced entry points, one per color; the fork into a color's
        // home lane sits at the ring space just before its own entry point,
        // i.e. just before that color would otherwise loop around again.
        let spacing = ring_len / num_players as u16;
        let entry_of = |color: PlayerColor| ring_space(spacing * color.0 as u16);
        let fork_of = |color: PlayerColor| ring_space(spacing * color.0 as u16 + ring_len - 1);

        for offset in 0..ring_len {
            let from = ring_space(offset).0 as usize;
            nodes[from].edges.push(Edge {
                to: ring_space(offset + 1),
                restricted_to: None,
                forced: false,
            });
        }
        for p in 0..num_players {
            let color = PlayerColor(p);
            let idx = entry_of(color).0 as usize;
            nodes[idx].safe = true;
        }
        for p in 0..num_players {
            let color = PlayerColor(p);
            for &slot in &yard_spaces[&color] {
                let idx = slot.0 as usize;
                nodes[idx].edges.push(Edge {
                    to: entry_of(color),
                    restricted_to: None,
                    forced: false,
                });
            }
        }

        // Home lane + Finish per color, forked off the ring. Left as a real
        // `Branch` (not `forced`) at the fork itself, so that color may
        // still choose to loop the ring again instead.
        for p in 0..num_players {
            let color = PlayerColor(p);
            let mut prev = fork_of(color);
            for _ in 0..home_lane_len {
                let lane = push_node(&mut nodes, SpaceKind::HomeLane, Some(color), true);
                nodes[prev.0 as usize].edges.push(Edge {
                    to: lane,
                    restricted_to: Some(color),
                    forced: false,
                });
                prev = lane;
            }
            let finish = push_node(&mut nodes, SpaceKind::Finish, Some(color), true);
            nodes[prev.0 as usize].edges.push(Edge {
                to: finish,
                restricted_to: Some(color),
                forced: false,
            });
        }

        Self { nodes, yard_spaces }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classic_board() -> BoardTopology {
        BoardTopology::standard_ring(4, 40, 5, 4)
    }

    #[test]
    fn yard_exit_edge_leads_to_that_colors_entry_point() {
        let board = classic_board();
        for p in 0..4 {
            let color = PlayerColor(p);
            for &yard_slot in board.yard_spaces(color) {
                let NextSpace::Single(entry) = board.next_space(yard_slot, color) else {
                    panic!("yard exit should be a single unconditional edge");
                };
                assert_eq!(board.node(entry).kind, SpaceKind::Shared);
                assert!(
                    board.node(entry).safe,
                    "a color's entry point should be safe"
                );
            }
        }
    }

    #[test]
    fn every_color_has_its_own_yard_slots() {
        let board = classic_board();
        for p in 0..4 {
            assert_eq!(board.yard_spaces(PlayerColor(p)).len(), 4);
        }
        assert!(board.yard_spaces(PlayerColor(9)).is_empty());
    }

    #[test]
    fn ring_traversal_wraps_all_the_way_around() {
        let board = classic_board();
        let start = board.yard_spaces(PlayerColor(0))[0];
        let NextSpace::Single(mut here) = board.next_space(start, PlayerColor(0)) else {
            panic!("expected the yard exit to land on the ring");
        };
        // Walking 39 plain ring steps from color 0's entry point should land
        // exactly on the fork just before completing the loop.
        for _ in 0..39 {
            here = match board.next_space(here, PlayerColor(0)) {
                NextSpace::Single(next) => next,
                other => panic!("expected a single ring step, got {other:?}"),
            };
        }
        match board.next_space(here, PlayerColor(0)) {
            NextSpace::Branch(options) => {
                assert_eq!(options.len(), 2);
                assert!(options.contains(&start_entry(&board, 0)));
            }
            other => panic!("expected the fork to offer a branch, got {other:?}"),
        }
    }

    fn start_entry(board: &BoardTopology, color: u8) -> SpaceId {
        let NextSpace::Single(entry) =
            board.next_space(board.yard_spaces(PlayerColor(color))[0], PlayerColor(color))
        else {
            panic!("expected a single yard exit edge");
        };
        entry
    }

    #[test]
    fn fork_is_a_branch_for_the_owning_color_but_a_single_edge_for_others() {
        let board = classic_board();
        let entry0 = start_entry(&board, 0);
        // Walk backwards conceptually by walking forward 39 steps from entry0.
        let mut here = entry0;
        for _ in 0..39 {
            here = match board.next_space(here, PlayerColor(0)) {
                NextSpace::Single(next) => next,
                other => panic!("expected a single ring step, got {other:?}"),
            };
        }
        let fork = here;

        match board.next_space(fork, PlayerColor(0)) {
            NextSpace::Branch(options) => {
                assert_eq!(options.len(), 2);
                assert!(options.contains(&entry0));
            }
            other => panic!("owning color should see a branch, got {other:?}"),
        }
        match board.next_space(fork, PlayerColor(1)) {
            NextSpace::Single(next) => assert_eq!(next, entry0),
            other => panic!("other colors should see only the ring continuing, got {other:?}"),
        }
    }

    #[test]
    fn home_lane_chain_ends_at_a_dead_end_finish() {
        let board = classic_board();
        let entry0 = start_entry(&board, 0);
        let mut here = entry0;
        for _ in 0..39 {
            here = match board.next_space(here, PlayerColor(0)) {
                NextSpace::Single(next) => next,
                other => panic!("expected a single ring step, got {other:?}"),
            };
        }
        let fork = here;
        let lane_entry = match board.next_space(fork, PlayerColor(0)) {
            NextSpace::Branch(options) => *options.iter().find(|&&s| s != entry0).unwrap(),
            other => panic!("expected a branch at the fork, got {other:?}"),
        };

        let mut here = lane_entry;
        assert_eq!(board.node(here).kind, SpaceKind::HomeLane);
        for _ in 0..4 {
            here = match board.next_space(here, PlayerColor(0)) {
                NextSpace::Single(next) => next,
                other => panic!("expected a single home lane step, got {other:?}"),
            };
            assert_eq!(board.node(here).kind, SpaceKind::HomeLane);
        }
        here = match board.next_space(here, PlayerColor(0)) {
            NextSpace::Single(next) => next,
            other => panic!("expected the last lane step to reach Finish, got {other:?}"),
        };
        assert_eq!(board.node(here).kind, SpaceKind::Finish);
        assert_eq!(board.next_space(here, PlayerColor(0)), NextSpace::DeadEnd);
    }

    #[test]
    fn restricted_edge_is_a_dead_end_for_a_different_color() {
        // Manually built, minimal topology: one space with a single edge
        // restricted to color 0, and nothing else.
        let restricted_target = SpaceId(1);
        let node = SpaceNode {
            id: SpaceId(0),
            kind: SpaceKind::Shared,
            owner: None,
            safe: false,
            edges: vec![Edge {
                to: restricted_target,
                restricted_to: Some(PlayerColor(0)),
                forced: false,
            }],
        };
        let target = SpaceNode {
            id: restricted_target,
            kind: SpaceKind::Shared,
            owner: None,
            safe: false,
            edges: Vec::new(),
        };
        let board = BoardTopology {
            nodes: vec![node, target],
            yard_spaces: HashMap::new(),
        };

        assert_eq!(
            board.next_space(SpaceId(0), PlayerColor(0)),
            NextSpace::Single(restricted_target)
        );
        assert_eq!(
            board.next_space(SpaceId(0), PlayerColor(1)),
            NextSpace::DeadEnd
        );
    }

    #[test]
    fn forced_edge_overrides_every_other_eligible_edge() {
        let forced_target = SpaceId(1);
        let optional_target = SpaceId(2);
        let node = SpaceNode {
            id: SpaceId(0),
            kind: SpaceKind::Shared,
            owner: None,
            safe: false,
            edges: vec![
                Edge {
                    to: optional_target,
                    restricted_to: None,
                    forced: false,
                },
                Edge {
                    to: forced_target,
                    restricted_to: Some(PlayerColor(0)),
                    forced: true,
                },
            ],
        };
        let a = SpaceNode {
            id: forced_target,
            kind: SpaceKind::Shared,
            owner: None,
            safe: false,
            edges: Vec::new(),
        };
        let b = SpaceNode {
            id: optional_target,
            kind: SpaceKind::Shared,
            owner: None,
            safe: false,
            edges: Vec::new(),
        };
        let board = BoardTopology {
            nodes: vec![node, a, b],
            yard_spaces: HashMap::new(),
        };

        // The forced edge only applies to color 0; color 1 still sees the
        // plain, unrestricted edge as its only option.
        assert_eq!(
            board.next_space(SpaceId(0), PlayerColor(0)),
            NextSpace::Single(forced_target)
        );
        assert_eq!(
            board.next_space(SpaceId(0), PlayerColor(1)),
            NextSpace::Single(optional_target)
        );
    }
}
