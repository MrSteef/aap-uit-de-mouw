//! Cards are data-driven behavior, not a closed enum — a new card kind is a
//! new type implementing `CardBehavior`, registered in a `CardCatalog`, not
//! a new branch threaded through the engine.

pub mod movement;
pub mod movement_modifier;

use crate::context::{
    AuditContext, CaptureOutcome, InteractionContext, MovementProposal, PlayContext,
};

/// Identifies one kind of card in a `CardCatalog`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CardKindId(pub u16);

/// Which broad family of effect a card belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CardCategory {
    Movement,
    MovementModifier,
    Offense,
    Defense,
    Deception,
}

/// Whether an audited move's claim matched what was truly played.
///
/// Defined here rather than in `audit.rs` (where ARCHITECTURE.md §9 shows
/// it) because `CardBehavior`'s audit hooks below need it, and `audit ──>
/// card` per §1's dependency graph — `card` must not depend on `audit`.
/// This mirrors why `EffectAnchor` lives in `pawn.rs` instead of
/// `context/`: the type moves to whichever module is lower in the
/// dependency graph among the ones that need it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AuditOutcome {
    LieCaught,
    ClaimWasTrue,
}

/// The behavior a card kind implements.
pub trait CardBehavior {
    /// Fires the instant this card is *actually* consumed from hand.
    fn on_played(&self, _ctx: &mut PlayContext) {}

    /// Fires once per *claimed* card in a play, contributing to the shared
    /// movement proposal that resolves once every claimed card has had a
    /// turn at it.
    fn on_claimed(&self, _ctx: &mut PlayContext, _proposal: &mut MovementProposal) {}

    /// Any touch — landing or passing — on a space where this card is
    /// attached, as this pawn's *actual* state.
    fn on_passed_as_played(&self, _ctx: &mut InteractionContext) {}
    /// As above, but for this pawn's *claimed* state.
    fn on_passed_as_claimed(&self, _ctx: &mut InteractionContext) {}

    /// An attempted capture, dispatched against this card's *actual* state.
    fn on_capture_attempted_as_played(&self, _ctx: &mut InteractionContext) -> CaptureOutcome {
        CaptureOutcome::Proceeds
    }
    /// As above, but for this pawn's *claimed* state.
    fn on_capture_attempted_as_claimed(&self, _ctx: &mut InteractionContext) -> CaptureOutcome {
        CaptureOutcome::Proceeds
    }

    /// Fires when an audit resolves, dispatched against the *actually
    /// played* card in the audited move — independent of `outcome`, so it
    /// can fire even when the accusation was wrong (see `StunTrapCard`).
    fn on_audited_as_played(&self, _outcome: AuditOutcome, _ctx: &mut AuditContext) {}
    /// As above, but for the *claimed* card in the audited move.
    fn on_audited_as_claimed(&self, _outcome: AuditOutcome, _ctx: &mut AuditContext) {}
}

/// One card kind's catalog entry: its identity, display info, and behavior.
pub struct CardMeta {
    pub id: CardKindId,
    pub display_name: &'static str,
    pub category: CardCategory,
    pub behavior: Box<dyn CardBehavior + Send + Sync>,
}

/// The registry of every card kind in a game. `CardKindId(n)` is always the
/// `n`th entry registered.
pub struct CardCatalog {
    definitions: Vec<CardMeta>,
}

impl CardCatalog {
    /// The catalog entry for `id`, or `None` if no card kind is registered
    /// under that id.
    pub fn get(&self, id: CardKindId) -> Option<&CardMeta> {
        self.definitions.get(id.0 as usize)
    }

    /// Builds a catalog from arbitrary entries — used by other modules'
    /// tests that need a `CardBehavior` `standard()` doesn't register yet
    /// (e.g. simulating `StunTrapCard` before it exists, in `audit.rs`'s
    /// tests).
    #[cfg(test)]
    pub(crate) fn from_definitions(definitions: Vec<CardMeta>) -> Self {
        Self { definitions }
    }

    /// The standard catalog. Only the movement/movement-modifier cards
    /// exist so far (build order §16, step 4) — offense/defense/deception
    /// cards are added as later steps build them.
    pub fn standard() -> Self {
        use movement::MoveCard;
        use movement_modifier::{DoubleModifierCard, RampageModifierCard};

        let definitions = vec![
            CardMeta {
                id: CardKindId(0),
                display_name: "Take 1",
                category: CardCategory::Movement,
                behavior: Box::new(MoveCard { steps: 1 }),
            },
            CardMeta {
                id: CardKindId(1),
                display_name: "Take 2",
                category: CardCategory::Movement,
                behavior: Box::new(MoveCard { steps: 2 }),
            },
            CardMeta {
                id: CardKindId(2),
                display_name: "Take 3",
                category: CardCategory::Movement,
                behavior: Box::new(MoveCard { steps: 3 }),
            },
            CardMeta {
                id: CardKindId(3),
                display_name: "Take 4",
                category: CardCategory::Movement,
                behavior: Box::new(MoveCard { steps: 4 }),
            },
            CardMeta {
                id: CardKindId(4),
                display_name: "Double",
                category: CardCategory::MovementModifier,
                behavior: Box::new(DoubleModifierCard { multiplier: 2 }),
            },
            CardMeta {
                id: CardKindId(5),
                display_name: "Rampage",
                category: CardCategory::MovementModifier,
                behavior: Box::new(RampageModifierCard),
            },
        ];
        Self { definitions }
    }
}

/// Shared test fixtures for building a `PlayContext` in isolation, used by
/// this module's own tests and by the concrete card kinds' tests
/// (`card::movement`, `card::movement_modifier`, ...).
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::board::{BoardTopology, PlayerColor, SpaceId};
    use crate::context::PlayContext;
    use crate::pawn::{Pawn, PawnId, PersistentEffectState};
    use crate::rules::RuleConfig;

    /// Owns everything a `PlayContext` borrows from, so tests don't need to
    /// juggle the lifetimes themselves.
    pub(crate) struct TestContext {
        topology: BoardTopology,
        rules: RuleConfig,
        catalog: CardCatalog,
        pawns: Vec<Pawn>,
        space_effects: HashMap<SpaceId, Vec<PersistentEffectState>>,
        mover: PawnId,
    }

    impl TestContext {
        pub(crate) fn ctx(&mut self) -> PlayContext<'_> {
            PlayContext::new(
                &self.topology,
                &self.rules,
                &self.catalog,
                &mut self.pawns,
                &mut self.space_effects,
                self.mover,
            )
        }
    }

    /// A minimal, valid `TestContext`: one pawn, sitting in its own yard,
    /// on a small standard board. Position doesn't matter for tests that
    /// only exercise `on_claimed`'s effect on a `MovementProposal`.
    pub(crate) fn test_context() -> TestContext {
        let topology = BoardTopology::standard_ring(2, 8, 3, 2).unwrap();
        let rules = crate::rules::minimal_rules();
        let catalog = CardCatalog::standard();
        let mover = PawnId(0);
        let pawns = vec![crate::pawn::tests::bare_pawn(
            mover,
            PlayerColor(0),
            topology.yard_spaces(PlayerColor(0))[0],
        )];
        TestContext {
            topology,
            rules,
            catalog,
            pawns,
            space_effects: HashMap::new(),
            mover,
        }
    }

    struct NoOpCard;
    impl CardBehavior for NoOpCard {}

    #[test]
    fn standard_catalog_registers_the_movement_cards() {
        let catalog = CardCatalog::standard();
        assert_eq!(
            catalog.get(CardKindId(3)).map(|m| m.display_name),
            Some("Take 4")
        );
        assert_eq!(
            catalog.get(CardKindId(4)).map(|m| m.display_name),
            Some("Double")
        );
        assert_eq!(
            catalog.get(CardKindId(5)).map(|m| m.display_name),
            Some("Rampage")
        );
        assert!(catalog.get(CardKindId(6)).is_none());
    }

    #[test]
    fn catalog_get_finds_a_registered_card_and_nothing_else() {
        let catalog = CardCatalog {
            definitions: vec![CardMeta {
                id: CardKindId(0),
                display_name: "No-Op",
                category: CardCategory::Movement,
                behavior: Box::new(NoOpCard),
            }],
        };
        assert_eq!(
            catalog.get(CardKindId(0)).map(|m| m.display_name),
            Some("No-Op")
        );
        assert!(catalog.get(CardKindId(1)).is_none());
    }

    #[test]
    fn move_double_and_rampage_combine_into_one_proposal() {
        use movement::MoveCard;
        use movement_modifier::{DoubleModifierCard, RampageModifierCard};

        let mut fixture = test_context();
        let mut proposal = MovementProposal::default();
        {
            let mut ctx = fixture.ctx();
            MoveCard { steps: 3 }.on_claimed(&mut ctx, &mut proposal);
            DoubleModifierCard { multiplier: 2 }.on_claimed(&mut ctx, &mut proposal);
            RampageModifierCard.on_claimed(&mut ctx, &mut proposal);
        }
        assert_eq!(proposal.steps, 3);
        assert_eq!(proposal.multiplier, 2);
        assert_eq!(
            proposal.capture_mode,
            crate::context::CaptureMode::EveryStepPassed
        );
    }

    #[test]
    fn a_bluffed_multi_card_claim_moves_the_pawn_by_the_claimed_distance() {
        use crate::board::NextSpace;
        use crate::play::{Declaration, PlayedCard};

        let mut fixture = test_context();
        let catalog = CardCatalog::standard();

        // Claims "Take 4" + "Double" (8 steps combined) but only "Take 1"
        // was truly played. Nothing here catches that lie yet — that's
        // audit.rs, a later build-order step — this just proves the board
        // reflects the claim, not the truth, which is the whole point of
        // bluffing: `on_claimed` only ever sees `claimed_cards`.
        let declaration = Declaration {
            pawn: fixture.mover,
            claimed_cards: vec![CardKindId(3), CardKindId(4)],
        };
        let played = PlayedCard {
            declaration,
            actual_cards: vec![CardKindId(0)],
        };

        let mut proposal = MovementProposal::default();
        {
            let mut ctx = fixture.ctx();
            for claimed in &played.declaration.claimed_cards {
                let card = catalog
                    .get(*claimed)
                    .expect("claimed card should be in the catalog");
                card.behavior.on_claimed(&mut ctx, &mut proposal);
            }
        }
        assert_eq!(proposal.steps, 4);
        assert_eq!(proposal.multiplier, 2);

        let start = fixture.pawns[0].position;
        {
            let mut ctx = fixture.ctx();
            ctx.resolve_movement(proposal).unwrap();
        }

        let mut expected = start;
        for _ in 0..8 {
            expected = match fixture
                .topology
                .next_space(expected, PlayerColor(0))
                .unwrap()
            {
                NextSpace::Single(space) => space,
                other => panic!("expected a single ring step, got {other:?}"),
            };
        }
        assert_eq!(fixture.pawns[0].position, expected);
    }
}
