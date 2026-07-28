use rand::RngExt;

use crate::agent::PlayerAgent;
use crate::game::TurnAction;
use crate::view::GameView;

/// Picks uniformly at random among the legal actions. Useful as a cheap
/// fuzz-testing opponent (ARCHITECTURE.md §15: "does the engine ever
/// panic or reach an inconsistent state") and as a stand-in bot.
pub struct RandomAgent;

impl PlayerAgent for RandomAgent {
    /// Panics if `legal` is empty. This can only happen for a player with
    /// a genuinely empty hand and no legal audit either — the
    /// `RuleConfig::no_available_action_behavior` gate that's supposed to
    /// handle that (ARCHITECTURE.md §3/§10) isn't implemented yet (see
    /// `game.rs`'s implementation-status note in ARCHITECTURE.md §13), so
    /// there's currently nothing sensible to return in that case.
    fn choose_action(&mut self, _view: &GameView, legal: &[TurnAction]) -> TurnAction {
        assert!(
            !legal.is_empty(),
            "RandomAgent has no legal action to choose from"
        );
        let mut rng = rand::rng();
        let index = rng.random_range(0..legal.len());
        legal[index].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::CardKindId;
    use crate::pawn::PawnId;
    use crate::play::{Declaration, PlayedCard};
    use crate::player::PlayerId;
    use crate::rules::minimal_rules;

    fn view() -> GameView {
        crate::view::build(&minimal_rules(), &[], &[], PlayerId(0))
    }

    #[test]
    fn always_returns_one_of_the_legal_actions() {
        let legal = vec![
            TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }),
            TurnAction::ForfeitCard(CardKindId(1)),
        ];
        let mut agent = RandomAgent;
        for _ in 0..20 {
            let chosen = agent.choose_action(&view(), &legal);
            assert!(legal.contains(&chosen));
        }
    }

    #[test]
    #[should_panic(expected = "no legal action")]
    fn panics_when_nothing_is_legal() {
        let mut agent = RandomAgent;
        agent.choose_action(&view(), &[]);
    }
}
