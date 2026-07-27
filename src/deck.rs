//! There is no discard pile. Treat the whole game — every player plus the
//! shared pile — as one closed loop: a card is always either in a hand, in
//! a personal reserve, attached to a pawn's history, or in the shared
//! pile, and it only ever moves between those, never out of the loop
//! entirely.

use rand::{Rng, RngExt};

use crate::card::CardKindId;

/// How many of each card kind a fresh personal deck starts with. Exact
/// numbers are a balance/playtesting question, not an architecture one —
/// this is just the shape that holds them.
#[derive(Clone, Debug)]
pub struct DeckComposition {
    pub counts: Vec<(CardKindId, u8)>,
}

/// A player's personal reserve. A played card's only "away from hand"
/// state is being attached to a pawn's history — it comes back here, not
/// to hand, once that history item resolves.
#[derive(Clone, Debug)]
pub struct Deck {
    reserve: Vec<CardKindId>,
}

impl Deck {
    /// Builds a fresh reserve from `composition`.
    pub fn new(composition: &DeckComposition) -> Self {
        let mut reserve = Vec::new();
        for &(card, count) in &composition.counts {
            reserve.extend(std::iter::repeat_n(card, count as usize));
        }
        Self { reserve }
    }

    /// Removes up to `count` cards at random for drawing into hand.
    /// Returns fewer than requested if the reserve is short — never an
    /// error, just however many are actually available.
    pub fn take(&mut self, count: u8, rng: &mut impl Rng) -> Vec<CardKindId> {
        take_random(&mut self.reserve, count, rng)
    }

    /// Adds a card back — from an aged-out history item, or as overflow
    /// redirected from a hand already at its cap. `bypass_cap` is set for
    /// aged-out returns specifically (`aged_out_exempt_from_deck_cap`); if
    /// the cap blocks it and `bypass_cap` is false, the card is handed back
    /// un-added so the caller can redirect it to `SharedPile`.
    pub fn give(&mut self, card: CardKindId, cap: u8, bypass_cap: bool) -> Option<CardKindId> {
        if !bypass_cap && self.reserve.len() >= cap as usize {
            return Some(card);
        }
        self.reserve.push(card);
        None
    }

    pub fn len(&self) -> usize {
        self.reserve.len()
    }

    pub fn is_empty(&self) -> bool {
        self.reserve.is_empty()
    }
}

/// The single cross-player pool — the only way cards move between players
/// other than a direct challenge outcome.
#[derive(Clone, Debug)]
pub struct SharedPile {
    cards: Vec<CardKindId>,
}

impl SharedPile {
    pub fn new(seed: Vec<CardKindId>) -> Self {
        Self { cards: seed }
    }

    /// Removes up to `count` cards at random. Returns fewer than requested
    /// if the pile is short — never an error, and never fabricates a card
    /// it doesn't hold.
    pub fn take(&mut self, count: u8, rng: &mut impl Rng) -> Vec<CardKindId> {
        take_random(&mut self.cards, count, rng)
    }

    pub fn add(&mut self, card: CardKindId) {
        self.cards.push(card);
    }

    pub fn len(&self) -> usize {
        self.cards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }
}

/// Removes up to `count` random elements from `pool`, without replacement.
/// Order among what's left behind isn't preserved.
fn take_random(pool: &mut Vec<CardKindId>, count: u8, rng: &mut impl Rng) -> Vec<CardKindId> {
    let actual = (count as usize).min(pool.len());
    let mut drawn = Vec::with_capacity(actual);
    for _ in 0..actual {
        let index = rng.random_range(0..pool.len());
        drawn.push(pool.swap_remove(index));
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composition() -> DeckComposition {
        DeckComposition {
            counts: vec![(CardKindId(0), 3), (CardKindId(1), 2)],
        }
    }

    #[test]
    fn new_flattens_composition_counts_into_the_reserve() {
        let deck = Deck::new(&composition());
        assert_eq!(deck.len(), 5);
    }

    #[test]
    fn take_never_returns_more_than_requested_or_available() {
        let mut deck = Deck::new(&composition());
        let mut rng = rand::rng();

        let drawn = deck.take(2, &mut rng);
        assert_eq!(drawn.len(), 2);
        assert_eq!(deck.len(), 3);

        let rest = deck.take(10, &mut rng);
        assert_eq!(rest.len(), 3);
        assert!(deck.is_empty());

        // Never fabricates: everything drawn came from the original counts.
        let mut all_drawn = drawn;
        all_drawn.extend(rest);
        assert_eq!(all_drawn.iter().filter(|&&c| c == CardKindId(0)).count(), 3);
        assert_eq!(all_drawn.iter().filter(|&&c| c == CardKindId(1)).count(), 2);
    }

    #[test]
    fn take_zero_returns_nothing() {
        let mut deck = Deck::new(&composition());
        let mut rng = rand::rng();
        assert!(deck.take(0, &mut rng).is_empty());
        assert_eq!(deck.len(), 5);
    }

    #[test]
    fn give_adds_under_the_cap_and_reports_success() {
        let mut deck = Deck::new(&DeckComposition { counts: vec![] });
        assert_eq!(deck.give(CardKindId(0), 5, false), None);
        assert_eq!(deck.len(), 1);
    }

    #[test]
    fn give_is_blocked_at_the_cap_without_bypass() {
        let mut deck = Deck::new(&DeckComposition {
            counts: vec![(CardKindId(0), 3)],
        });
        assert_eq!(deck.give(CardKindId(1), 3, false), Some(CardKindId(1)));
        // Blocked: the reserve is unchanged, and the un-added card is
        // handed back so the caller can redirect it elsewhere.
        assert_eq!(deck.len(), 3);
    }

    #[test]
    fn give_bypasses_the_cap_when_asked() {
        let mut deck = Deck::new(&DeckComposition {
            counts: vec![(CardKindId(0), 3)],
        });
        assert_eq!(deck.give(CardKindId(1), 3, true), None);
        assert_eq!(deck.len(), 4);
    }

    #[test]
    fn shared_pile_seeds_from_the_given_cards() {
        let pile = SharedPile::new(vec![CardKindId(0), CardKindId(1)]);
        assert_eq!(pile.len(), 2);
    }

    #[test]
    fn shared_pile_take_never_fabricates_cards() {
        let mut pile = SharedPile::new(vec![CardKindId(0), CardKindId(1)]);
        let mut rng = rand::rng();
        let drawn = pile.take(5, &mut rng);
        assert_eq!(drawn.len(), 2);
        assert!(pile.is_empty());
    }

    #[test]
    fn shared_pile_add_grows_the_pile() {
        let mut pile = SharedPile::new(vec![]);
        pile.add(CardKindId(0));
        assert_eq!(pile.len(), 1);
    }
}
