//! The complete, replayable log of everything that happened in a game —
//! the contract a future presentation layer consumes (see
//! ARCHITECTURE.md's "Scope and context").
//!
//! `AuditResolved` (ARCHITECTURE.md §13) is left out for now: its real
//! shape needs `AuditRequest`/`AuditOutcome` from `audit.rs`, which doesn't
//! exist until the build order (§16) reaches step 6. It's added then.

use crate::board::SpaceId;
use crate::card::CardKindId;
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
}
