//! Pure modifiers: they upgrade whatever movement is already being
//! claimed rather than carrying their own step count.

mod double_modifier_card;
mod rampage_modifier_card;

pub use double_modifier_card::DoubleModifierCard;
pub use rampage_modifier_card::RampageModifierCard;
