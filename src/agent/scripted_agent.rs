use std::collections::VecDeque;

use crate::agent::PlayerAgent;
use crate::game::TurnAction;
use crate::view::GameView;

/// A deterministic agent for golden-log tests: returns a pre-scripted
/// sequence of actions in order, ignoring `legal` (the test itself is
/// responsible for scripting a valid sequence for the game it's driving).
pub struct ScriptedAgent {
    actions: VecDeque<TurnAction>,
}

impl ScriptedAgent {
    pub fn new(actions: impl IntoIterator<Item = TurnAction>) -> Self {
        Self {
            actions: actions.into_iter().collect(),
        }
    }
}

impl PlayerAgent for ScriptedAgent {
    /// Panics if the script runs out — a test bug (the script is shorter
    /// than the number of turns it's asked to drive), not a game-state
    /// condition.
    fn choose_action(&mut self, _view: &GameView, _legal: &[TurnAction]) -> TurnAction {
        self.actions
            .pop_front()
            .expect("ScriptedAgent ran out of scripted actions")
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
    fn returns_scripted_actions_in_order() {
        let first = TurnAction::PlayCard(PlayedCard {
            declaration: Declaration {
                pawn: PawnId(0),
                claimed_cards: vec![CardKindId(0)],
            },
            actual_cards: vec![CardKindId(0)],
        });
        let second = TurnAction::ForfeitCard(CardKindId(1));
        let mut agent = ScriptedAgent::new([first.clone(), second.clone()]);

        assert_eq!(agent.choose_action(&view(), &[]), first);
        assert_eq!(agent.choose_action(&view(), &[]), second);
    }

    #[test]
    #[should_panic(expected = "ran out of scripted actions")]
    fn panics_once_the_script_is_exhausted() {
        let mut agent = ScriptedAgent::new(std::iter::empty());
        agent.choose_action(&view(), &[]);
    }
}
