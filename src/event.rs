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
    /// A pawn a caught lie's revert returned to the board — reinstated
    /// because it was captured during one of the now-undone moves, and
    /// (per `RuleConfig::revert_captures_on_lie`) hasn't moved under its
    /// own power since. Distinct from `PawnMoved`: this is a side effect
    /// of someone else's lie being caught, not a move this pawn made.
    PawnReinstated {
        pawn: PawnId,
        to: SpaceId,
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
    /// A player had no legal action at all (`NoAvailableActionBehavior`)
    /// and passed — distinct from `TurnForfeited`, which is a penalty from
    /// a card like `StunTrapCard`, not simply having nothing to do.
    TurnPassed {
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
    /// A card that would otherwise have gone to an eliminated `Frozen`
    /// player redirected to the pile instead, since they can no longer
    /// act to claim or use it: a false-accusation payment that would have
    /// gone to them, or a captured/removed pawn's dormant history that
    /// would otherwise wait on a yard-exit that will never happen.
    EliminatedPlayerRedirect,
}
