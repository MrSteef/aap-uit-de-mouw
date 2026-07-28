//! `PlayContext`: the restricted API surface a card's `on_played`/
//! `on_claimed` hooks act through.

use std::collections::HashMap;

use crate::board::{BoardTopology, SpaceId};
use crate::card::{CardCatalog, CardKindId};
use crate::context::InteractionContext;
use crate::event::GameEvent;
use crate::movement::{self, MoveError};
use crate::pawn::{
    AutomaticAuditCatch, ClaimedEffectState, EffectAnchor, ExpiryCondition, Pawn, PawnId,
    PersistentEffectState,
};
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

/// Everything a resolved play produced, once `PlayContext` is done with
/// it: the event log, plus any automatic-audit catches for the caller
/// (`GameState`) to route into the wider card economy.
#[derive(Default)]
pub struct PlayOutcome {
    pub events: Vec<GameEvent>,
    pub automatic_audit_catches: Vec<AutomaticAuditCatch>,
}

/// The context a card's `on_played`/`on_claimed` hooks act through: it can
/// resolve movement, attempt captures, attach persistent effects, and log
/// events, but nothing else.
pub struct PlayContext<'a> {
    topology: &'a BoardTopology,
    rules: &'a RuleConfig,
    catalog: &'a CardCatalog,
    pawns: &'a mut Vec<Pawn>,
    space_effects: &'a mut HashMap<SpaceId, Vec<PersistentEffectState>>,
    mover: PawnId,
    /// Which card is currently executing `on_played`/`on_claimed` — set by
    /// `begin_card` before each dispatch, read by `attach_persistent_effect`/
    /// `attach_claimed_effect` so they know what to attribute the effect to.
    current_card: Option<CardKindId>,
    events: Vec<GameEvent>,
    /// Every automatic-audit catch recorded while resolving this play —
    /// see `PlayOutcome`.
    automatic_audit_catches: Vec<AutomaticAuditCatch>,
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
            current_card: None,
            events: Vec::new(),
            automatic_audit_catches: Vec::new(),
        }
    }

    /// The pawn this play is moving.
    pub fn mover(&self) -> PawnId {
        self.mover
    }

    /// Declares which card is about to have its `on_played`/`on_claimed`
    /// dispatched — must be called before each such dispatch, since
    /// `attach_persistent_effect`/`attach_claimed_effect` attribute the
    /// effect to whichever card was declared most recently.
    pub fn begin_card(&mut self, card: CardKindId) {
        self.current_card = Some(card);
    }

    /// Resolves the combined walk described by `proposal`, moves the mover
    /// there, emits a `PawnMoved` event, and attempts to capture every
    /// pawn on a square that qualifies under `proposal.capture_mode`
    /// (landing square only, or every square passed). Returns what got
    /// captured, as `(pawn, position)` pairs — the caller uses this to
    /// build the mover's `MoveRecord.captures_caused`. A no-op (returns an
    /// empty `Vec`) if `proposal.steps == 0`.
    pub fn resolve_movement(
        &mut self,
        proposal: MovementProposal,
    ) -> Result<Vec<(PawnId, SpaceId)>, MoveError> {
        if proposal.steps == 0 {
            return Ok(Vec::new());
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

        let squares_to_check: Vec<SpaceId> = match proposal.capture_mode {
            CaptureMode::LandingSquareOnly => vec![outcome.final_space],
            CaptureMode::EveryStepPassed => outcome.squares_passed.clone(),
        };

        let mut captures_caused = Vec::new();
        for square in squares_to_check {
            let is_landing = square == outcome.final_space;
            let targets: Vec<PawnId> = self
                .pawns
                .iter()
                .filter(|pawn| pawn.position == square && pawn.id != self.mover)
                .map(|pawn| pawn.id)
                .collect();
            for target in targets {
                if self.attempt_capture(target, is_landing) == CaptureOutcome::Proceeds {
                    captures_caused.push((target, square));
                    self.emit(GameEvent::PawnCaptured {
                        pawn: target,
                        by: self.mover,
                    });
                }
            }
        }

        Ok(captures_caused)
    }

    /// Attempts to capture `target`, dispatching to its real persistent
    /// effects (`on_capture_attempted_as_played`) and outstanding claimed
    /// ones (`on_capture_attempted_as_claimed`) — no priority between the
    /// two; whichever exist get called. Blocked if any dispatched hook
    /// says so. `is_landing` is `true` only when `target` sits on the
    /// move's final square. A no-op (`Proceeds`) if `target` doesn't
    /// exist. If not blocked and `RuleConfig::capture_sends_to_yard`,
    /// sends `target` to one of its own yard slots.
    ///
    /// Independently callable by a future card with no movement involved
    /// at all — ARCHITECTURE.md §4 shows this taking only `target`; the
    /// added `is_landing` parameter is necessary since only the caller
    /// (here, `resolve_movement`) knows whether this square is the final
    /// one or a mid-path square under `CaptureMode::EveryStepPassed`.
    pub fn attempt_capture(&mut self, target: PawnId, is_landing: bool) -> CaptureOutcome {
        let Some(target_index) = self.pawns.iter().position(|pawn| pawn.id == target) else {
            return CaptureOutcome::Proceeds;
        };
        let persistent = self.pawns[target_index].persistent_effects().to_vec();
        let claimed = self.pawns[target_index].claimed_effects().to_vec();

        let mut blocked = false;
        {
            let mut interaction_ctx = InteractionContext::new(
                self.mover,
                target,
                is_landing,
                self.topology,
                self.rules,
                self.pawns,
                &mut self.events,
                &mut self.automatic_audit_catches,
            );
            for effect in &persistent {
                if let Some(meta) = self.catalog.get(effect.source_card)
                    && meta
                        .behavior
                        .on_capture_attempted_as_played(&mut interaction_ctx)
                        == CaptureOutcome::Blocked
                {
                    blocked = true;
                }
            }
            for effect in &claimed {
                if let Some(meta) = self.catalog.get(effect.source_card)
                    && meta
                        .behavior
                        .on_capture_attempted_as_claimed(&mut interaction_ctx)
                        == CaptureOutcome::Blocked
                {
                    blocked = true;
                }
            }
        }

        if !blocked
            && self.rules.capture_sends_to_yard
            && let Some(&yard_slot) = self
                .topology
                .yard_spaces(self.pawns[target_index].owner)
                .first()
        {
            self.pawns[target_index].capture_to(yard_slot);
        }

        if blocked {
            CaptureOutcome::Blocked
        } else {
            CaptureOutcome::Proceeds
        }
    }

    /// Attaches a real persistent effect, anchored to `anchor`, for
    /// whichever card `begin_card` most recently declared.
    pub fn attach_persistent_effect(
        &mut self,
        anchor: EffectAnchor,
        expires: Option<ExpiryCondition>,
    ) {
        let source_card = self.current_card.expect(
            "attach_persistent_effect called without begin_card — see PlayContext::begin_card",
        );
        let effect = PersistentEffectState {
            source_card,
            anchor,
            revealed: false,
            expires,
        };
        match anchor {
            EffectAnchor::Pawn(id) => {
                if let Some(pawn) = self.pawns.iter_mut().find(|pawn| pawn.id == id) {
                    pawn.attach_persistent_effect(effect);
                }
            }
            EffectAnchor::Space(space) => {
                self.space_effects.entry(space).or_default().push(effect);
            }
        }
    }

    /// Attaches an outstanding claimed effect, anchored to `anchor`, for
    /// whichever card `begin_card` most recently declared. Only
    /// `EffectAnchor::Pawn` is meaningful here — `ClaimedEffectState` lives
    /// on `Pawn` (ARCHITECTURE.md §8), with no space-anchored equivalent;
    /// a `Space`-anchored claim is silently dropped, since no card claims
    /// one yet.
    pub fn attach_claimed_effect(&mut self, anchor: EffectAnchor) {
        let source_card = self.current_card.expect(
            "attach_claimed_effect called without begin_card — see PlayContext::begin_card",
        );
        if let EffectAnchor::Pawn(id) = anchor
            && let Some(pawn) = self.pawns.iter_mut().find(|pawn| pawn.id == id)
        {
            pawn.attach_claimed_effect(ClaimedEffectState {
                source_card,
                anchor,
            });
        }
    }

    /// Records an event as having happened during this play.
    pub fn emit(&mut self, event: GameEvent) {
        self.events.push(event);
    }

    /// Consumes this context, returning everything it accumulated —
    /// `PlayContext` doesn't have a way to hand game state its results
    /// incrementally, so the caller drains it once the play is fully
    /// resolved.
    pub fn into_outcome(self) -> PlayOutcome {
        PlayOutcome {
            events: self.events,
            automatic_audit_catches: self.automatic_audit_catches,
        }
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
        vec![crate::pawn::tests::bare_pawn(PawnId(0), color, entry)]
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
        let captures = ctx.resolve_movement(proposal).unwrap();
        assert!(captures.is_empty());

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

        assert!(
            ctx.resolve_movement(MovementProposal::default())
                .unwrap()
                .is_empty()
        );
        assert_eq!(pawns[0].position, start);
    }

    #[test]
    fn resolve_movement_captures_a_pawn_on_the_landing_square_and_sends_it_to_yard() {
        let topology = small_board();
        let rules = crate::rules::minimal_rules();
        let catalog = CardCatalog::standard();
        let mut pawns = one_pawn_at(&topology, PlayerColor(0));
        let path_start = pawns[0].position;
        let landing = {
            let mut here = path_start;
            for _ in 0..3 {
                here = match topology.next_space(here, PlayerColor(0)).unwrap() {
                    NextSpace::Single(space) => space,
                    other => panic!("expected a single ring step, got {other:?}"),
                };
            }
            here
        };
        pawns.push(crate::pawn::tests::bare_pawn(
            PawnId(1),
            PlayerColor(1),
            landing,
        ));
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
        let captures = ctx.resolve_movement(proposal).unwrap();

        assert_eq!(captures, vec![(PawnId(1), landing)]);
        assert_eq!(pawns[0].position, landing);
        assert_eq!(pawns[1].position, topology.yard_spaces(PlayerColor(1))[0]);
    }

    #[test]
    fn attach_persistent_effect_requires_begin_card_first() {
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

        ctx.begin_card(crate::card::CardKindId(4));
        ctx.attach_persistent_effect(EffectAnchor::Pawn(PawnId(0)), None);

        assert_eq!(pawns[0].persistent_effects().len(), 1);
        assert_eq!(
            pawns[0].persistent_effects()[0].source_card,
            crate::card::CardKindId(4)
        );
    }

    #[test]
    fn attach_claimed_effect_attaches_to_the_named_pawn() {
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

        ctx.begin_card(crate::card::CardKindId(4));
        ctx.attach_claimed_effect(EffectAnchor::Pawn(PawnId(0)));

        assert_eq!(pawns[0].claimed_effects().len(), 1);
    }

    #[test]
    fn attempt_capture_on_an_unknown_pawn_proceeds() {
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

        assert_eq!(
            ctx.attempt_capture(PawnId(99), true),
            CaptureOutcome::Proceeds
        );
    }
}
