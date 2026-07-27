//! The one traversal algorithm for walking a pawn a number of steps across
//! the board, checking blockades and the finish line as it goes. Invoked
//! with an already-combined step count, not a single card's numbers.

use std::collections::HashMap;

use thiserror::Error;

use crate::board::{BoardError, BoardTopology, NextSpace, PlayerColor, SpaceId, SpaceKind};
use crate::pawn::Pawn;
use crate::rules::RuleConfig;

/// The squares touched by a resolved move, in order, and where it ended.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MovementOutcome {
    pub squares_passed: Vec<SpaceId>,
    pub final_space: SpaceId,
}

/// Ways a walk can fail to complete.
#[derive(Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum MoveError {
    #[error("blocked by a blockade")]
    BlockedByBlockade,
    #[error("overshot the finish line")]
    Overshoot,
    #[error("movement would require resolving a branch, which isn't supported here")]
    UnresolvedBranch,
    #[error("ran out of board before consuming every step")]
    DeadEnd,
    #[error(transparent)]
    InvalidBoard(#[from] BoardError),
}

/// Walks `owner`'s pawn `steps` spaces from `from`, honoring blockades
/// (`rules.blockades_enabled`) and the finish-line rule
/// (`rules.exact_count_to_finish`).
pub fn walk(
    topology: &BoardTopology,
    rules: &RuleConfig,
    pawns: &[Pawn],
    owner: PlayerColor,
    from: SpaceId,
    steps: u8,
) -> Result<MovementOutcome, MoveError> {
    let mut current = from;
    let mut squares_passed = Vec::new();

    for _ in 0..steps {
        if topology.node(current)?.kind == SpaceKind::Finish {
            if rules.exact_count_to_finish {
                return Err(MoveError::Overshoot);
            }
            break;
        }
        match topology.next_space(current, owner)? {
            NextSpace::Single(next) => {
                if rules.blockades_enabled && is_blockaded(pawns, next) {
                    return Err(MoveError::BlockedByBlockade);
                }
                current = next;
                squares_passed.push(next);
            }
            NextSpace::Branch(_) => return Err(MoveError::UnresolvedBranch),
            NextSpace::DeadEnd => return Err(MoveError::DeadEnd),
        }
    }

    Ok(MovementOutcome {
        squares_passed,
        final_space: current,
    })
}

/// A blockade is two or more of the *same* color's pawns stacked on one
/// space — it blocks every color from passing through or landing on it.
fn is_blockaded(pawns: &[Pawn], space: SpaceId) -> bool {
    let mut counts: HashMap<PlayerColor, u8> = HashMap::new();
    for pawn in pawns {
        if pawn.position == space {
            *counts.entry(pawn.owner).or_insert(0) += 1;
        }
    }
    counts.values().any(|&count| count >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pawn::PawnId;

    const RING_LEN: u16 = 8;
    const HOME_LANE_LEN: u16 = 3;

    fn test_board() -> BoardTopology {
        BoardTopology::standard_ring(2, RING_LEN, HOME_LANE_LEN, 2).unwrap()
    }

    fn test_rules(blockades_enabled: bool, exact_count_to_finish: bool) -> RuleConfig {
        RuleConfig {
            blockades_enabled,
            exact_count_to_finish,
            ..crate::rules::minimal_rules()
        }
    }

    fn expect_single(next: NextSpace) -> SpaceId {
        match next {
            NextSpace::Single(space) => space,
            other => panic!("expected a single edge, got {other:?}"),
        }
    }

    fn entry_of(board: &BoardTopology, color: PlayerColor) -> SpaceId {
        let yard_slot = board.yard_spaces(color)[0];
        expect_single(board.next_space(yard_slot, color).unwrap())
    }

    /// Steps `n` times via `next_space` directly, as an independent oracle
    /// for what `walk` should produce — panics if any step isn't a single
    /// unconditional edge.
    fn manual_steps(
        board: &BoardTopology,
        color: PlayerColor,
        from: SpaceId,
        n: u16,
    ) -> Vec<SpaceId> {
        let mut current = from;
        let mut result = Vec::new();
        for _ in 0..n {
            current = expect_single(board.next_space(current, color).unwrap());
            result.push(current);
        }
        result
    }

    /// The first space in `color`'s home lane, reached by resolving the
    /// fork's branch directly (something `walk` itself can't do).
    fn lane_entry_for(board: &BoardTopology, color: PlayerColor) -> SpaceId {
        let entry = entry_of(board, color);
        let fork = *manual_steps(board, color, entry, RING_LEN - 1)
            .last()
            .unwrap();
        match board.next_space(fork, color).unwrap() {
            NextSpace::Branch(options) => *options
                .iter()
                .find(|&&s| s != entry)
                .expect("home lane branch should differ from the ring-continue edge"),
            other => panic!("expected the fork to offer a branch, got {other:?}"),
        }
    }

    #[test]
    fn walks_a_straight_path_across_the_ring() {
        let board = test_board();
        let rules = test_rules(true, false);
        let entry = entry_of(&board, PlayerColor(0));
        let expected = manual_steps(&board, PlayerColor(0), entry, 3);
        let outcome = walk(&board, &rules, &[], PlayerColor(0), entry, 3).unwrap();
        assert_eq!(outcome.squares_passed, expected);
        assert_eq!(outcome.final_space, expected[2]);
    }

    #[test]
    fn blockade_stops_movement_when_enabled() {
        let board = test_board();
        let rules = test_rules(true, false);
        let entry = entry_of(&board, PlayerColor(0));
        let path = manual_steps(&board, PlayerColor(0), entry, 3);
        let pawns = vec![
            crate::pawn::tests::bare_pawn(PawnId(0), PlayerColor(1), path[1]),
            crate::pawn::tests::bare_pawn(PawnId(1), PlayerColor(1), path[1]),
        ];
        assert_eq!(
            walk(&board, &rules, &pawns, PlayerColor(0), entry, 3).unwrap_err(),
            MoveError::BlockedByBlockade
        );
    }

    #[test]
    fn blockade_is_ignored_when_the_rule_is_disabled() {
        let board = test_board();
        let rules = test_rules(false, false);
        let entry = entry_of(&board, PlayerColor(0));
        let path = manual_steps(&board, PlayerColor(0), entry, 3);
        let pawns = vec![
            crate::pawn::tests::bare_pawn(PawnId(0), PlayerColor(1), path[1]),
            crate::pawn::tests::bare_pawn(PawnId(1), PlayerColor(1), path[1]),
        ];
        let outcome = walk(&board, &rules, &pawns, PlayerColor(0), entry, 3).unwrap();
        assert_eq!(outcome.final_space, path[2]);
    }

    #[test]
    fn a_single_pawn_on_a_space_is_not_a_blockade() {
        let board = test_board();
        let rules = test_rules(true, false);
        let entry = entry_of(&board, PlayerColor(0));
        let path = manual_steps(&board, PlayerColor(0), entry, 3);
        let pawns = vec![crate::pawn::tests::bare_pawn(
            PawnId(0),
            PlayerColor(1),
            path[1],
        )];
        let outcome = walk(&board, &rules, &pawns, PlayerColor(0), entry, 3).unwrap();
        assert_eq!(outcome.final_space, path[2]);
    }

    #[test]
    fn overshooting_finish_is_an_error_when_exact_count_is_required() {
        let board = test_board();
        let rules = test_rules(true, true);
        let lane_entry = lane_entry_for(&board, PlayerColor(0));
        // HOME_LANE_LEN steps from the first lane space reaches Finish
        // exactly; one more overshoots it.
        let outcome = walk(
            &board,
            &rules,
            &[],
            PlayerColor(0),
            lane_entry,
            HOME_LANE_LEN as u8 + 1,
        );
        assert_eq!(outcome.unwrap_err(), MoveError::Overshoot);
    }

    #[test]
    fn overshooting_finish_clamps_when_exact_count_is_not_required() {
        let board = test_board();
        let rules = test_rules(true, false);
        let lane_entry = lane_entry_for(&board, PlayerColor(0));
        let outcome = walk(
            &board,
            &rules,
            &[],
            PlayerColor(0),
            lane_entry,
            HOME_LANE_LEN as u8 + 1,
        )
        .unwrap();
        assert_eq!(outcome.squares_passed.len(), HOME_LANE_LEN as usize);
        assert_eq!(
            board.node(outcome.final_space).unwrap().kind,
            SpaceKind::Finish
        );
    }

    #[test]
    fn branch_without_a_resolution_strategy_is_an_error() {
        // The fork itself is a genuine ambiguous branch for its owning
        // color (loop again vs. enter the home lane) that `walk` can't
        // resolve on its own.
        let board = test_board();
        let rules = test_rules(true, false);
        let entry = entry_of(&board, PlayerColor(0));
        let fork = *manual_steps(&board, PlayerColor(0), entry, RING_LEN - 1)
            .last()
            .unwrap();
        assert_eq!(
            walk(&board, &rules, &[], PlayerColor(0), fork, 1).unwrap_err(),
            MoveError::UnresolvedBranch
        );
    }

    #[test]
    fn walking_off_an_unknown_space_reports_the_board_error() {
        let board = test_board();
        let rules = test_rules(true, false);
        let bogus = SpaceId(u32::MAX);
        assert_eq!(
            walk(&board, &rules, &[], PlayerColor(0), bogus, 1).unwrap_err(),
            MoveError::InvalidBoard(BoardError::UnknownSpace(bogus))
        );
    }
}
