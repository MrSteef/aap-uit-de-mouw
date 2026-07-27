//! Players as plain data; decision-making is a separate concern (the
//! `PlayerAgent` trait, once `agent/` exists).

use crate::board::PlayerColor;
use crate::card::CardKindId;
use crate::deck::Deck;

/// Identifies one player.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PlayerId(pub u32);

/// One player's identity, hand, reserve, and score.
#[derive(Clone, Debug)]
pub struct Player {
    pub id: PlayerId,
    pub color: PlayerColor,
    pub hand: Vec<CardKindId>,
    pub deck: Deck,
    pub score: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::DeckComposition;

    #[test]
    fn player_carries_its_own_identity_hand_and_deck() {
        let player = Player {
            id: PlayerId(0),
            color: PlayerColor(0),
            hand: vec![CardKindId(1), CardKindId(2)],
            deck: Deck::new(&DeckComposition {
                counts: vec![(CardKindId(0), 4)],
            }),
            score: 0,
        };
        assert_eq!(player.id, PlayerId(0));
        assert_eq!(player.color, PlayerColor(0));
        assert_eq!(player.hand.len(), 2);
        assert_eq!(player.deck.len(), 4);
        assert_eq!(player.score, 0);
    }
}
