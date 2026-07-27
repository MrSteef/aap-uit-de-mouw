use crate::card::CardBehavior;
use crate::context::{MovementProposal, PlayContext};

/// A pure modifier — no base steps of its own. Needs a `MoveCard` (or
/// another steps-contributing card) in the same play to do anything;
/// `RuleConfig::max_cards_per_category_per_play` bounds how many of these
/// one play may stack.
pub struct DoubleModifierCard {
    pub multiplier: u8,
}

impl CardBehavior for DoubleModifierCard {
    fn on_claimed(&self, _ctx: &mut PlayContext, proposal: &mut MovementProposal) {
        proposal.multiplier *= self.multiplier;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::movement::MoveCard;
    use crate::card::tests::test_context;

    #[test]
    fn multiplies_the_proposal() {
        let mut fixture = test_context();
        let mut proposal = MovementProposal::default();
        MoveCard { steps: 4 }.on_claimed(&mut fixture.ctx(), &mut proposal);
        DoubleModifierCard { multiplier: 2 }.on_claimed(&mut fixture.ctx(), &mut proposal);
        assert_eq!(proposal.steps, 4);
        assert_eq!(proposal.multiplier, 2);
    }

    #[test]
    fn stacking_doubles_multiplies_further() {
        let mut fixture = test_context();
        let mut proposal = MovementProposal::default();
        DoubleModifierCard { multiplier: 2 }.on_claimed(&mut fixture.ctx(), &mut proposal);
        DoubleModifierCard { multiplier: 2 }.on_claimed(&mut fixture.ctx(), &mut proposal);
        assert_eq!(proposal.multiplier, 4);
    }

    #[test]
    fn alone_it_does_nothing_useful_without_a_steps_card() {
        let mut fixture = test_context();
        let mut proposal = MovementProposal::default();
        DoubleModifierCard { multiplier: 2 }.on_claimed(&mut fixture.ctx(), &mut proposal);
        assert_eq!(proposal.steps, 0);
    }
}
