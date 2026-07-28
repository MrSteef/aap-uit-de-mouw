use crate::card::CardBehavior;
use crate::context::{CaptureOutcome, InteractionContext, MovementProposal, PlayContext};
use crate::pawn::{EffectAnchor, ExpiryCondition};

/// How long a played Shield stays active — a property of the specific
/// card, not a `RuleConfig` toggle: different `CardKindId`s in the catalog
/// can each wrap `ShieldCard` with a different duration, so "1-turn
/// Shield" and "until it ages out Shield" can coexist as distinct cards.
///
/// ARCHITECTURE.md §5 defines a separate `ShieldDuration` enum
/// (`Turns(u8)`/`UntilPawnMoves`/`UntilHistoryExpires`) with the same
/// three variants as `pawn::ExpiryCondition`
/// (`AfterTurns(u8)`/`OnPawnMoved`/`WithSourceHistoryItem`). Rather than
/// keep two identical types and convert between them, `ShieldCard` just
/// uses `ExpiryCondition` directly.
pub struct ShieldCard {
    pub duration: ExpiryCondition,
}

impl CardBehavior for ShieldCard {
    fn on_played(&self, ctx: &mut PlayContext) {
        ctx.attach_persistent_effect(EffectAnchor::Pawn(ctx.mover()), Some(self.duration));
    }

    fn on_claimed(&self, ctx: &mut PlayContext, _proposal: &mut MovementProposal) {
        ctx.attach_claimed_effect(EffectAnchor::Pawn(ctx.mover()));
    }

    /// The real card being tested — apply its actual effect, and trigger
    /// the automatic audit so the corresponding claim (if any) resolves as
    /// a side effect of being tested.
    fn on_capture_attempted_as_played(&self, ctx: &mut InteractionContext) -> CaptureOutcome {
        ctx.trigger_automatic_audit();
        CaptureOutcome::Blocked
    }

    /// Only ever reached when there's a claimed Shield with no real one
    /// backing it — `attempt_capture`'s independent checks mean the played
    /// hook above already handles the case where a real one exists. All
    /// this needs to do is reveal that via the same audit trigger; it never
    /// blocks anything itself.
    fn on_capture_attempted_as_claimed(&self, ctx: &mut InteractionContext) -> CaptureOutcome {
        ctx.trigger_automatic_audit();
        CaptureOutcome::Proceeds
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{PlayerColor, SpaceId};
    use crate::card::tests::test_context;
    use crate::card::{CardCatalog, CardCategory, CardKindId, CardMeta};
    use crate::pawn::{MoveRecord, PawnId, RevealScope, tests::bare_pawn};
    use crate::rules::minimal_rules;
    use std::collections::HashMap;

    fn catalog_with_shield(id: CardKindId) -> CardCatalog {
        CardCatalog::from_definitions(vec![CardMeta {
            id,
            display_name: "Shield",
            category: CardCategory::Defense,
            behavior: Box::new(ShieldCard {
                duration: ExpiryCondition::OnPawnMoved,
            }),
        }])
    }

    fn record(claimed: Vec<u16>, actual: Vec<u16>, before: u32, after: u32) -> MoveRecord {
        MoveRecord {
            sequence: 0, // overwritten by push_move
            claimed_cards: claimed.into_iter().map(CardKindId).collect(),
            actual_cards: actual.into_iter().map(CardKindId).collect(),
            position_before: SpaceId(before),
            position_after: SpaceId(after),
            captures_caused: Vec::new(),
            reveal: RevealScope::Hidden,
        }
    }

    #[test]
    fn on_played_attaches_a_real_effect_anchored_to_the_mover() {
        let mut fixture = test_context();
        let mover = {
            let mut ctx = fixture.ctx();
            ctx.begin_card(CardKindId(0));
            ShieldCard {
                duration: ExpiryCondition::OnPawnMoved,
            }
            .on_played(&mut ctx);
            ctx.mover()
        };
        assert_eq!(fixture.pawns()[0].id, mover);
        assert_eq!(fixture.pawns()[0].persistent_effects().len(), 1);
        assert_eq!(
            fixture.pawns()[0].persistent_effects()[0].anchor,
            EffectAnchor::Pawn(mover)
        );
    }

    #[test]
    fn on_claimed_attaches_a_claimed_effect_anchored_to_the_mover() {
        let mut fixture = test_context();
        let mut proposal = MovementProposal::default();
        {
            let mut ctx = fixture.ctx();
            ctx.begin_card(CardKindId(0));
            ShieldCard {
                duration: ExpiryCondition::OnPawnMoved,
            }
            .on_claimed(&mut ctx, &mut proposal);
        }
        assert_eq!(fixture.pawns()[0].claimed_effects().len(), 1);
        // Shield doesn't contribute to movement.
        assert_eq!(proposal.steps, 0);
    }

    /// Scenario B from ARCHITECTURE.md §14: Green truthfully plays Shield
    /// on P1. Yellow's move attempts to land on P1 — the real effect
    /// blocks it, and the automatic audit finds the claim was true, so
    /// nothing is undone.
    #[test]
    fn truthful_shield_blocks_capture_and_the_claim_holds_up() {
        let shield_id = CardKindId(0);
        let topology = crate::board::BoardTopology::standard_ring(2, 8, 3, 2).unwrap();
        let rules = minimal_rules();
        let catalog = catalog_with_shield(shield_id);
        let defender = PawnId(0);
        let attacker = PawnId(1);
        let mut pawns = vec![
            bare_pawn(defender, PlayerColor(0), SpaceId(6)),
            bare_pawn(attacker, PlayerColor(1), SpaceId(5)),
        ];
        let mut space_effects = HashMap::new();

        {
            let mut ctx = PlayContext::new(
                &topology,
                &rules,
                &catalog,
                &mut pawns,
                &mut space_effects,
                defender,
            );
            ctx.begin_card(shield_id);
            ShieldCard {
                duration: ExpiryCondition::OnPawnMoved,
            }
            .on_played(&mut ctx);
            let mut proposal = MovementProposal::default();
            ShieldCard {
                duration: ExpiryCondition::OnPawnMoved,
            }
            .on_claimed(&mut ctx, &mut proposal);
        }
        // The move this play produced: truthfully claimed and played Shield.
        pawns[0].push_move(record(vec![shield_id.0], vec![shield_id.0], 5, 6), 5);

        let outcome = {
            let mut ctx = PlayContext::new(
                &topology,
                &rules,
                &catalog,
                &mut pawns,
                &mut space_effects,
                attacker,
            );
            ctx.attempt_capture(defender, true)
        };

        assert_eq!(outcome, CaptureOutcome::Blocked);
        assert_eq!(
            pawns[0].persistent_effects().len(),
            1,
            "a genuine Shield keeps blocking, it isn't consumed"
        );
        assert!(
            pawns[0].claimed_effects().is_empty(),
            "the tested claim is resolved either way"
        );
        assert_eq!(
            pawns[0].position,
            SpaceId(6),
            "ClaimWasTrue: nothing reverts"
        );
        assert_eq!(
            pawns[0].auditable_moves().count(),
            1,
            "the honest move stays in history"
        );
    }

    /// Contrast case from the same scenario: Green only *claims* Shield
    /// without truly playing it. The claimed effect alone triggers the
    /// automatic audit, catches the lie (undoing the bluffed move), and the
    /// capture still proceeds.
    #[test]
    fn bluffed_shield_is_exposed_and_the_capture_still_proceeds() {
        let shield_id = CardKindId(0);
        let real_card_id = CardKindId(1);
        let topology = crate::board::BoardTopology::standard_ring(2, 8, 3, 2).unwrap();
        let rules = minimal_rules();
        let catalog = catalog_with_shield(shield_id);
        let defender = PawnId(0);
        let attacker = PawnId(1);
        let mut pawns = vec![
            bare_pawn(defender, PlayerColor(0), SpaceId(6)),
            bare_pawn(attacker, PlayerColor(1), SpaceId(5)),
        ];
        let mut space_effects = HashMap::new();

        {
            let mut ctx = PlayContext::new(
                &topology,
                &rules,
                &catalog,
                &mut pawns,
                &mut space_effects,
                defender,
            );
            ctx.begin_card(shield_id);
            let mut proposal = MovementProposal::default();
            // Only claimed — on_played is never called, so no real effect exists.
            ShieldCard {
                duration: ExpiryCondition::OnPawnMoved,
            }
            .on_claimed(&mut ctx, &mut proposal);
        }
        pawns[0].push_move(record(vec![shield_id.0], vec![real_card_id.0], 5, 6), 5);

        let outcome = {
            let mut ctx = PlayContext::new(
                &topology,
                &rules,
                &catalog,
                &mut pawns,
                &mut space_effects,
                attacker,
            );
            ctx.attempt_capture(defender, true)
        };

        assert_eq!(outcome, CaptureOutcome::Proceeds);
        assert!(pawns[0].claimed_effects().is_empty());
        assert_eq!(
            pawns[0].auditable_moves().count(),
            0,
            "the bluffed move is reverted out of history"
        );
        assert_eq!(
            pawns[0].position,
            topology.yard_spaces(PlayerColor(0))[0],
            "capture proceeded, sending it to yard"
        );
    }

    /// Regression test for the "tests the newest move" bug: before linking
    /// an effect to the specific move that created it, a bluffed Shield
    /// claim could be "laundered" by playing one more truthful move with
    /// the same pawn before anyone attacked — the automatic audit would
    /// test that newer, honest move instead and find nothing wrong.
    #[test]
    fn bluffed_shield_is_still_caught_after_a_later_truthful_move() {
        let shield_id = CardKindId(0);
        let real_card_id = CardKindId(1);
        let topology = crate::board::BoardTopology::standard_ring(2, 8, 3, 2).unwrap();
        let rules = minimal_rules();
        let catalog = catalog_with_shield(shield_id);
        let defender = PawnId(0);
        let attacker = PawnId(1);
        let mut pawns = vec![
            bare_pawn(defender, PlayerColor(0), SpaceId(6)),
            bare_pawn(attacker, PlayerColor(1), SpaceId(5)),
        ];
        let mut space_effects = HashMap::new();

        // Move 0: claims Shield, actually plays something else — a lie,
        // and (since Shield's on_claimed contributes no steps) the pawn
        // doesn't move.
        {
            let mut ctx = PlayContext::new(
                &topology,
                &rules,
                &catalog,
                &mut pawns,
                &mut space_effects,
                defender,
            );
            ctx.begin_card(shield_id);
            let mut proposal = MovementProposal::default();
            ShieldCard {
                duration: ExpiryCondition::OnPawnMoved,
            }
            .on_claimed(&mut ctx, &mut proposal);
        }
        pawns[0].push_move(record(vec![shield_id.0], vec![real_card_id.0], 6, 6), 5);

        // Move 1: an unrelated, entirely truthful move with the same pawn.
        pawns[0].push_move(record(vec![2], vec![2], 6, 7), 5);

        // Now the attacker attempts a capture. The claimed Shield is still
        // outstanding from move 0 — the automatic audit must test *that*
        // move, not move 1 (the newest), to catch the bluff.
        let outcome = {
            let mut ctx = PlayContext::new(
                &topology,
                &rules,
                &catalog,
                &mut pawns,
                &mut space_effects,
                attacker,
            );
            ctx.attempt_capture(defender, true)
        };

        assert_eq!(outcome, CaptureOutcome::Proceeds);
        assert!(pawns[0].claimed_effects().is_empty());
        // Both moves get swept away by reverting move 0 (the bluff),
        // leaving the pawn back at its pre-bluff position, then captured.
        assert_eq!(pawns[0].auditable_moves().count(), 0);
        assert_eq!(
            pawns[0].position,
            topology.yard_spaces(PlayerColor(0))[0],
            "the exposed bluff reverts both moves, then the real capture proceeds"
        );
    }
}
