//! `InteractionContext`: the restricted API surface a card's pass/capture
//! hooks act through.

use crate::event::GameEvent;
use crate::pawn::PawnId;

/// The context a card's pass-through and capture-attempt hooks act
/// through.
pub struct InteractionContext<'a> {
    pub attacker: PawnId,
    pub defender: PawnId,
    /// `false` = a mid-path square being passed through; `true` = the
    /// move's final resting square.
    pub is_landing: bool,
    events: &'a mut Vec<GameEvent>,
}

impl<'a> InteractionContext<'a> {
    /// Marks whichever effect is currently being tested as publicly known.
    pub fn reveal_publicly(&mut self) {
        todo!("needs the real/claimed effect lists on Pawn, added in build-order step 6")
    }

    /// Compares the outstanding claimed-vs-actual card for whatever is
    /// currently being tested and, if they differ, applies the same
    /// cascading-revert consequences as a deliberate audit. Idempotent per
    /// capture attempt.
    pub fn trigger_automatic_audit(&mut self) {
        todo!("needs audit.rs's revert logic, added in build-order step 6")
    }

    /// Records an event as having happened during this interaction.
    pub fn emit(&mut self, event: GameEvent) {
        self.events.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_records_the_event() {
        let mut events = Vec::new();
        let mut ctx = InteractionContext {
            attacker: PawnId(0),
            defender: PawnId(1),
            is_landing: true,
            events: &mut events,
        };
        ctx.emit(GameEvent::PawnCaptured {
            pawn: PawnId(1),
            by: PawnId(0),
        });
        assert_eq!(events.len(), 1);
    }
}
