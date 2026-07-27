//! The claim/actual split that makes bluffing possible: what was
//! announced, and what really happened.

use crate::card::CardKindId;
use crate::pawn::PawnId;

/// What a player announces they're doing this turn.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Declaration {
    pub pawn: PawnId,
    pub claimed_cards: Vec<CardKindId>,
}

/// A resolved play: the announced claim, paired with the cards that were
/// truly consumed. `RuleConfig::max_cards_per_play` /
/// `max_cards_per_category_per_play` bound how large `claimed_cards` may
/// legally be — enforced wherever legal plays are enumerated, not by this
/// type itself.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlayedCard {
    pub declaration: Declaration,
    pub actual_cards: Vec<CardKindId>,
}
