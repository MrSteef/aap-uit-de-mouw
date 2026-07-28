//! `InteractionContext`: the restricted API surface a card's pass/capture
//! hooks act through.

use crate::board::BoardTopology;
use crate::event::GameEvent;
use crate::pawn::{self, AutomaticAuditCatch, Pawn, PawnId};
use crate::rules::RuleConfig;

/// The context a card's pass-through and capture-attempt hooks act
/// through.
pub struct InteractionContext<'a> {
    pub attacker: PawnId,
    pub defender: PawnId,
    /// `false` = a mid-path square being passed through; `true` = the
    /// move's final resting square.
    pub is_landing: bool,
    topology: &'a BoardTopology,
    rules: &'a RuleConfig,
    pawns: &'a mut Vec<Pawn>,
    events: &'a mut Vec<GameEvent>,
    /// Where a caught automatic-audit lie gets recorded — borrowed from
    /// whoever's accumulating them for the whole play (`PlayContext`),
    /// since routing the resulting cards touches the wider game economy,
    /// which this context has no reach into.
    catches: &'a mut Vec<AutomaticAuditCatch>,
    /// Which move `trigger_automatic_audit` should test if called right
    /// now — set by `begin_effect` before each hook dispatch, since a
    /// card's own struct carries no per-instance state to say which
    /// attached effect (and thus which move) it's currently being asked
    /// about.
    current_source_move: Option<u64>,
    /// Every source move already tested this capture attempt — keyed per
    /// move rather than a single flag, since a pawn could (in principle)
    /// carry more than one independent effect from different moves, and
    /// each still needs testing exactly once. Idempotency only needs to
    /// cover the *same* move being tested twice (once from the played
    /// side, once from the claimed side) — different moves are never
    /// deduped against each other.
    resolved_moves: Vec<u64>,
}

impl<'a> InteractionContext<'a> {
    /// Builds a context for one capture attempt (or pass-through) between
    /// `attacker` and `defender`.
    #[allow(
        clippy::too_many_arguments,
        reason = "a plain constructor threading borrowed context through is clearer here than a wrapper struct built just to dodge this lint"
    )]
    pub fn new(
        attacker: PawnId,
        defender: PawnId,
        is_landing: bool,
        topology: &'a BoardTopology,
        rules: &'a RuleConfig,
        pawns: &'a mut Vec<Pawn>,
        events: &'a mut Vec<GameEvent>,
        catches: &'a mut Vec<AutomaticAuditCatch>,
    ) -> Self {
        Self {
            attacker,
            defender,
            is_landing,
            topology,
            rules,
            pawns,
            events,
            catches,
            current_source_move: None,
            resolved_moves: Vec::new(),
        }
    }

    /// Declares which move the next hook dispatch is about — called by
    /// `PlayContext::attempt_capture` before each `on_capture_attempted_
    /// as_played`/`_as_claimed` call, so `trigger_automatic_audit` knows
    /// which of the defender's moves it's actually being asked to test.
    pub fn begin_effect(&mut self, source_move: u64) {
        self.current_source_move = Some(source_move);
    }

    /// Marks whichever effect is currently being tested as publicly known.
    pub fn reveal_publicly(&mut self) {
        todo!(
            "nothing drives this yet — no card calls it (ShieldCard uses trigger_automatic_audit instead)"
        )
    }

    /// Tests the specific move that attached the effect currently under
    /// test (set via `begin_effect`): if its claimed and actual cards
    /// differ, reverts it (and reinstates anything it captured along the
    /// way, per `RuleConfig::revert_captures_on_lie`) exactly like a
    /// deliberate audit would — except nobody chose to gamble here, so no
    /// penalty applies to either side if the claim turns out true. A
    /// caught lie is recorded via `catches` rather than acted on further
    /// here — see `AutomaticAuditCatch`. Either way, resolves the
    /// defender's claimed effect from that same move, if one is still
    /// outstanding — the claim has now been tested, one way or another.
    ///
    /// If the source move has already aged out of the defender's audit
    /// window (evicted by newer moves since the effect was attached),
    /// there's nothing left to test — treated the same as a settled claim.
    ///
    /// Idempotent per move, not just per capture attempt: whichever of
    /// `on_capture_attempted_as_played`/`_as_claimed` tests a given move
    /// first resolves it; a second call *for that same move* (e.g. the
    /// paired hook, when a real effect and a matching claim share the same
    /// source move) is a no-op. A *different* move — from another,
    /// independent effect on the same pawn — is not deduped against it.
    pub fn trigger_automatic_audit(&mut self) {
        let source_move = self.current_source_move.expect(
            "trigger_automatic_audit called without begin_effect — see InteractionContext::begin_effect",
        );
        if self.resolved_moves.contains(&source_move) {
            return;
        }
        self.resolved_moves.push(source_move);

        let Some(target_index) = self.pawns.iter().position(|pawn| pawn.id == self.defender) else {
            return;
        };
        self.pawns[target_index].resolve_claimed_effect(source_move);
        let Some((move_index, is_a_lie)) = self.pawns[target_index]
            .auditable_moves()
            .find(|(_, record)| record.sequence == source_move)
            .map(|(index, record)| (index, record.is_a_lie()))
        else {
            return;
        };
        if is_a_lie {
            let reversion = pawn::revert(
                self.pawns,
                self.topology,
                target_index,
                move_index,
                self.rules.revert_captures_on_lie,
            );
            self.catches.push(AutomaticAuditCatch {
                attacker: self.attacker,
                defender: self.defender,
                reversion,
            });
        }
    }

    /// Records an event as having happened during this interaction.
    pub fn emit(&mut self, event: GameEvent) {
        self.events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{PlayerColor, SpaceId};
    use crate::card::CardKindId;
    use crate::pawn::tests::bare_pawn;
    use crate::pawn::{MoveRecord, RevealScope};
    use crate::rules::minimal_rules;

    fn board() -> BoardTopology {
        BoardTopology::standard_ring(2, 8, 3, 2).unwrap()
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
    fn emit_records_the_event() {
        let topology = board();
        let rules = minimal_rules();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0))];
        let mut events = Vec::new();
        let mut catches = Vec::new();
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
            &mut catches,
        );
        ctx.emit(GameEvent::PawnCaptured {
            pawn: PawnId(0),
            by: PawnId(1),
        });
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn automatic_audit_reverts_the_defenders_lie_and_records_a_catch() {
        let topology = board();
        let rules = minimal_rules();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6))];
        pawns[0].push_move(record(vec![9], vec![1], 5, 6), 5);
        let mut events = Vec::new();
        let mut catches = Vec::new();
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
            &mut catches,
        );

        ctx.begin_effect(0);
        ctx.trigger_automatic_audit();

        assert_eq!(pawns[0].position, SpaceId(5));
        assert_eq!(pawns[0].auditable_moves().count(), 0);
        assert_eq!(catches.len(), 1);
        assert_eq!(catches[0].attacker, PawnId(1));
        assert_eq!(catches[0].defender, PawnId(0));
        assert_eq!(
            catches[0].reversion.directly_reverted_cards,
            vec![CardKindId(1)]
        );
    }

    #[test]
    fn automatic_audit_does_nothing_when_the_claim_was_true() {
        let topology = board();
        let rules = minimal_rules();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6))];
        pawns[0].push_move(record(vec![1], vec![1], 5, 6), 5);
        let mut events = Vec::new();
        let mut catches = Vec::new();
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
            &mut catches,
        );

        ctx.begin_effect(0);
        ctx.trigger_automatic_audit();

        assert_eq!(pawns[0].position, SpaceId(6));
        assert_eq!(pawns[0].auditable_moves().count(), 1);
        assert!(catches.is_empty());
    }

    #[test]
    fn automatic_audit_is_idempotent_for_the_same_move() {
        let topology = board();
        let rules = minimal_rules();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6))];
        pawns[0].push_move(record(vec![9], vec![1], 5, 6), 5);
        let mut events = Vec::new();
        let mut catches = Vec::new();
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
            &mut catches,
        );

        ctx.begin_effect(0);
        ctx.trigger_automatic_audit();
        // A second call for the *same* move (as happens when both the
        // played and claimed hooks fire for one shared move) must not try
        // to revert an already-empty history again, and must not record a
        // second catch.
        ctx.begin_effect(0);
        ctx.trigger_automatic_audit();

        assert_eq!(pawns[0].position, SpaceId(5));
        assert_eq!(catches.len(), 1);
    }

    #[test]
    fn automatic_audit_tests_the_move_that_attached_the_effect_not_the_newest_one() {
        // Regression test for the "tests the newest move" bug: a pawn
        // bluffs (move 0, a lie), then plays a second, truthful move
        // (move 1) with the same pawn before anyone attacks. An automatic
        // audit triggered by an effect attached at move 0 must still catch
        // move 0's lie, even though move 1 is now the newest one in
        // history.
        let topology = board();
        let rules = minimal_rules();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6))];
        pawns[0].push_move(record(vec![9], vec![1], 5, 6), 5); // move 0: a lie
        pawns[0].push_move(record(vec![2], vec![2], 6, 8), 5); // move 1: truthful
        let mut events = Vec::new();
        let mut catches = Vec::new();
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
            &mut catches,
        );

        // The effect under test was attached by move 0, not move 1 (the
        // newest).
        ctx.begin_effect(0);
        ctx.trigger_automatic_audit();

        assert_eq!(catches.len(), 1);
        assert_eq!(
            catches[0].reversion.directly_reverted_cards,
            vec![CardKindId(1)]
        );
        // Reverting move 0 also sweeps up move 1, since it happened after.
        assert_eq!(catches[0].reversion.swept_up_cards, vec![CardKindId(2)]);
        assert_eq!(pawns[0].position, SpaceId(5));
    }

    #[test]
    fn automatic_audit_treats_an_aged_out_source_move_as_settled() {
        let topology = board();
        let rules = minimal_rules();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6))];
        pawns[0].push_move(record(vec![9], vec![1], 5, 6), 1); // move 0: a lie, window 1
        pawns[0].push_move(record(vec![2], vec![2], 6, 8), 1); // move 1 evicts move 0
        let mut events = Vec::new();
        let mut catches = Vec::new();
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
            &mut catches,
        );

        // Move 0 (where the effect was attached) is no longer in the
        // window at all — nothing to test, nothing to catch. Position is
        // untouched either way: only `revert` ever changes it, and
        // nothing here calls it.
        ctx.begin_effect(0);
        ctx.trigger_automatic_audit();

        assert!(catches.is_empty());
        assert_eq!(pawns[0].position, SpaceId(6));
        assert_eq!(pawns[0].auditable_moves().count(), 1);
    }

    #[test]
    fn automatic_audit_does_not_dedupe_across_different_moves() {
        let topology = board();
        let rules = minimal_rules();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6))];
        pawns[0].push_move(record(vec![9], vec![1], 5, 6), 5); // move 0: a lie
        pawns[0].push_move(record(vec![9], vec![2], 6, 8), 5); // move 1: also a lie
        let mut events = Vec::new();
        let mut catches = Vec::new();
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
            &mut catches,
        );

        // Test move 1 (the newer one) first — reverting it only removes
        // itself, since nothing comes after it yet. Move 0 is unaffected
        // and still independently testable afterward. (Testing move 0
        // first would instead sweep move 1 up as part of its own revert,
        // which is correct cascading behavior but isn't what this test is
        // isolating.)
        ctx.begin_effect(1);
        ctx.trigger_automatic_audit();
        ctx.begin_effect(0);
        ctx.trigger_automatic_audit();

        assert_eq!(catches.len(), 2);
        assert_eq!(
            catches[0].reversion.directly_reverted_cards,
            vec![CardKindId(2)]
        );
        assert_eq!(
            catches[1].reversion.directly_reverted_cards,
            vec![CardKindId(1)]
        );
    }
}
