//! Cards are data-driven behavior, not a closed enum — a new card kind is a
//! new type implementing `CardBehavior`, registered in a `CardCatalog`, not
//! a new branch threaded through the engine.
//!
//! No concrete card kinds exist yet (see ARCHITECTURE.md §16's build
//! order); this module only lays out the shape they'll plug into.
//! `CardBehavior` is deliberately still an empty marker trait — its hooks
//! (`on_played`, `on_claimed`, ...) take `PlayContext`/`InteractionContext`/
//! `AuditContext`, which in turn need `Pawn`, `PlayerId`, and `GameEvent`
//! from modules that don't exist yet either. Those hooks get added once
//! `context/` (and, transitively, `pawn.rs`/`player.rs`/`event.rs`) land.

/// Identifies one kind of card in a `CardCatalog`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CardKindId(pub u16);

/// Which broad family of effect a card belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CardCategory {
    Movement,
    MovementModifier,
    Offense,
    Defense,
    Deception,
}

/// The behavior a card kind implements. Empty for now — see the module doc
/// for why its hooks aren't defined yet.
pub trait CardBehavior {}

/// One card kind's catalog entry: its identity, display info, and behavior.
pub struct CardMeta {
    pub id: CardKindId,
    pub display_name: &'static str,
    pub category: CardCategory,
    pub behavior: Box<dyn CardBehavior + Send + Sync>,
}

/// The registry of every card kind in a game. `CardKindId(n)` is always the
/// `n`th entry registered.
pub struct CardCatalog {
    definitions: Vec<CardMeta>,
}

impl CardCatalog {
    /// The catalog entry for `id`, or `None` if no card kind is registered
    /// under that id.
    pub fn get(&self, id: CardKindId) -> Option<&CardMeta> {
        self.definitions.get(id.0 as usize)
    }

    /// The standard catalog. Empty for now, since no concrete card kinds
    /// exist yet.
    pub fn standard() -> Self {
        Self {
            definitions: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoOpCard;
    impl CardBehavior for NoOpCard {}

    #[test]
    fn standard_catalog_starts_empty() {
        let catalog = CardCatalog::standard();
        assert!(catalog.get(CardKindId(0)).is_none());
    }

    #[test]
    fn catalog_get_finds_a_registered_card_and_nothing_else() {
        let catalog = CardCatalog {
            definitions: vec![CardMeta {
                id: CardKindId(0),
                display_name: "No-Op",
                category: CardCategory::Movement,
                behavior: Box::new(NoOpCard),
            }],
        };
        assert_eq!(
            catalog.get(CardKindId(0)).map(|m| m.display_name),
            Some("No-Op")
        );
        assert!(catalog.get(CardKindId(1)).is_none());
    }
}
