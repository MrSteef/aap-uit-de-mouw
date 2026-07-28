//! Players as agents: `Player` (in `player.rs`) is plain data;
//! decision-making is a separate trait, so a random bot and a
//! deterministic scripted-for-tests bot are just different implementors.
//! A `HumanAgent` deliberately doesn't exist at this layer — real human
//! input belongs to the future Unity bridge, out of scope for this crate.

mod random_agent;
mod scripted_agent;

pub use random_agent::RandomAgent;
pub use scripted_agent::ScriptedAgent;

use crate::game::TurnAction;
use crate::view::GameView;

/// Chooses one action per call, given the current view and the legal
/// actions available.
pub trait PlayerAgent {
    fn choose_action(&mut self, view: &GameView, legal: &[TurnAction]) -> TurnAction;
}
