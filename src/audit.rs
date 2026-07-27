//! Auditing: catching a lie, or paying for a wrong accusation.
//!
//! A `MoveRecord` is judged a lie if its `claimed_cards` and `actual_cards`
//! differ as multisets — order doesn't matter, so claiming `[Take4,
//! Double]` but having played `[Double, Take1]` is not a lie.

use thiserror::Error;

use crate::board::{BoardTopology, SpaceId};
use crate::card::{AuditOutcome, CardCatalog, CardKindId};
use crate::context::AuditContext;
use crate::pawn::{Pawn, PawnId};
use crate::player::{Player, PlayerId};
use crate::rules::RuleConfig;

/// A request to challenge one of a pawn's recent moves.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AuditRequest {
    pub auditor: PlayerId,
    pub target_pawn: PawnId,
    pub target_move_index: usize,
    /// Only populated under `PaymentSelectionMode::PayerChooses` for
    /// `audit_attempt_cost_selection` — see `RuleConfig::audit_attempt_cost`.
    /// Left empty under `RandomDraft`. Paying this cost (and, on a false
    /// accusation, `false_accusation_card_cost`) is the caller's job, not
    /// this module's — see the note on `resolve` below.
    pub attempt_cost_cards: Vec<CardKindId>,
}

/// What actually happened to the reverted pawn.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RevertOutcome {
    pub pawn: PawnId,
    pub reverted_to: SpaceId,
    /// The directly audited move's own actual cards — always destined for
    /// the auditor.
    ///
    /// ARCHITECTURE.md §9 shows a single flattened `cards_collected` field
    /// here instead. That loses exactly the distinction
    /// `RuleConfig::cascade_lie_rewards_destination` needs (whether the
    /// cascade's *swept-up* cards follow the directly-audited move to the
    /// auditor, or go to the pile instead) — with only one flat list,
    /// nothing downstream could ever honor that rule. Splitting the two
    /// preserves the information; which pile each list actually lands in
    /// is still the caller's decision (game.rs, ARCHITECTURE.md §16 step 8),
    /// same as every other card movement in this module.
    pub directly_audited_cards: Vec<CardKindId>,
    /// Actual cards from the newer moves swept up in the cascade.
    pub swept_up_cards: Vec<CardKindId>,
    pub reinstated_captures: Vec<(PawnId, SpaceId)>,
}

/// What happens to the challenger as a result of the audit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AuditConsequence {
    FalseAccusation,
    LieCaught(RevertOutcome),
}

/// The full result of resolving one audit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AuditResolution {
    pub outcome: AuditOutcome,
    pub consequence: AuditConsequence,
    /// Independent of `outcome` — driven by the *actually played* card's
    /// `on_audited_as_played` hook (see `StunTrapCard`), so it can be true
    /// even when the accusation was wrong.
    pub forfeits_auditor_turn: bool,
}

/// Ways resolving an audit can fail.
#[derive(Error, Clone, Copy, PartialEq, Eq, Debug)]
pub enum AuditError {
    #[error("no pawn exists with id {0:?}")]
    UnknownPawn(PawnId),
    #[error("pawn {pawn:?} has no move at history index {index}")]
    UnknownMoveIndex { pawn: PawnId, index: usize },
    #[error("pawn {0:?}'s owner isn't among the given players")]
    UnknownAuditee(PawnId),
}

/// Resolves `request` against `pawns`: compares the audited move's claim
/// against what was truly played, reverts the target pawn on a caught lie
/// (undoing its position and history back to just before that move, and
/// reinstating any pawns it captured along the way that haven't since moved
/// under their own power), and dispatches the actually-played and claimed
/// cards' `on_audited_as_*` hooks to determine `forfeits_auditor_turn`.
///
/// Deliberately out of scope here: paying `audit_attempt_cost` /
/// `false_accusation_card_cost`, and routing `RevertOutcome`'s cards to
/// hands/decks/the shared pile. Those touch multiple players' economies at
/// once (`GameState`'s job, ARCHITECTURE.md §16 step 8) — this function
/// only reports what happened to the audited pawn.
pub fn resolve(
    request: &AuditRequest,
    catalog: &CardCatalog,
    topology: &BoardTopology,
    rules: &RuleConfig,
    players: &[Player],
    pawns: &mut [Pawn],
) -> Result<AuditResolution, AuditError> {
    let target_index = pawns
        .iter()
        .position(|pawn| pawn.id == request.target_pawn)
        .ok_or(AuditError::UnknownPawn(request.target_pawn))?;

    let audited_record = pawns[target_index]
        .auditable_moves()
        .find(|&(index, _)| index == request.target_move_index)
        .map(|(_, record)| record.clone())
        .ok_or(AuditError::UnknownMoveIndex {
            pawn: request.target_pawn,
            index: request.target_move_index,
        })?;

    let outcome = if is_same_multiset(&audited_record.claimed_cards, &audited_record.actual_cards) {
        AuditOutcome::ClaimWasTrue
    } else {
        AuditOutcome::LieCaught
    };

    let auditee_color = pawns[target_index].owner;
    let auditee = players
        .iter()
        .find(|player| player.color == auditee_color)
        .map(|player| player.id)
        .ok_or(AuditError::UnknownAuditee(request.target_pawn))?;

    let consequence = match outcome {
        AuditOutcome::ClaimWasTrue => {
            pawns[target_index].mark_move_public(request.target_move_index);
            AuditConsequence::FalseAccusation
        }
        AuditOutcome::LieCaught => AuditConsequence::LieCaught(revert_and_reinstate(
            request,
            rules,
            topology,
            pawns,
            target_index,
        )),
    };

    let mut forfeits_auditor_turn = false;
    {
        let mut audit_ctx = AuditContext::new(
            request.auditor,
            auditee,
            request.target_pawn,
            &mut forfeits_auditor_turn,
        );
        for card_id in &audited_record.actual_cards {
            if let Some(meta) = catalog.get(*card_id) {
                meta.behavior.on_audited_as_played(outcome, &mut audit_ctx);
            }
        }
        for card_id in &audited_record.claimed_cards {
            if let Some(meta) = catalog.get(*card_id) {
                meta.behavior.on_audited_as_claimed(outcome, &mut audit_ctx);
            }
        }
    }

    Ok(AuditResolution {
        outcome,
        consequence,
        forfeits_auditor_turn,
    })
}

fn revert_and_reinstate(
    request: &AuditRequest,
    rules: &RuleConfig,
    topology: &BoardTopology,
    pawns: &mut [Pawn],
    target_index: usize,
) -> RevertOutcome {
    let reverted = pawns[target_index].revert_from(request.target_move_index);
    let reverted_to = pawns[target_index].position;

    let mut records = reverted.into_iter();
    let directly_audited = records
        .next()
        .expect("resolve() already confirmed a record exists at target_move_index");
    let directly_audited_cards = directly_audited.actual_cards.clone();

    let mut swept_up_cards = Vec::new();
    let mut candidates: Vec<(PawnId, SpaceId)> = directly_audited.captures_caused.clone();
    for record in records {
        swept_up_cards.extend(record.actual_cards);
        candidates.extend(record.captures_caused);
    }

    let reinstated_captures = if rules.revert_captures_on_lie {
        candidates
            .into_iter()
            .filter(|&(captured_id, _)| {
                pawns.iter().any(|pawn| {
                    pawn.id == captured_id
                        && topology.yard_spaces(pawn.owner).contains(&pawn.position)
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    for &(captured_id, position) in &reinstated_captures {
        if let Some(captured) = pawns.iter_mut().find(|pawn| pawn.id == captured_id) {
            captured.position = position;
        }
    }

    RevertOutcome {
        pawn: request.target_pawn,
        reverted_to,
        directly_audited_cards,
        swept_up_cards,
        reinstated_captures,
    }
}

/// Multiset equality on card identities — order doesn't matter.
fn is_same_multiset(claimed: &[CardKindId], actual: &[CardKindId]) -> bool {
    if claimed.len() != actual.len() {
        return false;
    }
    let mut claimed_sorted: Vec<u16> = claimed.iter().map(|c| c.0).collect();
    let mut actual_sorted: Vec<u16> = actual.iter().map(|c| c.0).collect();
    claimed_sorted.sort_unstable();
    actual_sorted.sort_unstable();
    claimed_sorted == actual_sorted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::PlayerColor;
    use crate::card::{CardCategory, CardMeta};
    use crate::deck::{Deck, DeckComposition};
    use crate::pawn::{MoveRecord, RevealScope, tests::bare_pawn};
    use crate::rules::minimal_rules;

    fn board() -> BoardTopology {
        BoardTopology::standard_ring(2, 8, 3, 2).unwrap()
    }

    fn players() -> Vec<Player> {
        vec![
            Player {
                id: PlayerId(0),
                color: PlayerColor(0),
                hand: Vec::new(),
                deck: Deck::new(&DeckComposition { counts: Vec::new() }),
                score: 0,
            },
            Player {
                id: PlayerId(1),
                color: PlayerColor(1),
                hand: Vec::new(),
                deck: Deck::new(&DeckComposition { counts: Vec::new() }),
                score: 0,
            },
        ]
    }

    fn record(claimed: Vec<u16>, actual: Vec<u16>, before: u32, after: u32) -> MoveRecord {
        MoveRecord {
            claimed_cards: claimed.into_iter().map(CardKindId).collect(),
            actual_cards: actual.into_iter().map(CardKindId).collect(),
            position_before: SpaceId(before),
            position_after: SpaceId(after),
            captures_caused: Vec::new(),
            reveal: RevealScope::Hidden,
        }
    }

    fn request(target_move_index: usize) -> AuditRequest {
        AuditRequest {
            auditor: PlayerId(1),
            target_pawn: PawnId(0),
            target_move_index,
            attempt_cost_cards: Vec::new(),
        }
    }

    #[test]
    fn true_claim_is_a_false_accusation_and_marks_the_move_public() {
        let topology = board();
        let catalog = CardCatalog::standard();
        let rules = minimal_rules();
        let ps = players();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(5))];
        pawns[0].push_move(record(vec![1, 2], vec![2, 1], 4, 5), 5);

        let resolution =
            resolve(&request(0), &catalog, &topology, &rules, &ps, &mut pawns).unwrap();

        assert_eq!(resolution.outcome, AuditOutcome::ClaimWasTrue);
        assert_eq!(resolution.consequence, AuditConsequence::FalseAccusation);
        assert_eq!(
            pawns[0].auditable_moves().next().unwrap().1.reveal,
            RevealScope::Public
        );
        assert_eq!(pawns[0].position, SpaceId(5));
    }

    #[test]
    fn mismatched_claim_is_a_lie_and_reverts_position_and_history() {
        let topology = board();
        let catalog = CardCatalog::standard();
        let rules = minimal_rules();
        let ps = players();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6))];
        pawns[0].push_move(record(vec![3], vec![3], 4, 5), 5);
        pawns[0].push_move(record(vec![3, 4], vec![1], 5, 6), 5);

        let resolution =
            resolve(&request(1), &catalog, &topology, &rules, &ps, &mut pawns).unwrap();

        assert_eq!(resolution.outcome, AuditOutcome::LieCaught);
        match resolution.consequence {
            AuditConsequence::LieCaught(revert) => {
                assert_eq!(revert.reverted_to, SpaceId(5));
                assert_eq!(revert.directly_audited_cards, vec![CardKindId(1)]);
                assert!(revert.swept_up_cards.is_empty());
            }
            other => panic!("expected LieCaught, got {other:?}"),
        }
        assert_eq!(pawns[0].position, SpaceId(5));
        assert_eq!(pawns[0].auditable_moves().count(), 1);
    }

    #[test]
    fn cascade_sweeps_up_newer_moves_actual_cards() {
        let topology = board();
        let catalog = CardCatalog::standard();
        let rules = minimal_rules();
        let ps = players();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(7))];
        pawns[0].push_move(record(vec![9], vec![1], 4, 5), 5);
        pawns[0].push_move(record(vec![2], vec![2], 5, 6), 5);
        pawns[0].push_move(record(vec![3], vec![3], 6, 7), 5);

        let resolution =
            resolve(&request(0), &catalog, &topology, &rules, &ps, &mut pawns).unwrap();

        match resolution.consequence {
            AuditConsequence::LieCaught(revert) => {
                assert_eq!(revert.reverted_to, SpaceId(4));
                assert_eq!(revert.directly_audited_cards, vec![CardKindId(1)]);
                let mut swept = revert.swept_up_cards.clone();
                swept.sort_by_key(|c| c.0);
                assert_eq!(swept, vec![CardKindId(2), CardKindId(3)]);
            }
            other => panic!("expected LieCaught, got {other:?}"),
        }
        assert!(pawns[0].auditable_moves().next().is_none());
    }

    #[test]
    fn reinstates_a_captured_pawn_still_parked_in_its_yard() {
        let topology = board();
        let catalog = CardCatalog::standard();
        let rules = minimal_rules();
        let ps = players();

        let captured_yard_slot = topology.yard_spaces(PlayerColor(1))[0];
        let capture_position = SpaceId(5);
        let mut pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6)),
            bare_pawn(PawnId(1), PlayerColor(1), captured_yard_slot),
        ];
        let mut lie = record(vec![9], vec![1], 5, 6);
        lie.captures_caused = vec![(PawnId(1), capture_position)];
        pawns[0].push_move(lie, 5);

        let resolution =
            resolve(&request(0), &catalog, &topology, &rules, &ps, &mut pawns).unwrap();

        match resolution.consequence {
            AuditConsequence::LieCaught(revert) => {
                assert_eq!(
                    revert.reinstated_captures,
                    vec![(PawnId(1), capture_position)]
                );
            }
            other => panic!("expected LieCaught, got {other:?}"),
        }
        assert_eq!(pawns[1].position, capture_position);
    }

    #[test]
    fn does_not_reinstate_a_pawn_that_has_moved_since_being_captured() {
        let topology = board();
        let catalog = CardCatalog::standard();
        let rules = minimal_rules();
        let ps = players();

        let capture_position = SpaceId(5);
        let moved_since_position = SpaceId(9);
        let mut pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6)),
            bare_pawn(PawnId(1), PlayerColor(1), moved_since_position),
        ];
        let mut lie = record(vec![9], vec![1], 5, 6);
        lie.captures_caused = vec![(PawnId(1), capture_position)];
        pawns[0].push_move(lie, 5);

        let resolution =
            resolve(&request(0), &catalog, &topology, &rules, &ps, &mut pawns).unwrap();

        match resolution.consequence {
            AuditConsequence::LieCaught(revert) => assert!(revert.reinstated_captures.is_empty()),
            other => panic!("expected LieCaught, got {other:?}"),
        }
        assert_eq!(pawns[1].position, moved_since_position);
    }

    #[test]
    fn revert_captures_on_lie_false_never_reinstates() {
        let topology = board();
        let catalog = CardCatalog::standard();
        let rules = crate::rules::RuleConfig {
            revert_captures_on_lie: false,
            ..minimal_rules()
        };
        let ps = players();

        let captured_yard_slot = topology.yard_spaces(PlayerColor(1))[0];
        let capture_position = SpaceId(5);
        let mut pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), SpaceId(6)),
            bare_pawn(PawnId(1), PlayerColor(1), captured_yard_slot),
        ];
        let mut lie = record(vec![9], vec![1], 5, 6);
        lie.captures_caused = vec![(PawnId(1), capture_position)];
        pawns[0].push_move(lie, 5);

        let resolution =
            resolve(&request(0), &catalog, &topology, &rules, &ps, &mut pawns).unwrap();

        match resolution.consequence {
            AuditConsequence::LieCaught(revert) => assert!(revert.reinstated_captures.is_empty()),
            other => panic!("expected LieCaught, got {other:?}"),
        }
        assert_eq!(pawns[1].position, captured_yard_slot);
    }

    struct StunTrapStub;
    impl crate::card::CardBehavior for StunTrapStub {
        fn on_audited_as_played(&self, _outcome: AuditOutcome, ctx: &mut AuditContext) {
            ctx.forfeit_auditor_turn();
        }
    }

    fn catalog_with_stun_trap() -> CardCatalog {
        CardCatalog::from_definitions(vec![CardMeta {
            id: CardKindId(0),
            display_name: "Stun Trap (test stub)",
            category: CardCategory::Deception,
            behavior: Box::new(StunTrapStub),
        }])
    }

    #[test]
    fn forfeit_hook_fires_independent_of_outcome() {
        let topology = board();
        let catalog = catalog_with_stun_trap();
        let rules = minimal_rules();
        let ps = players();

        // ClaimWasTrue case: still forfeits, since the real card played was
        // the stub "Stun Trap".
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(5))];
        pawns[0].push_move(record(vec![0], vec![0], 4, 5), 5);
        let resolution =
            resolve(&request(0), &catalog, &topology, &rules, &ps, &mut pawns).unwrap();
        assert_eq!(resolution.outcome, AuditOutcome::ClaimWasTrue);
        assert!(resolution.forfeits_auditor_turn);
    }

    #[test]
    fn forfeit_hook_does_not_fire_when_a_different_card_was_played() {
        let topology = board();
        let catalog = catalog_with_stun_trap();
        let rules = minimal_rules();
        let ps = players();

        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(5))];
        // Claimed the stub card, but a card with no registered id (99) was
        // truly played instead — catalog.get(99) is None, so no hook fires.
        pawns[0].push_move(record(vec![0], vec![99], 4, 5), 5);
        let resolution =
            resolve(&request(0), &catalog, &topology, &rules, &ps, &mut pawns).unwrap();
        assert!(!resolution.forfeits_auditor_turn);
    }

    #[test]
    fn unknown_pawn_is_an_error() {
        let topology = board();
        let catalog = CardCatalog::standard();
        let rules = minimal_rules();
        let ps = players();
        let mut pawns: Vec<Pawn> = Vec::new();
        assert_eq!(
            resolve(&request(0), &catalog, &topology, &rules, &ps, &mut pawns).unwrap_err(),
            AuditError::UnknownPawn(PawnId(0))
        );
    }

    #[test]
    fn unknown_move_index_is_an_error() {
        let topology = board();
        let catalog = CardCatalog::standard();
        let rules = minimal_rules();
        let ps = players();
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(5))];
        assert_eq!(
            resolve(&request(3), &catalog, &topology, &rules, &ps, &mut pawns).unwrap_err(),
            AuditError::UnknownMoveIndex {
                pawn: PawnId(0),
                index: 3
            }
        );
    }
}
