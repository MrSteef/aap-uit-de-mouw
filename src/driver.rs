//! Drives one turn at a time: asks the current player's agent for an
//! action, applies it, and decides — based on which kind of action just
//! landed — whether the turn is over.

use crate::agent::PlayerAgent;
use crate::event::GameEvent;
use crate::game::{GameEngine, GameError, TurnAction};

/// Runs exactly one turn: zero or more audits (each possibly followed by
/// a forced `ForfeitCard`), then one turn-ending action — always a
/// `PlayCard`, and also an `Audit` if `RuleConfig::auditing_costs_turn`
/// is set. `apply` itself doesn't know about "turns"; it just validates
/// and applies one action at a time. This loop is what decides when a
/// turn is over, based on which kind of action just landed.
pub fn play_one_turn(
    engine: &mut impl GameEngine,
    agents: &mut [Box<dyn PlayerAgent>],
) -> Result<Vec<GameEvent>, GameError> {
    let mut all_events = Vec::new();
    loop {
        let current = engine.current_player();
        let view = engine.view_for(current);
        let legal = engine.legal_actions(current);
        let action = agents[current.0 as usize].choose_action(&view, &legal);
        let would_end_turn = match &action {
            TurnAction::PlayCard(_) => true,
            TurnAction::Audit(_) => view.rules.auditing_costs_turn,
            TurnAction::ForfeitCard(_) => false,
        };
        all_events.extend(engine.apply(action)?);
        // A pending forfeit always takes priority over ending the turn,
        // even under `auditing_costs_turn` — `legal_actions` narrows to
        // ForfeitCard-only next iteration whenever one is still owed
        // (relevant when `false_accusation_card_cost` is more than one),
        // so checking that directly is simpler than tracking it here.
        if would_end_turn
            && !matches!(
                engine.legal_actions(current).first(),
                Some(TurnAction::ForfeitCard(_))
            )
        {
            return Ok(all_events);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{RandomAgent, ScriptedAgent};
    use crate::board::{BoardTopology, NextSpace, PlayerColor};
    use crate::card::{AuditOutcome, CardCatalog, CardKindId};
    use crate::deck::{Deck, DeckComposition};
    use crate::game::GameState;
    use crate::pawn::PawnId;
    use crate::pawn::tests::bare_pawn;
    use crate::play::{Declaration, PlayedCard};
    use crate::player::{Player, PlayerId};
    use crate::rules::{RuleConfig, minimal_rules};

    fn entry_of(topology: &BoardTopology, color: PlayerColor) -> crate::board::SpaceId {
        let yard = topology.yard_spaces(color)[0];
        match topology.next_space(yard, color).unwrap() {
            NextSpace::Single(space) => space,
            other => panic!("expected a single yard exit edge, got {other:?}"),
        }
    }

    fn steps_from(
        topology: &BoardTopology,
        color: PlayerColor,
        from: crate::board::SpaceId,
        n: u32,
    ) -> crate::board::SpaceId {
        let mut here = from;
        for _ in 0..n {
            here = match topology.next_space(here, color).unwrap() {
                NextSpace::Single(space) => space,
                other => panic!("expected a single ring step, got {other:?}"),
            };
        }
        here
    }

    fn empty_deck() -> Deck {
        Deck::new(&DeckComposition { counts: Vec::new() })
    }

    #[test]
    fn a_single_play_card_action_ends_the_turn() {
        let topology = BoardTopology::standard_ring(2, 8, 3, 2).unwrap();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let players = vec![
            Player {
                id: PlayerId(0),
                color: PlayerColor(0),
                hand: vec![CardKindId(0)],
                deck: empty_deck(),
                score: 0,
            },
            Player {
                id: PlayerId(1),
                color: PlayerColor(1),
                hand: Vec::new(),
                deck: empty_deck(),
                score: 0,
            },
        ];
        let pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), entry0)];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            crate::deck::SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        let mut agents: Vec<Box<dyn PlayerAgent>> = vec![
            Box::new(ScriptedAgent::new([TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            })])),
            Box::new(ScriptedAgent::new([])),
        ];

        let events = play_one_turn(&mut state, &mut agents).unwrap();

        assert!(
            events
                .iter()
                .any(|event| matches!(event, GameEvent::PawnMoved { .. }))
        );
        assert_eq!(state.current_player, PlayerId(1));
    }

    #[test]
    fn auditing_costs_turn_ends_the_turn_without_a_play_card() {
        let topology = BoardTopology::standard_ring(2, 8, 3, 2).unwrap();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let rules = RuleConfig {
            auditing_costs_turn: true,
            ..minimal_rules()
        };
        let players = vec![
            Player {
                id: PlayerId(0),
                color: PlayerColor(0),
                hand: vec![CardKindId(0)],
                deck: empty_deck(),
                score: 0,
            },
            Player {
                id: PlayerId(1),
                color: PlayerColor(1),
                hand: vec![CardKindId(1)],
                deck: empty_deck(),
                score: 0,
            },
        ];
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), entry0)];
        pawns[0].push_move(
            crate::pawn::MoveRecord {
                sequence: 0, // overwritten by push_move
                claimed_cards: vec![CardKindId(0)],
                actual_cards: vec![CardKindId(0)],
                position_before: entry0,
                position_after: entry0,
                captures_caused: Vec::new(),
                reveal: crate::pawn::RevealScope::Hidden,
            },
            3,
        );
        // It's player 1's turn to challenge player 0's already-recorded move.
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            pawns,
            crate::deck::SharedPile::new(Vec::new()),
            PlayerId(1),
        );

        let mut agents: Vec<Box<dyn PlayerAgent>> = vec![
            Box::new(ScriptedAgent::new([])),
            Box::new(ScriptedAgent::new([TurnAction::Audit(
                crate::audit::AuditRequest {
                    auditor: PlayerId(1),
                    target_pawn: PawnId(0),
                    target_move_index: 0,
                    attempt_cost_cards: Vec::new(),
                },
            )])),
        ];

        play_one_turn(&mut state, &mut agents).unwrap();

        assert_eq!(state.current_player, PlayerId(0));
    }

    /// The full bluff-and-audit story from ARCHITECTURE.md §14's Scenario
    /// A, driven end-to-end through `play_one_turn`: Blue claims a
    /// distance of 8 while truly playing something else and a Shield on
    /// the side; Red challenges it on their turn, catches the lie, and
    /// collects both real cards before playing their own honest move.
    #[test]
    fn golden_scenario_a_bluff_then_audit() {
        let topology = BoardTopology::standard_ring(2, 24, 3, 2).unwrap();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let entry1 = entry_of(&topology, PlayerColor(1));
        let bluffed_destination = steps_from(&topology, PlayerColor(0), entry0, 8);
        let honest_destination = steps_from(&topology, PlayerColor(1), entry1, 2);

        let players = vec![
            Player {
                id: PlayerId(0),
                color: PlayerColor(0),
                hand: vec![CardKindId(3), CardKindId(4), CardKindId(0), CardKindId(6)],
                deck: empty_deck(),
                score: 0,
            },
            Player {
                id: PlayerId(1),
                color: PlayerColor(1),
                hand: vec![CardKindId(1)],
                deck: empty_deck(),
                score: 0,
            },
        ];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), entry0),
            bare_pawn(PawnId(1), PlayerColor(1), entry1),
        ];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            crate::deck::SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        let mut agents: Vec<Box<dyn PlayerAgent>> = vec![
            Box::new(ScriptedAgent::new([TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(3), CardKindId(4)],
                },
                actual_cards: vec![CardKindId(0), CardKindId(6)],
            })])),
            Box::new(ScriptedAgent::new([
                TurnAction::Audit(crate::audit::AuditRequest {
                    auditor: PlayerId(1),
                    target_pawn: PawnId(0),
                    target_move_index: 0,
                    attempt_cost_cards: Vec::new(),
                }),
                TurnAction::PlayCard(PlayedCard {
                    declaration: Declaration {
                        pawn: PawnId(1),
                        claimed_cards: vec![CardKindId(1)],
                    },
                    actual_cards: vec![CardKindId(1)],
                }),
            ])),
        ];

        // Blue's turn: the bluffed play.
        let blue_events = play_one_turn(&mut state, &mut agents).unwrap();
        assert!(blue_events.iter().any(|event| matches!(
            event,
            GameEvent::PawnMoved {
                pawn: PawnId(0),
                ..
            }
        )));
        assert_eq!(state.pawns[0].position, bluffed_destination);
        assert_eq!(state.current_player, PlayerId(1));

        // Red's turn: audit catches the lie, then Red plays honestly.
        let red_events = play_one_turn(&mut state, &mut agents).unwrap();
        assert!(red_events.iter().any(|event| matches!(
            event,
            GameEvent::AuditResolved {
                outcome: AuditOutcome::LieCaught,
                ..
            }
        )));
        assert!(red_events.iter().any(|event| matches!(
            event,
            GameEvent::PawnMoved {
                pawn: PawnId(1),
                ..
            }
        )));

        // The lie unwound Blue's pawn all the way back to its entry point.
        assert_eq!(state.pawns[0].position, entry0);
        assert_eq!(state.pawns[0].auditable_moves().count(), 0);
        // Red collected both of Blue's real cards.
        let mut red_hand = state.players[1].hand.clone();
        red_hand.sort_by_key(|c| c.0);
        assert_eq!(red_hand, vec![CardKindId(0), CardKindId(6)]);
        // Red's own honest move landed where expected, and it's Blue's turn again.
        assert_eq!(state.pawns[1].position, honest_destination);
        assert_eq!(state.current_player, PlayerId(0));
    }

    /// ARCHITECTURE.md §15: `RandomAgent` vs. `RandomAgent` self-play, run
    /// in bulk, is a cheap fuzz test for "does the engine ever panic or
    /// reach an inconsistent state" — independent of testing any specific
    /// rule.
    #[test]
    fn random_agents_self_play_many_turns_without_panicking() {
        let topology = BoardTopology::standard_ring(2, 24, 3, 2).unwrap();
        let rules = minimal_rules();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let entry1 = entry_of(&topology, PlayerColor(1));

        let composition = DeckComposition {
            counts: vec![(CardKindId(0), 20), (CardKindId(1), 20)],
        };
        let mut rng = rand::rng();
        let mut deck0 = Deck::new(&composition);
        let hand0 = deck0.take(rules.starting_hand_size, &mut rng);
        let mut deck1 = Deck::new(&composition);
        let hand1 = deck1.take(rules.starting_hand_size, &mut rng);

        let players = vec![
            Player {
                id: PlayerId(0),
                color: PlayerColor(0),
                hand: hand0,
                deck: deck0,
                score: 0,
            },
            Player {
                id: PlayerId(1),
                color: PlayerColor(1),
                hand: hand1,
                deck: deck1,
                score: 0,
            },
        ];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), entry0),
            bare_pawn(PawnId(1), PlayerColor(1), entry1),
        ];
        let shared_pile =
            crate::deck::SharedPile::new(vec![CardKindId(0); rules.starting_pile_size as usize]);
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            pawns,
            shared_pile,
            PlayerId(0),
        );

        let mut agents: Vec<Box<dyn PlayerAgent>> =
            vec![Box::new(RandomAgent), Box::new(RandomAgent)];
        for _ in 0..30 {
            // `RuleConfig::no_available_action_behavior` (a player with a
            // nonempty hand but genuinely no walkable move and no legal
            // audit either) isn't implemented yet — see game.rs's
            // implementation-status note in ARCHITECTURE.md §13. That's a
            // real, separately-tracked gap, not what this fuzz test exists
            // to catch, so stop cleanly if it's reached rather than
            // treating `RandomAgent`'s resulting panic as a finding.
            if state.legal_actions(state.current_player()).is_empty() {
                break;
            }
            play_one_turn(&mut state, &mut agents).unwrap();
        }
    }
}
