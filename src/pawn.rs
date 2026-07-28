//! Pawn identity, position, persistent-effect bookkeeping, and the move
//! history that makes both auditing and reinstatement possible.
//!
//! A pawn's history persists until it next leaves the yard — capture only
//! clears `position` and the persistent/claimed effect lists, never
//! `history` itself. That's what makes "reinstate with history intact"
//! possible at all. No separate field remembers a pre-capture position:
//! it's already sitting in the *capturing* pawn's own `MoveRecord`
//! (`captures_caused`), and it's only ever needed while that record is
//! still within the capturing pawn's own audit window anyway. Cards tied
//! up in a captured pawn remain tied up in it until it either leaves the
//! yard or the owner manually reclaims them, forfeiting reinstatement.

use std::collections::VecDeque;

use crate::board::{BoardTopology, PlayerColor, SpaceId};
use crate::card::CardKindId;

/// Identifies a single pawn.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PawnId(pub u32);

/// A pawn's identity, owner, position, active effects, and recent move
/// history.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Pawn {
    pub id: PawnId,
    pub owner: PlayerColor,
    pub position: SpaceId,
    persistent_effects: Vec<PersistentEffectState>,
    /// Outstanding *claims* of a persistent effect, tracked separately from
    /// real ones — see the anchor-mismatch note in ARCHITECTURE.md §5.
    /// Resolved (removed) the moment `trigger_automatic_audit` tests the
    /// specific one it's about, one way or another.
    claimed_effects: Vec<ClaimedEffectState>,
    /// Capacity is bounded to `RuleConfig::audit_window` by whoever calls
    /// `push_move`, not enforced internally by this type.
    history: VecDeque<MoveRecord>,
    /// Stamped onto each `MoveRecord` as it's pushed, then never reused —
    /// unlike a `history` index, which shifts as older records age out,
    /// this stays a stable way to name "the move that attached effect X"
    /// for as long as that move remains in the window. See `next_sequence`.
    next_move_sequence: u64,
}

/// Anchors a persistent effect to a pawn (follows it wherever it goes) or a
/// space (stays behind after whichever pawn triggered it leaves).
///
/// Defined here rather than alongside `PlayContext` in `context/` (where
/// ARCHITECTURE.md §4 shows it) because `PersistentEffectState` below needs
/// it and, per §1's dependency graph, `pawn` must not depend on `context` —
/// `context` depends on `pawn`, not the reverse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectAnchor {
    Pawn(PawnId),
    Space(SpaceId),
}

/// A real, currently-active persistent effect (e.g. a played Shield).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PersistentEffectState {
    pub source_card: CardKindId,
    /// Which of the *acting* pawn's moves attached this effect — see
    /// `Pawn::next_sequence`. Lets an automatic audit test the move that
    /// actually created the effect being challenged, rather than whatever
    /// the pawn's most recent move happens to be.
    pub source_move: u64,
    pub anchor: EffectAnchor,
    pub revealed: bool,
    pub expires: Option<ExpiryCondition>,
}

/// When a persistent effect expires on its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExpiryCondition {
    AfterTurns(u8),
    OnPawnMoved,
    WithSourceHistoryItem,
}

/// An outstanding *claim* of a persistent effect, with no real effect
/// backing it (or not yet known to).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ClaimedEffectState {
    pub source_card: CardKindId,
    /// See `PersistentEffectState::source_move` — the same idea, for the
    /// claim rather than the real attachment.
    pub source_move: u64,
    pub anchor: EffectAnchor,
}

/// One resolved move: what was claimed, what really happened, and what it
/// did.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MoveRecord {
    /// Stamped by `Pawn::push_move`, which owns the counter this comes
    /// from — any value set here by the caller is overwritten, so
    /// constructing a `MoveRecord` can use a placeholder.
    pub sequence: u64,
    pub claimed_cards: Vec<CardKindId>,
    pub actual_cards: Vec<CardKindId>,
    pub position_before: SpaceId,
    pub position_after: SpaceId,
    pub captures_caused: Vec<(PawnId, SpaceId)>,
    pub reveal: RevealScope,
}

/// Only meaningful for records that *stay* in history — a move proven true
/// by a failed accusation. A caught lie's records leave history entirely
/// for the auditor's hand, so there's nothing left here to track
/// visibility for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RevealScope {
    Hidden,
    Public,
}

impl MoveRecord {
    /// Whether `claimed_cards` and `actual_cards` differ as multisets —
    /// order doesn't matter, so claiming `[Take4, Double]` but having
    /// played `[Double, Take1]` is not a lie.
    pub fn is_a_lie(&self) -> bool {
        if self.claimed_cards.len() != self.actual_cards.len() {
            return true;
        }
        let mut claimed: Vec<u16> = self.claimed_cards.iter().map(|c| c.0).collect();
        let mut actual: Vec<u16> = self.actual_cards.iter().map(|c| c.0).collect();
        claimed.sort_unstable();
        actual.sort_unstable();
        claimed != actual
    }
}

impl Pawn {
    /// The sequence number the *next* call to `push_move` will stamp onto
    /// its record — lets a card's hook (via `PlayContext`) tag an effect
    /// it's attaching with "the move that's about to be created," before
    /// that move actually exists yet.
    pub fn next_sequence(&self) -> u64 {
        self.next_move_sequence
    }

    /// Pushes `record` onto this pawn's history, stamping it with the next
    /// sequence number first (overwriting whatever `record.sequence` was
    /// set to). If that leaves more than `window` records, the oldest one
    /// ages out and is returned — its cards are the caller's responsibility
    /// to route back to the owner's reserve (ARCHITECTURE.md §10). `None`
    /// if nothing aged out.
    pub fn push_move(&mut self, mut record: MoveRecord, window: usize) -> Option<MoveRecord> {
        record.sequence = self.next_move_sequence;
        self.next_move_sequence += 1;
        self.history.push_back(record);
        if self.history.len() > window {
            self.history.pop_front()
        } else {
            None
        }
    }

    /// Every move still within this pawn's audit window, oldest first,
    /// paired with the index `revert_from` would need to undo it.
    pub fn auditable_moves(&self) -> impl Iterator<Item = (usize, &MoveRecord)> {
        self.history.iter().enumerate()
    }

    /// Marks the move at `index` as provenly honest — a failed accusation
    /// leaves it in history, now publicly known to be true. A no-op if
    /// `index` is out of range.
    pub fn mark_move_public(&mut self, index: usize) {
        if let Some(record) = self.history.get_mut(index) {
            record.reveal = RevealScope::Public;
        }
    }

    /// The real, currently-active persistent effects on this pawn.
    pub fn persistent_effects(&self) -> &[PersistentEffectState] {
        &self.persistent_effects
    }

    /// The outstanding claimed effects on this pawn.
    pub fn claimed_effects(&self) -> &[ClaimedEffectState] {
        &self.claimed_effects
    }

    /// Attaches a real persistent effect to this pawn.
    pub fn attach_persistent_effect(&mut self, effect: PersistentEffectState) {
        self.persistent_effects.push(effect);
    }

    /// Attaches an outstanding claimed effect to this pawn.
    pub fn attach_claimed_effect(&mut self, effect: ClaimedEffectState) {
        self.claimed_effects.push(effect);
    }

    /// Resolves the claimed effect attached by move `source_move`, if one
    /// is still outstanding — called once `trigger_automatic_audit` has
    /// tested it, one way or another (see the field doc on
    /// `claimed_effects`). A no-op if no claim from that move exists (it
    /// may already have been resolved, or the source move aged out of the
    /// window before ever being tested).
    pub fn resolve_claimed_effect(&mut self, source_move: u64) {
        self.claimed_effects
            .retain(|effect| effect.source_move != source_move);
    }

    /// Clears `persistent_effects` and `claimed_effects`, moves `position`
    /// to `yard_slot`. Deliberately does *not* touch `history` — those
    /// records' cards stay attached and dormant until one of the two paths
    /// below.
    pub fn capture_to(&mut self, yard_slot: SpaceId) {
        self.persistent_effects.clear();
        self.claimed_effects.clear();
        self.position = yard_slot;
    }

    /// Called when this pawn's first move *out* of the yard resolves.
    /// Every still-attached record is treated exactly like a natural
    /// age-out at this point — returns their cards for the caller to send
    /// to the owner's reserve (ARCHITECTURE.md §10).
    pub fn clear_history_on_exit(&mut self) -> Vec<CardKindId> {
        self.drain_history_cards()
    }

    /// The early-cashout alternative to waiting for `clear_history_on_exit`:
    /// the owner may collect a captured pawn's attached cards straight to
    /// hand now, at the cost of losing that pawn's reinstatement
    /// eligibility — there's nothing left to revert it to afterward.
    pub fn collect_early_forfeiting_reinstatement(&mut self) -> Vec<CardKindId> {
        self.drain_history_cards()
    }

    fn drain_history_cards(&mut self) -> Vec<CardKindId> {
        self.history
            .drain(..)
            .flat_map(|record| record.actual_cards)
            .collect()
    }

    /// Reverts this pawn to its position just before the move at `index`,
    /// discarding that move and everything after it. Returns the discarded
    /// records, oldest (the directly-audited one) first — the caller uses
    /// them to distribute cards and reinstate captures.
    ///
    /// `index >= self.history.len()` is treated as nothing to revert
    /// (returns an empty `Vec`) rather than panicking — `audit.rs` already
    /// validates the index before calling this, but `Pawn` is a public
    /// type and shouldn't trust an out-of-range index from any caller.
    pub fn revert_from(&mut self, index: usize) -> Vec<MoveRecord> {
        if index >= self.history.len() {
            return Vec::new();
        }
        let reverted: Vec<MoveRecord> = self.history.split_off(index).into();
        if let Some(first) = reverted.first() {
            self.position = first.position_before;
        }
        reverted
    }
}

/// The result of an automatic audit (triggered by a capture attempt, not a
/// deliberate challenge) catching a lie: who gets credited with triggering
/// it, whose lie it was, and what the mechanical revert produced. Recorded
/// rather than acted on immediately — routing the cards to a player or the
/// shared pile touches the wider game economy (caps, other players'
/// hands), which is `GameState`'s job (ARCHITECTURE.md §16 step 8), not
/// something a card's own hook can reach.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AutomaticAuditCatch {
    pub attacker: PawnId,
    pub defender: PawnId,
    pub reversion: Reversion,
}

/// The mechanical result of reverting a pawn to just before one of its
/// moves: what cards come loose, and which captures (if any) get
/// reinstated.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Reversion {
    pub reverted_to: Option<SpaceId>,
    /// The directly reverted move's own actual cards.
    pub directly_reverted_cards: Vec<CardKindId>,
    /// Actual cards from the newer moves swept up along with it.
    pub swept_up_cards: Vec<CardKindId>,
    pub reinstated_captures: Vec<(PawnId, SpaceId)>,
}

/// Reverts `pawns[target_index]` to just before the move at `move_index`,
/// discarding it and everything after, and — if `reinstate_captures` —
/// reinstating any pawns it captured along the way that haven't since
/// moved under their own power (checked via whether a captured pawn's
/// *current* position is still one of its own `topology.yard_spaces`).
///
/// Shared by `audit::resolve` (a deliberate audit) and
/// `context::InteractionContext::trigger_automatic_audit` (an automatic
/// check, e.g. a bluffed Shield) — both need the exact same mechanical
/// revert, just with different things happening around it (card-economy
/// routing, forfeit dispatch). Lives here rather than in `audit.rs`
/// because `context` (which needs it too) can't depend on `audit` without
/// a cycle (`card ──> context ──> audit ──> card`, per §1's dependency
/// graph) — the same fix as `EffectAnchor`/`AuditOutcome` before it.
pub fn revert(
    pawns: &mut [Pawn],
    topology: &BoardTopology,
    target_index: usize,
    move_index: usize,
    reinstate_captures: bool,
) -> Reversion {
    let reverted = pawns[target_index].revert_from(move_index);
    let mut records = reverted.into_iter();
    let Some(directly_reverted) = records.next() else {
        return Reversion::default();
    };
    let reverted_to = Some(pawns[target_index].position);
    let directly_reverted_cards = directly_reverted.actual_cards.clone();

    let mut swept_up_cards = Vec::new();
    let mut candidates: Vec<(PawnId, SpaceId)> = directly_reverted.captures_caused.clone();
    for record in records {
        swept_up_cards.extend(record.actual_cards);
        candidates.extend(record.captures_caused);
    }

    let reinstated_captures = if reinstate_captures {
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

    Reversion {
        reverted_to,
        directly_reverted_cards,
        swept_up_cards,
        reinstated_captures,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn bare_pawn(id: PawnId, owner: PlayerColor, position: SpaceId) -> Pawn {
        Pawn {
            id,
            owner,
            position,
            persistent_effects: Vec::new(),
            claimed_effects: Vec::new(),
            history: VecDeque::new(),
            next_move_sequence: 0,
        }
    }

    fn record(claimed: Vec<u16>, actual: Vec<u16>, before: u32, after: u32) -> MoveRecord {
        MoveRecord {
            sequence: 0, // overwritten by push_move
            claimed_cards: claimed.into_iter().map(CardKindId).collect(),
            actual_cards: actual.into_iter().map(CardKindId).collect(),
            position_before: SpaceId(before),
            position_after: SpaceId(after),
            captures_caused: Vec::new(),
            reveal: RevealScope::Hidden,
        }
    }

    #[test]
    fn push_move_keeps_everything_within_the_window() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0));
        assert_eq!(pawn.push_move(record(vec![1], vec![1], 0, 1), 3), None);
        assert_eq!(pawn.push_move(record(vec![2], vec![2], 1, 2), 3), None);
        assert_eq!(pawn.auditable_moves().count(), 2);
    }

    #[test]
    fn push_move_stamps_a_stable_increasing_sequence() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0));
        assert_eq!(pawn.next_sequence(), 0);
        pawn.push_move(record(vec![1], vec![1], 0, 1), 5);
        assert_eq!(pawn.next_sequence(), 1);
        pawn.push_move(record(vec![2], vec![2], 1, 2), 5);
        assert_eq!(pawn.next_sequence(), 2);
        let sequences: Vec<u64> = pawn.auditable_moves().map(|(_, r)| r.sequence).collect();
        assert_eq!(sequences, vec![0, 1]);
    }

    #[test]
    fn push_move_ages_out_the_oldest_record_past_the_window() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0));
        let first = record(vec![1], vec![1], 0, 1);
        assert_eq!(pawn.push_move(first, 1), None);
        let first_as_stored = pawn.auditable_moves().next().unwrap().1.clone();
        let aged_out = pawn.push_move(record(vec![2], vec![2], 1, 2), 1);
        assert_eq!(aged_out, Some(first_as_stored));
        assert_eq!(pawn.auditable_moves().count(), 1);
    }

    #[test]
    fn auditable_moves_reports_oldest_first_with_matching_indices() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0));
        pawn.push_move(record(vec![1], vec![1], 0, 1), 5);
        pawn.push_move(record(vec![2], vec![2], 1, 2), 5);
        let indices: Vec<usize> = pawn.auditable_moves().map(|(i, _)| i).collect();
        assert_eq!(indices, vec![0, 1]);
    }

    #[test]
    fn capture_to_clears_effects_and_moves_but_keeps_history() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(5));
        pawn.push_move(record(vec![1], vec![1], 0, 5), 5);
        pawn.persistent_effects.push(PersistentEffectState {
            source_card: CardKindId(9),
            source_move: 0,
            anchor: EffectAnchor::Pawn(PawnId(0)),
            revealed: false,
            expires: None,
        });
        pawn.claimed_effects.push(ClaimedEffectState {
            source_card: CardKindId(9),
            source_move: 0,
            anchor: EffectAnchor::Pawn(PawnId(0)),
        });

        pawn.capture_to(SpaceId(0));

        assert_eq!(pawn.position, SpaceId(0));
        assert!(pawn.persistent_effects.is_empty());
        assert!(pawn.claimed_effects.is_empty());
        assert_eq!(pawn.auditable_moves().count(), 1);
    }

    #[test]
    fn clear_history_on_exit_drains_history_and_returns_actual_cards() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0));
        pawn.push_move(record(vec![1], vec![2, 3], 0, 1), 5);
        pawn.push_move(record(vec![4], vec![5], 1, 2), 5);

        let mut cards = pawn.clear_history_on_exit();
        cards.sort_by_key(|c| c.0);
        assert_eq!(cards, vec![CardKindId(2), CardKindId(3), CardKindId(5)]);
        assert_eq!(pawn.auditable_moves().count(), 0);
    }

    #[test]
    fn collect_early_forfeiting_reinstatement_also_drains_history() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0));
        pawn.push_move(record(vec![1], vec![7], 0, 1), 5);
        assert_eq!(
            pawn.collect_early_forfeiting_reinstatement(),
            vec![CardKindId(7)]
        );
        assert_eq!(pawn.auditable_moves().count(), 0);
    }

    #[test]
    fn revert_from_discards_the_move_and_everything_after_it() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(3));
        pawn.push_move(record(vec![1], vec![1], 0, 1), 5);
        pawn.push_move(record(vec![2], vec![2], 1, 2), 5);
        pawn.push_move(record(vec![3], vec![3], 2, 3), 5);

        let reverted = pawn.revert_from(1);

        assert_eq!(reverted.len(), 2);
        assert_eq!(reverted[0].position_before, SpaceId(1));
        assert_eq!(reverted[1].position_before, SpaceId(2));
        assert_eq!(pawn.position, SpaceId(1));
        assert_eq!(pawn.auditable_moves().count(), 1);
    }

    #[test]
    fn revert_from_out_of_range_reverts_nothing() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(3));
        pawn.push_move(record(vec![1], vec![1], 0, 1), 5);
        assert_eq!(pawn.revert_from(5), Vec::new());
        assert_eq!(pawn.position, SpaceId(3));
        assert_eq!(pawn.auditable_moves().count(), 1);
    }

    #[test]
    fn is_a_lie_ignores_card_order() {
        assert!(!record(vec![1, 2], vec![2, 1], 0, 1).is_a_lie());
        assert!(record(vec![1, 2], vec![1, 3], 0, 1).is_a_lie());
        assert!(record(vec![1], vec![1, 2], 0, 1).is_a_lie());
    }

    #[test]
    fn attach_and_read_persistent_and_claimed_effects() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0));
        pawn.attach_persistent_effect(PersistentEffectState {
            source_card: CardKindId(9),
            source_move: 0,
            anchor: EffectAnchor::Pawn(PawnId(0)),
            revealed: false,
            expires: Some(ExpiryCondition::OnPawnMoved),
        });
        pawn.attach_claimed_effect(ClaimedEffectState {
            source_card: CardKindId(9),
            source_move: 0,
            anchor: EffectAnchor::Pawn(PawnId(0)),
        });

        assert_eq!(pawn.persistent_effects().len(), 1);
        assert_eq!(pawn.claimed_effects().len(), 1);
    }

    #[test]
    fn resolve_claimed_effect_only_removes_the_matching_source_move() {
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), SpaceId(0));
        pawn.attach_claimed_effect(ClaimedEffectState {
            source_card: CardKindId(9),
            source_move: 0,
            anchor: EffectAnchor::Pawn(PawnId(0)),
        });
        pawn.attach_claimed_effect(ClaimedEffectState {
            source_card: CardKindId(9),
            source_move: 1,
            anchor: EffectAnchor::Pawn(PawnId(0)),
        });

        pawn.resolve_claimed_effect(0);

        assert_eq!(pawn.claimed_effects().len(), 1);
        assert_eq!(pawn.claimed_effects()[0].source_move, 1);
    }

    #[test]
    fn revert_reinstates_captures_and_reports_split_cards() {
        let captured_yard_slot = SpaceId(2);
        let mut pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), SpaceId(7)),
            bare_pawn(PawnId(1), PlayerColor(1), captured_yard_slot),
        ];
        let topology = crate::board::BoardTopology::standard_ring(2, 8, 3, 2).unwrap();
        let mut lie = record(vec![9], vec![1], 4, 5);
        lie.captures_caused = vec![(PawnId(1), SpaceId(5))];
        pawns[0].push_move(lie, 5);
        pawns[0].push_move(record(vec![2], vec![2], 5, 6), 5);

        let reversion = revert(&mut pawns, &topology, 0, 0, true);

        assert_eq!(reversion.reverted_to, Some(SpaceId(4)));
        assert_eq!(reversion.directly_reverted_cards, vec![CardKindId(1)]);
        assert_eq!(reversion.swept_up_cards, vec![CardKindId(2)]);
        assert_eq!(reversion.reinstated_captures, vec![(PawnId(1), SpaceId(5))]);
        assert_eq!(pawns[1].position, SpaceId(5));
    }

    #[test]
    fn revert_out_of_range_reports_nothing() {
        let mut pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), SpaceId(3))];
        let topology = crate::board::BoardTopology::standard_ring(2, 8, 3, 2).unwrap();
        let reversion = revert(&mut pawns, &topology, 0, 9, true);
        assert_eq!(reversion, Reversion::default());
    }
}
