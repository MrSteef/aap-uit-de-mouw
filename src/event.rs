//! The complete, replayable log of everything that happened in a game —
//! the contract a future presentation layer consumes (see
//! ARCHITECTURE.md's "Scope and context").

use crate::board::SpaceId;
use crate::card::{AuditOutcome, CardKindId};
use crate::pawn::PawnId;
use crate::player::PlayerId;

/// One thing that happened during a game.
#[derive(Clone, Debug)]
pub enum GameEvent {
    PawnMoved {
        pawn: PawnId,
        from: SpaceId,
        to: SpaceId,
    },
    PawnCaptured {
        pawn: PawnId,
        by: PawnId,
    },
    CardConsumed {
        player: PlayerId,
    },
    PersistentEffectRevealed {
        pawn: PawnId,
        card: CardKindId,
        was_real: bool,
    },
    /// ARCHITECTURE.md §13 shows this carrying a full `AuditRequest`
    /// instead of these three fields. `audit::AuditRequest` can't be
    /// referenced here without a cycle (`event ──> audit ──> card ──>
    /// context ──> event`, since `context` already depends on `event`) —
    /// so the fields actually needed are inlined instead.
    AuditResolved {
        auditor: PlayerId,
        target_pawn: PawnId,
        target_move_index: usize,
        outcome: AuditOutcome,
    },
    /// Redacted per-viewer — see ARCHITECTURE.md §11.
    CardsTransferred {
        from: PlayerId,
        to: PlayerId,
        cards: Vec<CardKindId>,
    },
    CardsGrantedFromPile {
        player: PlayerId,
        count: usize,
    },
    CardsEnteredPile {
        cards: Vec<CardKindId>,
        source: PileSource,
    },
    TurnForfeited {
        player: PlayerId,
    },
    PlayerEliminated {
        player: PlayerId,
    },
    PlayerWon {
        player: PlayerId,
    },
}

/// Where cards entering the shared pile came from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PileSource {
    AgedOutOverflow,
    CapturedPawnFinished,
    CascadedAuditSpoils,
    AutomaticAuditSpoils,
    /// Not in ARCHITECTURE.md §13's original list: an audit-attempt-cost
    /// payment (§3's `audit_attempt_cost`) that overflowed the recipient's
    /// hand and reserve. Needed once `game.rs` (§16 step 8) actually paid
    /// this cost somewhere.
    AuditAttemptCostOverflow,
    /// As above, but for a false-accusation payment
    /// (`false_accusation_card_cost`) that overflowed.
    FalseAccusationOverflow,
    /// A card granted *from* the pile (a capture reward, or a
    /// `NoAvailableActionBehavior::DrawCard` lifeline) that immediately
    /// bounced back because the recipient's hand and reserve were both
    /// already full. Rare in practice.
    GrantBounceback,
}
