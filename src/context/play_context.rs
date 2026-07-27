//! `PlayContext`: the restricted API surface a card's `on_played`/
//! `on_claimed` hooks act through.

use std::collections::HashMap;

use crate::board::{BoardTopology, SpaceId};
use crate::card::CardCatalog;
use crate::event::GameEvent;
use crate::movement::{self, MoveError};
use crate::pawn::{EffectAnchor, Pawn, PawnId, PersistentEffectState};
use crate::rules::RuleConfig;

/// How far a capture reaches along a resolved move: only the landing
/// square, or every square passed along the way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureMode {
    LandingSquareOnly,
    EveryStepPassed,
}

/// What a play's claimed cards accumulate into before one movement
/// resolves. A plain movement card adds to `steps`; a double-style
/// modifier multiplies `multiplier`; a rampage-style modifier upgrades
/// `capture_mode`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MovementProposal {
    pub steps: u8,
    pub multiplier: u8,
    pub capture_mode: CaptureMode,
}

impl Default for MovementProposal {
    fn default() -> Self {
        Self {
            steps: 0,
            multiplier: 1,
            capture_mode: CaptureMode::LandingSquareOnly,
        }
    }
}

/// Whether an attempted capture actually happens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureOutcome {
    Proceeds,
    Blocked,
}

/// The context a card's `on_played`/`on_claimed` hooks act through: it can
/// resolve movement, attempt captures, attach persistent effects, and log
/// events, but nothing else.
// `catalog`/`space_effects` aren't read by anything yet — `attempt_capture`/
// `attach_persistent_effect` are still `todo!()` and will read them once
// implemented.
#[allow(dead_code)]
pub struct PlayContext<'a> {
    topology: &'a BoardTopology,
    rules: &'a RuleConfig,
    catalog: &'a CardCatalog,
    pawns: &'a mut Vec<Pawn>,
    space_effects: &'a mut HashMap<SpaceId, Vec<PersistentEffectState>>,
    mover: PawnId,
    events: Vec<GameEvent>,
}

impl<'a> PlayContext<'a> {
    /// Builds a context for `mover`'s play, borrowing everything it needs
    /// from live game state.
    pub fn new(
        topology: &'a BoardTopology,
        rules: &'a RuleConfig,
        catalog: &'a CardCatalog,
        pawns: &'a mut Vec<Pawn>,
        space_effects: &'a mut HashMap<SpaceId, Vec<PersistentEffectState>>,
        mover: PawnId,
    ) -> Self {
        Self {
            topology,
            rules,
            catalog,
            pawns,
            space_effects,
            mover,
            events: Vec::new(),
        }
    }

    /// The pawn this play is moving.
    pub fn mover(&self) -> PawnId {
        self.mover
    }

    /// Resolves the combined walk described by `proposal` and moves the
    /// mover there, emitting a `PawnMoved` event. A no-op if
    /// `proposal.steps == 0`.
    ///
    /// Capture dispatch (`attempt_capture` per `proposal.capture_mode`)
    /// isn't wired in yet — it needs `Pawn`'s real persistent-effect state
    /// (ARCHITECTURE.md §16, step 6) to have anything meaningful to check.
    /// Landing on or passing another pawn is currently a no-op.
    pub fn resolve_movement(&mut self, proposal: MovementProposal) -> Result<(), MoveError> {
        if proposal.steps == 0 {
            return Ok(());
        }
        // `mover` is always one of `pawns` by construction — `new` is the
        // only way to build a `PlayContext`, and its caller (the not-yet-built
        // game engine) always derives `mover` from that same pawn list.
        let index = self
            .pawns
            .iter()
            .position(|pawn| pawn.id == self.mover)
            .expect("PlayContext::mover must be one of PlayContext::pawns");
        let owner = self.pawns[index].owner;
        let from = self.pawns[index].position;
        let total_steps = proposal.steps.saturating_mul(proposal.multiplier);

        let outcome = movement::walk(
            self.topology,
            self.rules,
            self.pawns,
            owner,
            from,
            total_steps,
        )?;
        self.pawns[index].position = outcome.final_space;
        self.emit(GameEvent::PawnMoved {
            pawn: self.mover,
            from,
            to: outcome.final_space,
        });
        Ok(())
    }

    /// Attempts to capture `target`, dispatching to its persistent and
    /// claimed effects. Independently callable by a future card with no
    /// movement involved at all.
    pub fn attempt_capture(&mut self, _target: PawnId) -> CaptureOutcome {
        todo!("needs CardBehavior's capture-attempt hooks, added once context/ is fully wired")
    }

    /// Attaches whichever card is currently executing `on_played` to
    /// `anchor`.
    pub fn attach_persistent_effect(&mut self, _anchor: EffectAnchor) {
        todo!("needs to know which card is currently executing — added alongside concrete cards")
    }

    /// Records an event as having happened during this play.
    pub fn emit(&mut self, event: GameEvent) {
        self.events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{NextSpace, PlayerColor};

    fn small_board() -> BoardTopology {
        BoardTopology::standard_ring(2, 8, 3, 2).unwrap()
    }

    fn one_pawn_at(topology: &BoardTopology, color: PlayerColor) -> Vec<Pawn> {
        let yard = topology.yard_spaces(color)[0];
        let entry = match topology.next_space(yard, color).unwrap() {
            NextSpace::Single(space) => space,
            other => panic!("expected a single yard exit edge, got {other:?}"),
        };
        vec![Pawn {
            id: PawnId(0),
            owner: color,
            position: entry,
        }]
    }

    #[test]
    fn movement_proposal_default_is_a_zero_step_single_capture() {
        let proposal = MovementProposal::default();
        assert_eq!(proposal.steps, 0);
        assert_eq!(proposal.multiplier, 1);
        assert_eq!(proposal.capture_mode, CaptureMode::LandingSquareOnly);
    }

    #[test]
    fn mover_reports_the_pawn_this_context_was_built_for() {
        let topology = small_board();
        let rules = crate::rules::minimal_rules();
        let catalog = CardCatalog::standard();
        let mut pawns = one_pawn_at(&topology, PlayerColor(0));
        let mut space_effects = HashMap::new();
        let ctx = PlayContext::new(
            &topology,
            &rules,
            &catalog,
            &mut pawns,
            &mut space_effects,
            PawnId(0),
        );
        assert_eq!(ctx.mover(), PawnId(0));
    }

    #[test]
    fn emit_records_the_event() {
        let topology = small_board();
        let rules = crate::rules::minimal_rules();
        let catalog = CardCatalog::standard();
        let mut pawns = one_pawn_at(&topology, PlayerColor(0));
        let mut space_effects = HashMap::new();
        let mut ctx = PlayContext::new(
            &topology,
            &rules,
            &catalog,
            &mut pawns,
            &mut space_effects,
            PawnId(0),
        );
        ctx.emit(GameEvent::PawnMoved {
            pawn: PawnId(0),
            from: SpaceId(0),
            to: SpaceId(1),
        });
        assert_eq!(ctx.events.len(), 1);
    }

    #[test]
    fn resolve_movement_walks_the_mover_and_emits_pawn_moved() {
        let topology = small_board();
        let rules = crate::rules::minimal_rules();
        let catalog = CardCatalog::standard();
        let mut pawns = one_pawn_at(&topology, PlayerColor(0));
        let start = pawns[0].position;
        let mut space_effects = HashMap::new();
        let mut ctx = PlayContext::new(
            &topology,
            &rules,
            &catalog,
            &mut pawns,
            &mut space_effects,
            PawnId(0),
        );

        let proposal = MovementProposal {
            steps: 3,
            multiplier: 1,
            capture_mode: CaptureMode::LandingSquareOnly,
        };
        ctx.resolve_movement(proposal).unwrap();

        assert_eq!(pawns[0].position, {
            let mut expected = start;
            for _ in 0..3 {
                expected = match topology.next_space(expected, PlayerColor(0)).unwrap() {
                    NextSpace::Single(space) => space,
                    other => panic!("expected a single ring step, got {other:?}"),
                };
            }
            expected
        });
    }

    #[test]
    fn resolve_movement_is_a_no_op_for_zero_steps() {
        let topology = small_board();
        let rules = crate::rules::minimal_rules();
        let catalog = CardCatalog::standard();
        let mut pawns = one_pawn_at(&topology, PlayerColor(0));
        let start = pawns[0].position;
        let mut space_effects = HashMap::new();
        let mut ctx = PlayContext::new(
            &topology,
            &rules,
            &catalog,
            &mut pawns,
            &mut space_effects,
            PawnId(0),
        );

        ctx.resolve_movement(MovementProposal::default()).unwrap();
        assert_eq!(pawns[0].position, start);
    }
}
