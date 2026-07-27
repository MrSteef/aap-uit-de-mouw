//! Pawn identity and position, plus the persistent-effect bookkeeping that
//! anchors to a pawn or a space. The full struct from ARCHITECTURE.md §8
//! (move history, captured-pawn bookkeeping) lands when the build order
//! (§16) reaches step 6; for now this only carries what `movement.rs` and
//! `context/` need.

use crate::board::{PlayerColor, SpaceId};
use crate::card::CardKindId;

/// Identifies a single pawn.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PawnId(pub u32);

/// A pawn's identity, owner, and current position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pawn {
    pub id: PawnId,
    pub owner: PlayerColor,
    pub position: SpaceId,
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
