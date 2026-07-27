use crate::card::CardBehavior;
use crate::context::{MovementProposal, PlayContext};

/// A card claiming a plain number of steps.
pub struct MoveCard {
    pub steps: u8,
}

impl CardBehavior for MoveCard {
    fn on_claimed(&self, _ctx: &mut PlayContext, proposal: &mut MovementProposal) {
        proposal.steps += self.steps;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::tests::test_context;

    #[test]
    fn adds_its_steps_to_the_proposal() {
        let mut fixture = test_context();
        let mut proposal = MovementProposal::default();
        MoveCard { steps: 4 }.on_claimed(&mut fixture.ctx(), &mut proposal);
        assert_eq!(proposal.steps, 4);
    }

    #[test]
    fn multiple_move_cards_accumulate() {
        let mut fixture = test_context();
        let mut proposal = MovementProposal::default();
        MoveCard { steps: 4 }.on_claimed(&mut fixture.ctx(), &mut proposal);
        MoveCard { steps: 1 }.on_claimed(&mut fixture.ctx(), &mut proposal);
        assert_eq!(proposal.steps, 5);
    }
}
