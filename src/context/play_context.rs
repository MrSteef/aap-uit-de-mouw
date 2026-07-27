//! `PlayContext`: the restricted API surface a card's `on_played`/
//! `on_claimed` hooks act through.

use std::collections::HashMap;

use crate::board::{BoardTopology, SpaceId};
use crate::card::CardCatalog;
use crate::event::GameEvent;
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
// `topology`/`rules`/`catalog`/`pawns`/`space_effects` aren't read by
// anything yet — `resolve_movement`/`attempt_capture`/
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
    /// The pawn this play is moving.
    pub fn mover(&self) -> PawnId {
        self.mover
    }

    /// Resolves the combined walk described by `proposal`, capturing every
    /// square touched that qualifies under its `capture_mode`. A no-op if
    /// `proposal.steps == 0`.
    pub fn resolve_movement(&mut self, _proposal: MovementProposal) {
        todo!("wired up once movement.rs's walk() is driven from here — see ARCHITECTURE.md §4")
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
    use crate::board::PlayerColor;

    fn play_context<'a>(
        topology: &'a BoardTopology,
        rules: &'a RuleConfig,
        catalog: &'a CardCatalog,
        pawns: &'a mut Vec<Pawn>,
        space_effects: &'a mut HashMap<SpaceId, Vec<PersistentEffectState>>,
        mover: PawnId,
    ) -> PlayContext<'a> {
        PlayContext {
            topology,
            rules,
            catalog,
            pawns,
            space_effects,
            mover,
            events: Vec::new(),
        }
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
        let topology = BoardTopology::standard_ring(2, 8, 3, 2).unwrap();
        let rules = crate::rules::minimal_rules();
        let catalog = CardCatalog::standard();
        let mut pawns = vec![Pawn {
            id: PawnId(0),
            owner: PlayerColor(0),
            position: SpaceId(0),
        }];
        let mut space_effects = HashMap::new();
        let ctx = play_context(
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
        let topology = BoardTopology::standard_ring(2, 8, 3, 2).unwrap();
        let rules = crate::rules::minimal_rules();
        let catalog = CardCatalog::standard();
        let mut pawns = vec![Pawn {
            id: PawnId(0),
            owner: PlayerColor(0),
            position: SpaceId(0),
        }];
        let mut space_effects = HashMap::new();
        let mut ctx = play_context(
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
}
