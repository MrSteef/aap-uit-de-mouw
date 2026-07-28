//! Per-player views — the single place hidden information gets redacted,
//! so "can this player legally know X" only has to be gotten right once.
//! This is what an agent makes decisions from, never raw game state
//! directly.

use crate::board::{PlayerColor, SpaceId};
use crate::card::CardKindId;
use crate::pawn::{Pawn, PawnId, RevealScope};
use crate::player::{Player, PlayerId};
use crate::rules::RuleConfig;

/// What one player currently knows about the game.
#[derive(Clone, Debug)]
pub struct GameView {
    pub rules: RuleConfig,
    pub players: Vec<PlayerPublicInfo>,
    pub my_id: PlayerId,
    pub my_hand: Vec<CardKindId>,
    pub pawns: Vec<PawnView>,
}

/// What every player can see about another player — never their hand's
/// actual contents, just its size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayerPublicInfo {
    pub id: PlayerId,
    pub color: PlayerColor,
    pub hand_size: usize,
    pub score: i32,
}

/// One pawn as seen by a specific viewer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PawnView {
    pub id: PawnId,
    pub owner: PlayerId,
    pub position: SpaceId,
    /// One entry per real persistent effect on the pawn — `None` where the
    /// viewer isn't entitled to know which card it is.
    pub persistent_effects: Vec<Option<CardKindId>>,
    pub history: Vec<MoveRecordView>,
}

/// One history entry as seen by a specific viewer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MoveRecordView {
    pub claimed_cards: Vec<CardKindId>,
    /// `Some` only once `RevealScope::Public`, or for the viewer's own
    /// pawns (a player always knows what they truly played).
    pub actual_cards: Option<Vec<CardKindId>>,
}

/// Builds `viewer`'s view of the game: their own hand in full, every
/// player's public info, and every pawn redacted per the rules above.
pub fn build(rules: &RuleConfig, players: &[Player], pawns: &[Pawn], viewer: PlayerId) -> GameView {
    let my_hand = players
        .iter()
        .find(|player| player.id == viewer)
        .map(|player| player.hand.clone())
        .unwrap_or_default();
    let viewer_color = players
        .iter()
        .find(|player| player.id == viewer)
        .map(|player| player.color);

    let player_views = players
        .iter()
        .map(|player| PlayerPublicInfo {
            id: player.id,
            color: player.color,
            hand_size: player.hand.len(),
            score: player.score,
        })
        .collect();

    let pawn_views = pawns
        .iter()
        .map(|pawn| build_pawn_view(players, pawn, viewer_color))
        .collect();

    GameView {
        rules: rules.clone(),
        players: player_views,
        my_id: viewer,
        my_hand,
        pawns: pawn_views,
    }
}

fn build_pawn_view(players: &[Player], pawn: &Pawn, viewer_color: Option<PlayerColor>) -> PawnView {
    let owner = players
        .iter()
        .find(|player| player.color == pawn.owner)
        .map(|player| player.id)
        .expect("every pawn's owner color should belong to one of the given players");
    let is_own_pawn = viewer_color == Some(pawn.owner);

    let persistent_effects = pawn
        .persistent_effects()
        .iter()
        .map(|effect| {
            if effect.revealed || is_own_pawn {
                Some(effect.source_card)
            } else {
                None
            }
        })
        .collect();

    let history = pawn
        .auditable_moves()
        .map(|(_, record)| MoveRecordView {
            claimed_cards: record.claimed_cards.clone(),
            actual_cards: if is_own_pawn || record.reveal == RevealScope::Public {
                Some(record.actual_cards.clone())
            } else {
                None
            },
        })
        .collect();

    PawnView {
        id: pawn.id,
        owner,
        position: pawn.position,
        persistent_effects,
        history,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::CardKindId as CardId;
    use crate::deck::{Deck, DeckComposition};
    use crate::pawn::tests::bare_pawn;
    use crate::pawn::{EffectAnchor, ExpiryCondition, PersistentEffectState};
    use crate::rules::minimal_rules;

    fn empty_deck() -> Deck {
        Deck::new(&DeckComposition { counts: Vec::new() })
    }

    fn players() -> Vec<Player> {
        vec![
            Player {
                id: PlayerId(0),
                color: PlayerColor(0),
                hand: vec![CardId(1), CardId(2)],
                deck: empty_deck(),
                score: 0,
            },
            Player {
                id: PlayerId(1),
                color: PlayerColor(1),
                hand: vec![CardId(3)],
                deck: empty_deck(),
                score: 5,
            },
        ]
    }

    #[test]
    fn my_hand_is_fully_visible_but_others_only_show_a_count() {
        let rules = minimal_rules();
        let ps = players();
        let view = build(&rules, &ps, &[], PlayerId(0));

        assert_eq!(view.my_id, PlayerId(0));
        assert_eq!(view.my_hand, vec![CardId(1), CardId(2)]);
        let other = view.players.iter().find(|p| p.id == PlayerId(1)).unwrap();
        assert_eq!(other.hand_size, 1);
        assert_eq!(other.score, 5);
    }

    #[test]
    fn own_pawns_persistent_effects_are_always_visible() {
        let rules = minimal_rules();
        let ps = players();
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0));
        pawn.attach_persistent_effect(PersistentEffectState {
            source_card: CardId(9),
            source_move: 0,
            anchor: EffectAnchor::Pawn(PawnId(0)),
            revealed: false,
            expires: Some(ExpiryCondition::OnPawnMoved),
        });

        let view = build(&rules, &ps, &[pawn], PlayerId(0));

        assert_eq!(view.pawns[0].persistent_effects, vec![Some(CardId(9))]);
    }

    #[test]
    fn other_players_pawns_hide_unrevealed_effects_but_show_revealed_ones() {
        let rules = minimal_rules();
        let ps = players();
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0));
        pawn.attach_persistent_effect(PersistentEffectState {
            source_card: CardId(9),
            source_move: 0,
            anchor: EffectAnchor::Pawn(PawnId(0)),
            revealed: false,
            expires: None,
        });
        pawn.attach_persistent_effect(PersistentEffectState {
            source_card: CardId(10),
            source_move: 0,
            anchor: EffectAnchor::Pawn(PawnId(0)),
            revealed: true,
            expires: None,
        });

        let view = build(&rules, &ps, &[pawn], PlayerId(1));

        assert_eq!(
            view.pawns[0].persistent_effects,
            vec![None, Some(CardId(10))]
        );
    }

    #[test]
    fn own_move_history_always_shows_actual_cards() {
        let rules = minimal_rules();
        let ps = players();
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(1));
        pawn.push_move(
            crate::pawn::MoveRecord {
                sequence: 0, // overwritten by push_move
                claimed_cards: vec![CardId(1)],
                actual_cards: vec![CardId(2)],
                position_before: SpaceId(0),
                position_after: SpaceId(1),
                captures_caused: Vec::new(),
                reveal: RevealScope::Hidden,
            },
            5,
        );

        let view = build(&rules, &ps, &[pawn], PlayerId(0));

        assert_eq!(view.pawns[0].history[0].claimed_cards, vec![CardId(1)]);
        assert_eq!(view.pawns[0].history[0].actual_cards, Some(vec![CardId(2)]));
    }

    #[test]
    fn other_players_hidden_moves_hide_actual_cards_until_publicly_revealed() {
        let rules = minimal_rules();
        let ps = players();
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(2));
        pawn.push_move(
            crate::pawn::MoveRecord {
                sequence: 0, // overwritten by push_move
                claimed_cards: vec![CardId(1)],
                actual_cards: vec![CardId(2)],
                position_before: SpaceId(0),
                position_after: SpaceId(1),
                captures_caused: Vec::new(),
                reveal: RevealScope::Hidden,
            },
            5,
        );
        pawn.push_move(
            crate::pawn::MoveRecord {
                sequence: 0, // overwritten by push_move
                claimed_cards: vec![CardId(3)],
                actual_cards: vec![CardId(3)],
                position_before: SpaceId(1),
                position_after: SpaceId(2),
                captures_caused: Vec::new(),
                reveal: RevealScope::Public,
            },
            5,
        );

        let view = build(&rules, &ps, &[pawn], PlayerId(1));

        assert_eq!(view.pawns[0].history[0].actual_cards, None);
        assert_eq!(view.pawns[0].history[0].claimed_cards, vec![CardId(1)]);
        assert_eq!(view.pawns[0].history[1].actual_cards, Some(vec![CardId(3)]));
    }

    #[test]
    fn pawn_owner_is_resolved_to_a_player_id() {
        let rules = minimal_rules();
        let ps = players();
        let pawn = bare_pawn(PawnId(7), PlayerColor(1), SpaceId(3));

        let view = build(&rules, &ps, &[pawn], PlayerId(0));

        assert_eq!(view.pawns[0].owner, PlayerId(1));
        assert_eq!(view.pawns[0].id, PawnId(7));
        assert_eq!(view.pawns[0].position, SpaceId(3));
    }
}
