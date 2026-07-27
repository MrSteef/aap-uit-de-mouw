use crate::card::CardBehavior;
use crate::context::{CaptureMode, MovementProposal, PlayContext};

/// Also a pure modifier, for the same reason as Double — it upgrades
/// whatever movement is already being claimed rather than carrying its own
/// step count.
pub struct RampageModifierCard;

impl CardBehavior for RampageModifierCard {
    fn on_claimed(&self, _ctx: &mut PlayContext, proposal: &mut MovementProposal) {
        proposal.capture_mode = CaptureMode::EveryStepPassed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::tests::test_context;

    #[test]
    fn upgrades_capture_mode_to_every_step_passed() {
        let mut fixture = test_context();
        let mut proposal = MovementProposal::default();
        assert_eq!(proposal.capture_mode, CaptureMode::LandingSquareOnly);
        RampageModifierCard.on_claimed(&mut fixture.ctx(), &mut proposal);
        assert_eq!(proposal.capture_mode, CaptureMode::EveryStepPassed);
    }
}
