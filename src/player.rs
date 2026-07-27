//! Player identity. The full `Player` struct (hand, deck, score — see
//! ARCHITECTURE.md §12) lands when the build order (§16) reaches step 5;
//! only the id type exists so far, needed by `context::AuditContext`.

/// Identifies one player.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PlayerId(pub u32);
