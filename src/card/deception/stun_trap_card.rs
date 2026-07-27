use crate::card::{AuditOutcome, CardBehavior};
use crate::context::AuditContext;

/// A card built to punish curiosity: challenging a move where this card
/// was the one truly played costs the challenger their next turn
/// outright, regardless of whether the challenge also caught a lie.
pub struct StunTrapCard;

impl CardBehavior for StunTrapCard {
    fn on_audited_as_played(&self, _outcome: AuditOutcome, ctx: &mut AuditContext) {
        ctx.forfeit_auditor_turn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pawn::PawnId;
    use crate::player::PlayerId;

    #[test]
    fn forfeits_the_auditors_turn_regardless_of_outcome() {
        for outcome in [AuditOutcome::ClaimWasTrue, AuditOutcome::LieCaught] {
            let mut forfeit = false;
            {
                let mut ctx = AuditContext::new(PlayerId(0), PlayerId(1), PawnId(0), &mut forfeit);
                StunTrapCard.on_audited_as_played(outcome, &mut ctx);
            }
            assert!(forfeit, "expected a forfeit for {outcome:?}");
        }
    }

    #[test]
    fn on_audited_as_claimed_does_not_forfeit() {
        let mut forfeit = false;
        {
            let mut ctx = AuditContext::new(PlayerId(0), PlayerId(1), PawnId(0), &mut forfeit);
            StunTrapCard.on_audited_as_claimed(AuditOutcome::LieCaught, &mut ctx);
        }
        assert!(!forfeit);
    }
}
