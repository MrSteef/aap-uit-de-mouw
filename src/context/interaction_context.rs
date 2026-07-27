//! `InteractionContext`: the restricted API surface a card's pass/capture
//! hooks act through.

use crate::board::BoardTopology;
use crate::event::GameEvent;
use crate::pawn::{self, Pawn, PawnId};
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
    /// Whichever of `on_capture_attempted_as_played`/`_as_claimed` calls
    /// `trigger_automatic_audit` first resolves it; the other's call, if
    /// any, is a no-op — see `trigger_automatic_audit`.
    automatic_audit_resolved: bool,
}

impl<'a> InteractionContext<'a> {
    /// Builds a context for one capture attempt (or pass-through) between
    /// `attacker` and `defender`.
    pub fn new(
        attacker: PawnId,
        defender: PawnId,
        is_landing: bool,
        topology: &'a BoardTopology,
        rules: &'a RuleConfig,
        pawns: &'a mut Vec<Pawn>,
        events: &'a mut Vec<GameEvent>,
    ) -> Self {
        Self {
            attacker,
            defender,
            is_landing,
            topology,
            rules,
            pawns,
            events,
            automatic_audit_resolved: false,
        }
    }

    /// Marks whichever effect is currently being tested as publicly known.
    pub fn reveal_publicly(&mut self) {
        todo!(
            "nothing drives this yet — no card calls it (ShieldCard uses trigger_automatic_audit instead)"
        )
    }

    /// Tests the defender's most recent move: if its claimed and actual
    /// cards differ, reverts it (and reinstates anything it captured along
    /// the way, per `RuleConfig::revert_captures_on_lie`) exactly like a
    /// deliberate audit would — except nobody chose to gamble here, so no
    /// penalty applies to either side if the claim turns out true. Either
    /// way, clears the defender's outstanding claimed effects — the claim
    /// has now been tested, one way or another. Idempotent per capture
    /// attempt: whichever of `on_capture_attempted_as_played`/`_as_claimed`
    /// calls this first resolves it; the other's call, if any, is a no-op.
    ///
    /// Simplification: this tests the defender's *newest* auditable move,
    /// not necessarily the specific move that attached the effect being
    /// tested. `PersistentEffectState`/`ClaimedEffectState` don't record
    /// which history entry created them — ARCHITECTURE.md §5 itself calls
    /// Shield's bookkeeping "not final answers," and linking an effect to
    /// a specific history index safely (indices shift as older moves age
    /// out) is a bigger structural change than this step's scope. In
    /// practice a capture attempt follows shortly after the relevant
    /// claim/play, so the newest move is almost always the right one.
    ///
    /// Also out of scope, matching `audit::resolve`'s own scoping: routing
    /// any collected cards to the shared pile
    /// (`RuleConfig::automatic_audit_reward_destination`) — that's
    /// `GameState`'s job (ARCHITECTURE.md §16 step 8).
    pub fn trigger_automatic_audit(&mut self) {
        if self.automatic_audit_resolved {
            return;
        }
        self.automatic_audit_resolved = true;

        let Some(target_index) = self.pawns.iter().position(|pawn| pawn.id == self.defender) else {
            return;
        };
        let Some((move_index, is_a_lie)) = self.pawns[target_index]
            .auditable_moves()
            .last()
            .map(|(index, record)| (index, record.is_a_lie()))
        else {
            return;
        };
        if is_a_lie {
            pawn::revert(
                self.pawns,
                self.topology,
                target_index,
                move_index,
                self.rules.revert_captures_on_lie,
            );
        }
        // The claim has now been tested, one way or another — see the doc
        // comment on `Pawn::claimed_effects`.
        self.pawns[target_index].clear_claimed_effects();
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
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
        );
        ctx.emit(GameEvent::PawnCaptured {
            pawn: PawnId(0),
            by: PawnId(1),
        });
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn automatic_audit_reverts_the_defenders_lie() {
        let topology = board();
        let rules = minimal_rules();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6))];
        pawns[0].push_move(record(vec![9], vec![1], 5, 6), 5);
        let mut events = Vec::new();
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
        );

        ctx.trigger_automatic_audit();

        assert_eq!(pawns[0].position, SpaceId(5));
        assert_eq!(pawns[0].auditable_moves().count(), 0);
    }

    #[test]
    fn automatic_audit_does_nothing_when_the_claim_was_true() {
        let topology = board();
        let rules = minimal_rules();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6))];
        pawns[0].push_move(record(vec![1], vec![1], 5, 6), 5);
        let mut events = Vec::new();
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
        );

        ctx.trigger_automatic_audit();

        assert_eq!(pawns[0].position, SpaceId(6));
        assert_eq!(pawns[0].auditable_moves().count(), 1);
    }

    #[test]
    fn automatic_audit_is_idempotent_per_capture_attempt() {
        let topology = board();
        let rules = minimal_rules();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6))];
        pawns[0].push_move(record(vec![9], vec![1], 5, 6), 5);
        let mut events = Vec::new();
        let mut ctx = InteractionContext::new(
            PawnId(1),
            PawnId(0),
            true,
            &topology,
            &rules,
            &mut pawns,
            &mut events,
        );

        ctx.trigger_automatic_audit();
        // A second call (as happens when both the played and claimed hooks
        // fire) must not try to revert an already-empty history again.
        ctx.trigger_automatic_audit();

        assert_eq!(pawns[0].position, SpaceId(5));
    }
}
