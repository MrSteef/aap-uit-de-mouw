//! Every optional rule — from which tradition's mechanics apply, to every
//! cost, reward, and cap in the card economy — lives here as data, not as a
//! branch buried somewhere in the engine.

use std::collections::HashMap;

use bon::Builder;

use crate::card::{CardCategory, CardKindId};

/// The full set of tunable rules for one game.
#[derive(Builder, Clone, Debug)]
pub struct RuleConfig {
    pub pawn_count: u8,
    pub exit_rule: ExitRule,
    pub blockades_enabled: bool,
    pub capture_sends_to_yard: bool,
    pub bonus_turn_on_capture: bool,
    pub bonus_turn_on_exit: bool,
    pub exact_count_to_finish: bool,

    pub audit_window: u8,
    pub max_audits_per_turn: u8,
    pub revert_captures_on_lie: bool,
    /// Paid unconditionally, the moment a challenge is submitted, regardless
    /// of whether it turns out right or wrong.
    pub audit_attempt_cost: u8,
    pub audit_attempt_cost_destination: CardDestination,
    pub audit_attempt_cost_selection: PaymentSelectionMode,
    /// How many *additional* cards a wrong accusation costs the challenger,
    /// on top of `audit_attempt_cost`.
    pub false_accusation_card_cost: u8,
    pub false_accusation_destination: CardDestination,
    pub false_accusation_selection: PaymentSelectionMode,
    /// Whether submitting a challenge consumes the challenger's entire turn.
    pub auditing_costs_turn: bool,
    /// Whether a captured pawn's pre-capture moves can still be actively
    /// challenged while it sits in the yard.
    pub captured_pawns_remain_auditable: bool,
    /// Whether the event log reveals exactly which cards were collected in
    /// an audit to every player, or only to the auditor and auditee.
    pub reveal_collected_cards_publicly: bool,

    pub playing_card_mandatory: bool,
    pub max_cards_per_play: u8,
    pub max_cards_per_category_per_play: HashMap<CardCategory, u8>,
    /// Whether a play's claimed card count may differ from its actual card
    /// count, not just the identities.
    pub allow_card_count_mismatch: bool,

    pub starting_deck_size: u8,
    pub starting_hand_size: u8,
    /// Draw target: topped up from a player's own reserve at the end of
    /// their turn.
    pub hand_soft_cap: u8,
    /// Absolute ceiling, checked only against external inflows.
    pub hand_hard_cap: u8,
    /// Reserve ceiling, checked the same way against external inflows.
    pub deck_cap: u8,
    /// Whether a pawn's aged-out history returning to its owner's own
    /// reserve may exceed `deck_cap`.
    pub aged_out_exempt_from_deck_cap: bool,

    pub starting_pile_size: u8,
    /// Cards granted to a player for successfully capturing an opponent's
    /// pawn, drawn from the shared pile.
    pub capture_reward_from_pile: u8,
    /// Whether cards from an automatically-caught bluff (like a fake
    /// Shield) go to the pile instead of whoever stumbled onto it.
    pub automatic_audit_reward_destination: AutomaticAuditCardDestination,
    /// Whether a pawn reaching Finish sends whatever cards are still tied
    /// to its recent history into the pile, or back to its owner's own
    /// reserve as normal.
    pub finished_pawn_dumps_history_destination: FinishedPawnHistoryDestination,
    /// Whether a big caught lie's cascaded-away (merely swept-up, not
    /// directly audited) cards go to the pile, or join the
    /// directly-audited move's cards in going to the auditor.
    pub cascade_lie_rewards_destination: CascadeSweepDestination,

    /// What happens, independently of anything else, the instant a
    /// player's hand and reserve are both completely empty.
    pub cards_exhausted_behavior: CardsExhaustedBehavior,
    /// What happens to a player who — having survived the check above —
    /// starts their turn with no legal action at all.
    pub no_available_action_behavior: NoAvailableActionBehavior,
}

/// How a color's pawns enter play from the yard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExitRule {
    Automatic,
    RequiresCard(CardKindId),
}

/// How the payer's cards are picked for either audit-related payment.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaymentSelectionMode {
    PayerChooses,
    RandomDraft,
}

/// Where a payment goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardDestination {
    SharedPile,
    Auditee,
}

/// What happens to a player whose hand and reserve are both empty.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardsExhaustedBehavior {
    Ignored,
    Eliminated(EliminatedPawnHandling),
}

/// What happens to a player with genuinely no legal action available.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoAvailableActionBehavior {
    AutoSkip,
    DrawCard(u8),
    Eliminated(EliminatedPawnHandling),
}

/// How an eliminated player's pawns behave for the rest of the game.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EliminatedPawnHandling {
    Frozen,
    Removed,
}

/// Where the cards from an automatically-caught bluff (e.g. an exposed
/// fake Shield) go: the shared pile, or the attacking player who triggered
/// the check by attempting a capture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AutomaticAuditCardDestination {
    SharedPile,
    Attacker,
}

/// Where a finishing pawn's still-attached history cards go: the shared
/// pile, or back to their owner's own reserve, same as a natural age-out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FinishedPawnHistoryDestination {
    SharedPile,
    OwnerReserve,
}

/// Where a big caught lie's cascaded-away (merely swept-up, not directly
/// audited) cards go: the shared pile, or along with the directly-audited
/// move's cards to the auditor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CascadeSweepDestination {
    SharedPile,
    Auditor,
}

/// A fully-populated `RuleConfig`, for tests elsewhere in the crate that
/// just need *some* valid rules and don't care about specific values.
#[cfg(test)]
pub(crate) fn minimal_rules() -> RuleConfig {
    RuleConfig::builder()
        .pawn_count(4)
        .exit_rule(ExitRule::Automatic)
        .blockades_enabled(true)
        .capture_sends_to_yard(true)
        .bonus_turn_on_capture(false)
        .bonus_turn_on_exit(false)
        .exact_count_to_finish(false)
        .audit_window(3)
        .max_audits_per_turn(1)
        .revert_captures_on_lie(true)
        .audit_attempt_cost(0)
        .audit_attempt_cost_destination(CardDestination::SharedPile)
        .audit_attempt_cost_selection(PaymentSelectionMode::RandomDraft)
        .false_accusation_card_cost(1)
        .false_accusation_destination(CardDestination::Auditee)
        .false_accusation_selection(PaymentSelectionMode::RandomDraft)
        .auditing_costs_turn(false)
        .captured_pawns_remain_auditable(true)
        .reveal_collected_cards_publicly(true)
        .playing_card_mandatory(true)
        .max_cards_per_play(2)
        .max_cards_per_category_per_play(HashMap::from([(CardCategory::Movement, 1)]))
        .allow_card_count_mismatch(false)
        .starting_deck_size(20)
        .starting_hand_size(5)
        .hand_soft_cap(5)
        .hand_hard_cap(8)
        .deck_cap(30)
        .aged_out_exempt_from_deck_cap(true)
        .starting_pile_size(10)
        .capture_reward_from_pile(2)
        .automatic_audit_reward_destination(AutomaticAuditCardDestination::SharedPile)
        .finished_pawn_dumps_history_destination(FinishedPawnHistoryDestination::SharedPile)
        .cascade_lie_rewards_destination(CascadeSweepDestination::SharedPile)
        .cards_exhausted_behavior(CardsExhaustedBehavior::Ignored)
        .no_available_action_behavior(NoAvailableActionBehavior::AutoSkip)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_rules() -> RuleConfig {
        minimal_rules()
    }

    #[test]
    fn builder_round_trips_every_field() {
        let rules = full_rules();
        assert_eq!(rules.pawn_count, 4);
        assert_eq!(rules.exit_rule, ExitRule::Automatic);
        assert!(rules.blockades_enabled);
        assert_eq!(rules.audit_window, 3);
        assert_eq!(
            rules.audit_attempt_cost_destination,
            CardDestination::SharedPile
        );
        assert_eq!(
            rules.false_accusation_selection,
            PaymentSelectionMode::RandomDraft
        );
        assert_eq!(
            rules
                .max_cards_per_category_per_play
                .get(&CardCategory::Movement),
            Some(&1)
        );
        assert_eq!(
            rules.cards_exhausted_behavior,
            CardsExhaustedBehavior::Ignored
        );
        assert_eq!(
            rules.no_available_action_behavior,
            NoAvailableActionBehavior::AutoSkip
        );
    }

    #[test]
    fn exit_rule_can_require_a_specific_card() {
        let rules = RuleConfig {
            exit_rule: ExitRule::RequiresCard(CardKindId(7)),
            ..full_rules()
        };
        assert_eq!(rules.exit_rule, ExitRule::RequiresCard(CardKindId(7)));
    }

    #[test]
    fn eliminated_variants_carry_their_pawn_handling() {
        assert_eq!(
            CardsExhaustedBehavior::Eliminated(EliminatedPawnHandling::Frozen),
            CardsExhaustedBehavior::Eliminated(EliminatedPawnHandling::Frozen)
        );
        assert_ne!(
            CardsExhaustedBehavior::Eliminated(EliminatedPawnHandling::Frozen),
            CardsExhaustedBehavior::Eliminated(EliminatedPawnHandling::Removed)
        );
        assert_eq!(
            NoAvailableActionBehavior::DrawCard(2),
            NoAvailableActionBehavior::DrawCard(2)
        );
    }
}
