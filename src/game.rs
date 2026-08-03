//! Orchestration: the actions a player may submit, the full game state,
//! and the engine that validates and applies them.

use std::collections::{HashMap, HashSet};

use rand::RngExt;
use thiserror::Error;

use crate::audit::{self, AuditConsequence, AuditError, AuditRequest};
use crate::board::{BoardError, SpaceKind};
use crate::board::{BoardTopology, SpaceId};
use crate::card::{CardCatalog, CardCategory, CardKindId};
use crate::context::{MovementProposal, PlayContext};
use crate::deck::SharedPile;
use crate::event::{GameEvent, PileSource};
use crate::movement::MoveError;
use crate::pawn::{
    AutomaticAuditCatch, MoveRecord, Pawn, PawnId, PersistentEffectState, RevealScope,
};
use crate::play::{Declaration, PlayedCard};
use crate::player::{Player, PlayerId};
use crate::rules::{
    AutomaticAuditCardDestination, CardDestination, CardsExhaustedBehavior,
    CascadeSweepDestination, EliminatedPawnHandling, ExitRule, FinishedPawnHistoryDestination,
    NoAvailableActionBehavior, PaymentSelectionMode, RuleConfig,
};
use crate::view::{self, GameView};

/// One action a player may submit on their turn.
// `PlayCard` wraps `play::PlayedCard` (a claim paired with what was truly
// played), not just the claim alone — without the real cards reaching the
// engine somehow, bluffing (the entire point of this game) could never
// work.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TurnAction {
    Audit(AuditRequest),
    /// Only ever legal when a forfeit is pending for you.
    ForfeitCard(CardKindId),
    PlayCard(PlayedCard),
    /// Only ever legal when no other action is possible at all — see
    /// `RuleConfig::no_available_action_behavior`. Ends the turn without
    /// doing anything else.
    Pass,
}

/// Where a pending forfeit's cards are ultimately headed — resolved once,
/// when the forfeit is first set up, from whichever `CardDestination` the
/// rules specify (`Auditee` needs a concrete player id to mean anything).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PaymentTarget {
    SharedPile,
    Player(PlayerId),
}

/// Who still owes a forfeit, where it's headed, and how many cards remain.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PendingForfeit {
    owed_by: PlayerId,
    target: PaymentTarget,
    remaining: u8,
}

/// The full state of one game in progress.
pub struct GameState {
    pub topology: BoardTopology,
    pub rules: RuleConfig,
    pub catalog: CardCatalog,
    pub players: Vec<Player>,
    pub pawns: Vec<Pawn>,
    pub shared_pile: SharedPile,
    pub current_player: PlayerId,
    audits_this_turn: u8,
    forfeited_next_turn: HashSet<PlayerId>,
    pending_forfeit: Option<PendingForfeit>,
    space_effects: HashMap<SpaceId, Vec<PersistentEffectState>>,
    /// Who's out, and how their pawns behave for the rest of the game —
    /// see `RuleConfig::cards_exhausted_behavior`/
    /// `no_available_action_behavior`. An eliminated player is skipped
    /// permanently by `advance_turn`, unlike `forfeited_next_turn`'s
    /// one-turn skip.
    eliminated_players: HashMap<PlayerId, EliminatedPawnHandling>,
}

/// Ways an action can fail to apply.
#[derive(Error, Debug)]
pub enum GameError {
    #[error("no player exists with id {0:?}")]
    UnknownPlayer(PlayerId),
    #[error("no pawn exists with id {0:?}")]
    UnknownPawn(PawnId),
    #[error("pawn {pawn:?} doesn't belong to player {player:?}")]
    NotYourPawn { pawn: PawnId, player: PlayerId },
    #[error("it isn't {0:?}'s turn")]
    NotYourTurn(PlayerId),
    #[error("card {0:?} isn't in that player's hand")]
    CardNotInHand(CardKindId),
    #[error(
        "claimed {claimed} cards but actually played {actual}, and allow_card_count_mismatch is false"
    )]
    CardCountMismatch { claimed: usize, actual: usize },
    #[error("a play may include at most {max} cards, but {actual} were claimed")]
    TooManyCards { max: u8, actual: usize },
    #[error("category {category:?} allows at most {max} cards per play, but {actual} were claimed")]
    TooManyOfCategory {
        category: CardCategory,
        max: u8,
        actual: usize,
    },
    #[error("a forfeit is owed before any other action can be taken")]
    PendingForfeitOwed,
    #[error("no forfeit is currently owed")]
    NoPendingForfeit,
    #[error("at most {max} audits are allowed per turn")]
    TooManyAudits { max: u8 },
    #[error("payer must choose exactly {expected} cards, not {actual}")]
    InvalidPaymentSelection { expected: u8, actual: usize },
    #[error(transparent)]
    Audit(#[from] AuditError),
    #[error(transparent)]
    Move(#[from] MoveError),
    #[error(transparent)]
    Board(#[from] BoardError),
}

/// What a `GameState` (or any other engine implementation) exposes to
/// drive a game forward.
pub trait GameEngine {
    fn legal_actions(&self, player: PlayerId) -> Vec<TurnAction>;
    fn apply(&mut self, action: TurnAction) -> Result<Vec<GameEvent>, GameError>;
    fn view_for(&self, player: PlayerId) -> GameView;
    fn current_player(&self) -> PlayerId;
}

impl GameState {
    /// Assembles a game from already-prepared pieces — building a fresh
    /// `Deck`/hand per player, seeding the pile, and placing pawns are the
    /// caller's job; this just wires them together.
    pub fn new(
        topology: BoardTopology,
        rules: RuleConfig,
        catalog: CardCatalog,
        players: Vec<Player>,
        pawns: Vec<Pawn>,
        shared_pile: SharedPile,
        current_player: PlayerId,
    ) -> Self {
        Self {
            topology,
            rules,
            catalog,
            players,
            pawns,
            shared_pile,
            current_player,
            audits_this_turn: 0,
            forfeited_next_turn: HashSet::new(),
            pending_forfeit: None,
            space_effects: HashMap::new(),
            eliminated_players: HashMap::new(),
        }
    }

    fn player_index(&self, id: PlayerId) -> Result<usize, GameError> {
        self.players
            .iter()
            .position(|player| player.id == id)
            .ok_or(GameError::UnknownPlayer(id))
    }

    /// Resolves any pawn's owning player — named generically since it's
    /// used both for a deliberate audit's auditee and (below) an automatic
    /// audit's attacker/defender, not just auditees specifically.
    fn resolve_pawn_owner(&self, target_pawn: PawnId) -> Result<PlayerId, GameError> {
        let pawn = self
            .pawns
            .iter()
            .find(|pawn| pawn.id == target_pawn)
            .ok_or(GameError::UnknownPawn(target_pawn))?;
        self.players
            .iter()
            .find(|player| player.color == pawn.owner)
            .map(|player| player.id)
            .ok_or(GameError::Audit(AuditError::UnknownAuditee(target_pawn)))
    }

    /// Whether `player` is eliminated under `EliminatedPawnHandling::Frozen`
    /// specifically — the case where card flows that would otherwise land
    /// with them get redirected to the shared pile instead, since they can
    /// no longer act to claim or use anything (see `route_payment` and the
    /// capture-processing loop in `apply_play_card`).
    fn is_frozen(&self, player: PlayerId) -> bool {
        self.eliminated_players.get(&player) == Some(&EliminatedPawnHandling::Frozen)
    }

    /// Gate 1's trigger condition: `player`'s own hand and reserve are both
    /// completely empty, independent of whether they could otherwise still
    /// act (e.g. via a dormant collectible pawn in their yard — that's
    /// gate 2's concern, not this one).
    fn hand_and_deck_are_empty(&self, player: PlayerId) -> bool {
        let Ok(idx) = self.player_index(player) else {
            return false;
        };
        self.players[idx].hand.is_empty() && self.players[idx].deck.is_empty()
    }

    /// Gate 2's trigger condition: `player` has an empty hand *and* no
    /// dormant cards collectible from any of their own pawns sitting in
    /// the yard — i.e. genuinely no legal action available. Deliberately
    /// narrower than "`legal_actions` is empty": a free audit (when
    /// `audit_attempt_cost` is 0) can still be legal here without this
    /// being considered "having an option," per `GAME_DESIGN.md`'s
    /// wording — this gate is about movement/card-play options, not
    /// auditing specifically.
    fn has_no_legal_action(&self, player: PlayerId) -> bool {
        let Ok(idx) = self.player_index(player) else {
            return true;
        };
        if !self.players[idx].hand.is_empty() {
            return false;
        }
        let color = self.players[idx].color;
        !self.pawns.iter().any(|pawn| {
            pawn.owner == color
                && self
                    .topology
                    .node(pawn.position)
                    .is_ok_and(|node| node.kind == SpaceKind::Yard)
                && pawn.auditable_moves().count() > 0
        })
    }

    /// Marks `player` eliminated under `handling`, emitting
    /// `GameEvent::PlayerEliminated`. Under `Removed`, every one of their
    /// pawns is sent to the yard (if not there already) and has its
    /// dormant history force-drained to the shared pile immediately —
    /// normal resolution (waiting for a yard-exit, or the owner cashing in
    /// early) will never happen for a player who no longer gets turns.
    /// `Frozen` doesn't need any of that here: a frozen pawn stays exactly
    /// as interactable as before, and the redirect for *its* card flows
    /// happens at the point each one would occur (`route_payment`,
    /// `apply_play_card`'s capture handling), not eagerly here.
    fn eliminate_player(
        &mut self,
        player: PlayerId,
        handling: EliminatedPawnHandling,
        events: &mut Vec<GameEvent>,
    ) {
        if self.eliminated_players.contains_key(&player) {
            return;
        }
        self.eliminated_players.insert(player, handling);
        events.push(GameEvent::PlayerEliminated { player });

        if handling != EliminatedPawnHandling::Removed {
            return;
        }
        let Ok(idx) = self.player_index(player) else {
            return;
        };
        let color = self.players[idx].color;
        let yard_slot = self.topology.yard_spaces(color).first().copied();
        for pawn_idx in 0..self.pawns.len() {
            if self.pawns[pawn_idx].owner != color {
                continue;
            }
            if let Some(yard_slot) = yard_slot {
                self.pawns[pawn_idx].capture_to(yard_slot);
            }
            let dumped = self.pawns[pawn_idx].collect_early_forfeiting_reinstatement();
            if dumped.is_empty() {
                continue;
            }
            for &card in &dumped {
                self.shared_pile.add(card);
            }
            events.push(GameEvent::CardsEnteredPile {
                cards: dumped,
                source: PileSource::EliminatedPlayerRedirect,
            });
        }
    }

    /// Whether claiming `combo` for `pawn_id` would actually resolve —
    /// `movement::walk` can fail (e.g. landing exactly on the pawn's own
    /// home-lane fork, which has no branch-resolution mechanism yet), and
    /// a combo that can't resolve isn't a legal action to offer. Runs the
    /// real `on_claimed`/`resolve_movement` dispatch against a scratch
    /// clone of the pawns, so this stays correct for whatever cards exist
    /// rather than hardcoding movement-card knowledge here.
    fn combo_is_walkable(&self, pawn_id: PawnId, combo: &[CardKindId]) -> bool {
        let mut scratch_pawns = self.pawns.clone();
        let mut scratch_space_effects = self.space_effects.clone();
        let mut proposal = MovementProposal::default();
        let mut ctx = PlayContext::new(
            &self.topology,
            &self.rules,
            &self.catalog,
            &mut scratch_pawns,
            &mut scratch_space_effects,
            pawn_id,
        );
        for &card_id in combo {
            ctx.begin_card(card_id);
            if let Some(meta) = self.catalog.get(card_id) {
                meta.behavior.on_claimed(&mut ctx, &mut proposal);
            }
        }
        ctx.resolve_movement(proposal).is_ok()
    }

    /// Adds `card` to `player`'s hand if under `hand_hard_cap`, else their
    /// reserve (checked against `deck_cap`), else the shared pile — the
    /// standard chain for any external inflow.
    fn give_card_to_player(
        &mut self,
        player: PlayerId,
        card: CardKindId,
        overflow_source: PileSource,
        events: &mut Vec<GameEvent>,
    ) {
        let idx = self
            .player_index(player)
            .expect("caller ensures player exists");
        if self.players[idx].hand.len() < self.rules.hand_hard_cap as usize {
            self.players[idx].hand.push(card);
        } else if let Some(overflowed) =
            self.players[idx]
                .deck
                .give(card, self.rules.deck_cap, false)
        {
            self.shared_pile.add(overflowed);
            events.push(GameEvent::CardsEnteredPile {
                cards: vec![overflowed],
                source: overflow_source,
            });
        }
    }

    /// Routes the cards from a caught automatic-audit lie (e.g. an exposed
    /// bluffed Shield) to wherever `RuleConfig::automatic_audit_reward_
    /// destination` specifies. Unlike a deliberate audit, there's no
    /// `cascade_lie_rewards_destination`-style split here — nobody chose
    /// to gamble, so the directly-reverted and swept-up cards are treated
    /// as one undifferentiated pool.
    // This used to be entirely missing: the mechanical revert (position,
    // reinstated captures) always applied via `pawn::revert`'s side
    // effects regardless, but the cards it freed up had nowhere to go and
    // were simply lost until this function existed.
    fn route_automatic_audit_catch(
        &mut self,
        catch: AutomaticAuditCatch,
        events: &mut Vec<GameEvent>,
    ) -> Result<(), GameError> {
        for &(pawn, to) in &catch.reversion.reinstated_captures {
            events.push(GameEvent::PawnReinstated { pawn, to });
        }
        let mut cards = catch.reversion.directly_reverted_cards;
        cards.extend(catch.reversion.swept_up_cards);
        if cards.is_empty() {
            return Ok(());
        }
        match self.rules.automatic_audit_reward_destination {
            AutomaticAuditCardDestination::SharedPile => {
                for &card in &cards {
                    self.shared_pile.add(card);
                }
                events.push(GameEvent::CardsEnteredPile {
                    cards,
                    source: PileSource::AutomaticAuditSpoils,
                });
            }
            AutomaticAuditCardDestination::Attacker => {
                let attacker_owner = self.resolve_pawn_owner(catch.attacker)?;
                let defender_owner = self.resolve_pawn_owner(catch.defender)?;
                for &card in &cards {
                    self.give_card_to_player(
                        attacker_owner,
                        card,
                        PileSource::AutomaticAuditSpoils,
                        events,
                    );
                }
                events.push(GameEvent::CardsTransferred {
                    from: defender_owner,
                    to: attacker_owner,
                    cards,
                });
            }
        }
        Ok(())
    }

    /// Returns `card` straight to `player`'s reserve (never their hand) —
    /// for history resolving naturally, not an external windfall.
    fn return_card_to_reserve(
        &mut self,
        player: PlayerId,
        card: CardKindId,
        bypass_cap: bool,
        overflow_source: PileSource,
        events: &mut Vec<GameEvent>,
    ) {
        let idx = self
            .player_index(player)
            .expect("caller ensures player exists");
        if let Some(overflowed) = self.players[idx]
            .deck
            .give(card, self.rules.deck_cap, bypass_cap)
        {
            self.shared_pile.add(overflowed);
            events.push(GameEvent::CardsEnteredPile {
                cards: vec![overflowed],
                source: overflow_source,
            });
        }
    }

    /// Removes `count` cards from `player`'s hand per `selection`.
    /// `PayerChooses` requires exactly `count` cards named in `chosen`,
    /// each of which must genuinely be in hand. `RandomDraft` ignores
    /// `chosen` and draws uniformly at random, taking fewer if the hand is
    /// short (never an error, matching `Deck`/`SharedPile`'s own contract).
    fn take_payment_from_hand(
        &mut self,
        player: PlayerId,
        count: u8,
        selection: PaymentSelectionMode,
        chosen: &[CardKindId],
    ) -> Result<Vec<CardKindId>, GameError> {
        let idx = self.player_index(player)?;
        match selection {
            PaymentSelectionMode::PayerChooses => {
                if chosen.len() != count as usize {
                    return Err(GameError::InvalidPaymentSelection {
                        expected: count,
                        actual: chosen.len(),
                    });
                }
                let mut hand = self.players[idx].hand.clone();
                let mut taken = Vec::with_capacity(chosen.len());
                for &card in chosen {
                    let pos = hand
                        .iter()
                        .position(|&c| c == card)
                        .ok_or(GameError::CardNotInHand(card))?;
                    hand.remove(pos);
                    taken.push(card);
                }
                self.players[idx].hand = hand;
                Ok(taken)
            }
            PaymentSelectionMode::RandomDraft => {
                let mut rng = rand::rng();
                let hand = &mut self.players[idx].hand;
                let n = (count as usize).min(hand.len());
                let mut taken = Vec::with_capacity(n);
                for _ in 0..n {
                    let i = rng.random_range(0..hand.len());
                    taken.push(hand.swap_remove(i));
                }
                Ok(taken)
            }
        }
    }

    /// Sends a payment to wherever `destination` specifies, applying the
    /// usual hand → deck → pile overflow chain if it's headed to a player.
    fn route_payment(
        &mut self,
        payer: PlayerId,
        cards: Vec<CardKindId>,
        destination: CardDestination,
        auditee: PlayerId,
        overflow_source: PileSource,
        events: &mut Vec<GameEvent>,
    ) {
        if cards.is_empty() {
            return;
        }
        // A frozen auditee can no longer act to claim or use anything —
        // redirect what would otherwise land with them to the pile
        // instead, same as the SharedPile case.
        if destination == CardDestination::Auditee && self.is_frozen(auditee) {
            for &card in &cards {
                self.shared_pile.add(card);
            }
            events.push(GameEvent::CardsEnteredPile {
                cards,
                source: PileSource::EliminatedPlayerRedirect,
            });
            return;
        }
        match destination {
            CardDestination::SharedPile => {
                for &card in &cards {
                    self.shared_pile.add(card);
                }
                events.push(GameEvent::CardsEnteredPile {
                    cards,
                    source: overflow_source,
                });
            }
            CardDestination::Auditee => {
                for &card in &cards {
                    self.give_card_to_player(auditee, card, overflow_source, events);
                }
                events.push(GameEvent::CardsTransferred {
                    from: payer,
                    to: auditee,
                    cards,
                });
            }
        }
    }

    /// Draws up to `count` cards from the shared pile straight into
    /// `player`'s hand (via the usual overflow chain) — the capture reward
    /// and `NoAvailableActionBehavior::DrawCard` lifeline both work this
    /// way.
    fn grant_cards_from_pile(&mut self, player: PlayerId, count: u8, events: &mut Vec<GameEvent>) {
        if count == 0 {
            return;
        }
        let mut rng = rand::rng();
        let drawn = self.shared_pile.take(count, &mut rng);
        if drawn.is_empty() {
            return;
        }
        events.push(GameEvent::CardsGrantedFromPile {
            player,
            count: drawn.len(),
        });
        for card in drawn {
            self.give_card_to_player(player, card, PileSource::GrantBounceback, events);
        }
    }

    /// Tops `player`'s hand back up to `hand_soft_cap` from their own
    /// reserve — the only routine draw, at the end of their turn.
    fn end_of_turn_draw(&mut self, player: PlayerId) {
        let idx = self
            .player_index(player)
            .expect("caller ensures player exists");
        let hand_len = self.players[idx].hand.len();
        let target = self.rules.hand_soft_cap as usize;
        if hand_len >= target {
            return;
        }
        let need = (target - hand_len) as u8;
        let mut rng = rand::rng();
        let drawn = self.players[idx].deck.take(need, &mut rng);
        self.players[idx].hand.extend(drawn);
    }

    /// Picks the next player in turn order, skipping anyone who currently
    /// owes a forfeited turn (`StunTrapCard`, one turn only) or has been
    /// eliminated (permanent, unlike a forfeit).
    fn advance_to_next_player(&mut self) {
        let player_count = self.players.len();
        if player_count == 0 {
            return;
        }
        let current_idx = self
            .players
            .iter()
            .position(|player| player.id == self.current_player)
            .unwrap_or(0);
        let mut next_idx = (current_idx + 1) % player_count;
        for _ in 0..player_count {
            let candidate = self.players[next_idx].id;
            if self.forfeited_next_turn.remove(&candidate) {
                next_idx = (next_idx + 1) % player_count;
                continue;
            }
            if self.eliminated_players.contains_key(&candidate) {
                next_idx = (next_idx + 1) % player_count;
                continue;
            }
            break;
        }
        self.current_player = self.players[next_idx].id;
    }

    /// Advances to the next player, then resolves whatever's true for them
    /// at the start of their turn — `RuleConfig::cards_exhausted_behavior`
    /// (gate 1) and `no_available_action_behavior` (gate 2), looping past
    /// any further eliminations those gates themselves cause. Called after
    /// every turn-ending action; not called for the very first turn of a
    /// fresh game, so a ruleset configured such that the initial player
    /// already can't act isn't handled specially.
    fn advance_turn(&mut self, events: &mut Vec<GameEvent>) {
        self.advance_to_next_player();
        self.resolve_turn_start_gates(events);
    }

    fn resolve_turn_start_gates(&mut self, events: &mut Vec<GameEvent>) {
        // Bounded rather than an unconditional loop: if every player were
        // somehow eliminated, gate 1/2 re-triggering for whoever's left
        // "current" would otherwise spin forever. Determining an actual
        // game-over condition is out of scope here; this just guarantees
        // termination in that degenerate case.
        for _ in 0..=self.players.len() {
            let player = self.current_player;
            if self.hand_and_deck_are_empty(player)
                && let CardsExhaustedBehavior::Eliminated(handling) =
                    self.rules.cards_exhausted_behavior
            {
                self.eliminate_player(player, handling, events);
                self.advance_to_next_player();
                continue;
            }
            if self.has_no_legal_action(player) {
                match self.rules.no_available_action_behavior {
                    NoAvailableActionBehavior::AutoSkip => {}
                    NoAvailableActionBehavior::DrawCard(n) => {
                        self.grant_cards_from_pile(player, n, events);
                    }
                    NoAvailableActionBehavior::Eliminated(handling) => {
                        self.eliminate_player(player, handling, events);
                        self.advance_to_next_player();
                        continue;
                    }
                }
            }
            break;
        }
    }

    fn apply_play_card(&mut self, played: PlayedCard) -> Result<Vec<GameEvent>, GameError> {
        if self.pending_forfeit.is_some() {
            return Err(GameError::PendingForfeitOwed);
        }

        let claimed = played.declaration.claimed_cards.clone();
        let actual = played.actual_cards.clone();

        if !self.rules.allow_card_count_mismatch && claimed.len() != actual.len() {
            return Err(GameError::CardCountMismatch {
                claimed: claimed.len(),
                actual: actual.len(),
            });
        }
        if claimed.len() > self.rules.max_cards_per_play as usize {
            return Err(GameError::TooManyCards {
                max: self.rules.max_cards_per_play,
                actual: claimed.len(),
            });
        }
        let mut claimed_counts: HashMap<CardCategory, u8> = HashMap::new();
        for &id in &claimed {
            if let Some(meta) = self.catalog.get(id) {
                *claimed_counts.entry(meta.category).or_insert(0) += 1;
            }
        }
        for (category, &max) in &self.rules.max_cards_per_category_per_play {
            let count = claimed_counts.get(category).copied().unwrap_or(0);
            if count > max {
                return Err(GameError::TooManyOfCategory {
                    category: *category,
                    max,
                    actual: count as usize,
                });
            }
        }

        let pawn_id = played.declaration.pawn;
        let pawn_index = self
            .pawns
            .iter()
            .position(|pawn| pawn.id == pawn_id)
            .ok_or(GameError::UnknownPawn(pawn_id))?;
        let owner_color = self.pawns[pawn_index].owner;
        let pawn_owner_id = self
            .players
            .iter()
            .find(|player| player.color == owner_color)
            .map(|player| player.id)
            .ok_or(GameError::UnknownPawn(pawn_id))?;
        if pawn_owner_id != self.current_player {
            return Err(GameError::NotYourPawn {
                pawn: pawn_id,
                player: self.current_player,
            });
        }
        let acting_player = self.current_player;
        let player_idx = self.player_index(acting_player)?;

        // The actual cards must genuinely be in hand — the claim can say
        // anything, but what's truly consumed can't be fabricated.
        {
            let mut hand = self.players[player_idx].hand.clone();
            for &card in &actual {
                let pos = hand
                    .iter()
                    .position(|&c| c == card)
                    .ok_or(GameError::CardNotInHand(card))?;
                hand.remove(pos);
            }
            self.players[player_idx].hand = hand;
        }

        let position_before = self.pawns[pawn_index].position;
        let was_in_yard = self.topology.node(position_before)?.kind == SpaceKind::Yard;

        let mut events = vec![GameEvent::CardConsumed {
            player: acting_player,
        }];

        let (captures_caused, outcome) = {
            let mut proposal = MovementProposal::default();
            let mut ctx = PlayContext::new(
                &self.topology,
                &self.rules,
                &self.catalog,
                &mut self.pawns,
                &mut self.space_effects,
                pawn_id,
            );
            for &card_id in &actual {
                ctx.begin_card(card_id);
                if let Some(meta) = self.catalog.get(card_id) {
                    meta.behavior.on_played(&mut ctx);
                }
            }
            for &card_id in &claimed {
                ctx.begin_card(card_id);
                if let Some(meta) = self.catalog.get(card_id) {
                    meta.behavior.on_claimed(&mut ctx, &mut proposal);
                }
            }
            let captures = ctx.resolve_movement(proposal)?;
            (captures, ctx.into_outcome())
        };
        events.extend(outcome.events);

        for catch in outcome.automatic_audit_catches {
            self.route_automatic_audit_catch(catch, &mut events)?;
        }

        let position_after = self.pawns[pawn_index].position;

        if was_in_yard {
            let aged = self.pawns[pawn_index].clear_history_on_exit();
            for card in aged {
                self.return_card_to_reserve(
                    acting_player,
                    card,
                    self.rules.aged_out_exempt_from_deck_cap,
                    PileSource::AgedOutOverflow,
                    &mut events,
                );
            }
        }

        let record = MoveRecord {
            sequence: 0, // overwritten by push_move, right below
            claimed_cards: claimed,
            actual_cards: actual,
            position_before,
            position_after,
            captures_caused: captures_caused.clone(),
            reveal: RevealScope::Hidden,
        };
        if let Some(aged_out) =
            self.pawns[pawn_index].push_move(record, self.rules.audit_window as usize)
        {
            for card in aged_out.actual_cards {
                self.return_card_to_reserve(
                    acting_player,
                    card,
                    self.rules.aged_out_exempt_from_deck_cap,
                    PileSource::AgedOutOverflow,
                    &mut events,
                );
            }
        }

        if self.topology.node(position_after)?.kind == SpaceKind::Finish {
            // A finished pawn never moves again, so its history can never
            // naturally age out via `push_move`'s own eviction — it has to
            // be drained here, regardless of which destination the rule
            // below sends it to, or the cards would simply never resolve.
            let dumped = self.pawns[pawn_index].collect_early_forfeiting_reinstatement();
            if !dumped.is_empty() {
                match self.rules.finished_pawn_dumps_history_destination {
                    FinishedPawnHistoryDestination::SharedPile => {
                        for &card in &dumped {
                            self.shared_pile.add(card);
                        }
                        events.push(GameEvent::CardsEnteredPile {
                            cards: dumped,
                            source: PileSource::CapturedPawnFinished,
                        });
                    }
                    FinishedPawnHistoryDestination::OwnerReserve => {
                        for card in dumped {
                            self.return_card_to_reserve(
                                acting_player,
                                card,
                                self.rules.aged_out_exempt_from_deck_cap,
                                PileSource::AgedOutOverflow,
                                &mut events,
                            );
                        }
                    }
                }
            }
        }

        for &(captured_pawn, _position) in &captures_caused {
            events.push(GameEvent::PawnCaptured {
                pawn: captured_pawn,
                by: pawn_id,
            });
            self.grant_cards_from_pile(
                acting_player,
                self.rules.capture_reward_from_pile,
                &mut events,
            );
            // A frozen owner never gets another turn, so the normal "wait
            // for a yard-exit, or cash in early" resolution for a captured
            // pawn's dormant history would otherwise leave it stuck
            // forever — drain it to the pile immediately instead, same
            // reasoning as the finished-pawn dump above.
            if let Ok(owner) = self.resolve_pawn_owner(captured_pawn)
                && self.is_frozen(owner)
                && let Some(captured_idx) =
                    self.pawns.iter().position(|pawn| pawn.id == captured_pawn)
            {
                let dumped = self.pawns[captured_idx].collect_early_forfeiting_reinstatement();
                if !dumped.is_empty() {
                    for &card in &dumped {
                        self.shared_pile.add(card);
                    }
                    events.push(GameEvent::CardsEnteredPile {
                        cards: dumped,
                        source: PileSource::EliminatedPlayerRedirect,
                    });
                }
            }
        }

        self.end_of_turn_draw(acting_player);
        self.advance_turn(&mut events);

        Ok(events)
    }

    fn apply_audit(&mut self, request: AuditRequest) -> Result<Vec<GameEvent>, GameError> {
        if self.pending_forfeit.is_some() {
            return Err(GameError::PendingForfeitOwed);
        }
        if request.auditor != self.current_player {
            return Err(GameError::NotYourTurn(request.auditor));
        }
        if self.audits_this_turn >= self.rules.max_audits_per_turn {
            return Err(GameError::TooManyAudits {
                max: self.rules.max_audits_per_turn,
            });
        }

        let auditee = self.resolve_pawn_owner(request.target_pawn)?;
        let mut events = Vec::new();

        if self.rules.audit_attempt_cost > 0 {
            let paid = self.take_payment_from_hand(
                request.auditor,
                self.rules.audit_attempt_cost,
                self.rules.audit_attempt_cost_selection,
                &request.attempt_cost_cards,
            )?;
            self.route_payment(
                request.auditor,
                paid,
                self.rules.audit_attempt_cost_destination,
                auditee,
                PileSource::AuditAttemptCostOverflow,
                &mut events,
            );
        }
        self.audits_this_turn += 1;

        let resolution = audit::resolve(
            &request,
            &self.catalog,
            &self.topology,
            &self.rules,
            &self.players,
            &mut self.pawns,
        )?;

        events.push(GameEvent::AuditResolved {
            auditor: request.auditor,
            target_pawn: request.target_pawn,
            target_move_index: request.target_move_index,
            outcome: resolution.outcome,
        });

        match resolution.consequence {
            AuditConsequence::FalseAccusation => {
                if self.rules.false_accusation_card_cost > 0 {
                    match self.rules.false_accusation_selection {
                        PaymentSelectionMode::RandomDraft => {
                            let paid = self.take_payment_from_hand(
                                request.auditor,
                                self.rules.false_accusation_card_cost,
                                PaymentSelectionMode::RandomDraft,
                                &[],
                            )?;
                            self.route_payment(
                                request.auditor,
                                paid,
                                self.rules.false_accusation_destination,
                                auditee,
                                PileSource::FalseAccusationOverflow,
                                &mut events,
                            );
                        }
                        PaymentSelectionMode::PayerChooses => {
                            let target = match self.rules.false_accusation_destination {
                                CardDestination::SharedPile => PaymentTarget::SharedPile,
                                CardDestination::Auditee => PaymentTarget::Player(auditee),
                            };
                            self.pending_forfeit = Some(PendingForfeit {
                                owed_by: request.auditor,
                                target,
                                remaining: self.rules.false_accusation_card_cost,
                            });
                        }
                    }
                }
            }
            AuditConsequence::LieCaught(revert) => {
                for &(pawn, to) in &revert.reinstated_captures {
                    events.push(GameEvent::PawnReinstated { pawn, to });
                }
                if !revert.directly_audited_cards.is_empty() {
                    for &card in &revert.directly_audited_cards {
                        self.give_card_to_player(
                            request.auditor,
                            card,
                            PileSource::CascadedAuditSpoils,
                            &mut events,
                        );
                    }
                    events.push(GameEvent::CardsTransferred {
                        from: auditee,
                        to: request.auditor,
                        cards: revert.directly_audited_cards,
                    });
                }
                if !revert.swept_up_cards.is_empty() {
                    if self.rules.cascade_lie_rewards_destination
                        == CascadeSweepDestination::SharedPile
                    {
                        for &card in &revert.swept_up_cards {
                            self.shared_pile.add(card);
                        }
                        events.push(GameEvent::CardsEnteredPile {
                            cards: revert.swept_up_cards,
                            source: PileSource::CascadedAuditSpoils,
                        });
                    } else {
                        for &card in &revert.swept_up_cards {
                            self.give_card_to_player(
                                request.auditor,
                                card,
                                PileSource::CascadedAuditSpoils,
                                &mut events,
                            );
                        }
                        events.push(GameEvent::CardsTransferred {
                            from: auditee,
                            to: request.auditor,
                            cards: revert.swept_up_cards,
                        });
                    }
                }
            }
        }

        if resolution.forfeits_auditor_turn {
            self.forfeited_next_turn.insert(request.auditor);
            events.push(GameEvent::TurnForfeited {
                player: request.auditor,
            });
        }

        // `apply` doesn't otherwise know about "turns" (driver.rs decides
        // when one is over) — but when auditing itself ends the turn
        // (`auditing_costs_turn`), or a `StunTrapCard` cuts it short,
        // *something* has to actually advance `current_player`, the same
        // way `apply_play_card` always does. Skipped while a forfeit from
        // *this* audit is still pending — that has to clear first.
        if (self.rules.auditing_costs_turn || resolution.forfeits_auditor_turn)
            && self.pending_forfeit.is_none()
        {
            self.end_of_turn_draw(request.auditor);
            self.advance_turn(&mut events);
        }

        Ok(events)
    }

    fn apply_forfeit_card(&mut self, card: CardKindId) -> Result<Vec<GameEvent>, GameError> {
        let Some(pending) = self.pending_forfeit else {
            return Err(GameError::NoPendingForfeit);
        };
        if pending.owed_by != self.current_player {
            return Err(GameError::NotYourTurn(pending.owed_by));
        }
        let idx = self.player_index(pending.owed_by)?;
        let pos = self.players[idx]
            .hand
            .iter()
            .position(|&c| c == card)
            .ok_or(GameError::CardNotInHand(card))?;
        self.players[idx].hand.remove(pos);

        let mut events = Vec::new();
        match pending.target {
            PaymentTarget::SharedPile => {
                self.shared_pile.add(card);
                events.push(GameEvent::CardsEnteredPile {
                    cards: vec![card],
                    source: PileSource::FalseAccusationOverflow,
                });
            }
            PaymentTarget::Player(to) => {
                self.give_card_to_player(
                    to,
                    card,
                    PileSource::FalseAccusationOverflow,
                    &mut events,
                );
                events.push(GameEvent::CardsTransferred {
                    from: pending.owed_by,
                    to,
                    cards: vec![card],
                });
            }
        }

        let remaining = pending.remaining - 1;
        self.pending_forfeit = if remaining == 0 {
            None
        } else {
            Some(PendingForfeit {
                remaining,
                ..pending
            })
        };

        Ok(events)
    }

    /// The only legal action when nothing else is (see the `Pass` variant
    /// doc comment and `legal_actions`'s fallback) — ends the turn without
    /// doing anything else, still subject to the normal end-of-turn draw.
    fn apply_pass(&mut self) -> Result<Vec<GameEvent>, GameError> {
        if self.pending_forfeit.is_some() {
            return Err(GameError::PendingForfeitOwed);
        }
        let player = self.current_player;
        let mut events = vec![GameEvent::TurnPassed { player }];
        self.end_of_turn_draw(player);
        self.advance_turn(&mut events);
        Ok(events)
    }
}

impl GameEngine for GameState {
    fn legal_actions(&self, player: PlayerId) -> Vec<TurnAction> {
        if let Some(pending) = self.pending_forfeit {
            if pending.owed_by != player {
                return Vec::new();
            }
            let Ok(idx) = self.player_index(player) else {
                return Vec::new();
            };
            return self.players[idx]
                .hand
                .iter()
                .copied()
                .map(TurnAction::ForfeitCard)
                .collect();
        }
        if player != self.current_player {
            return Vec::new();
        }
        let Ok(idx) = self.player_index(player) else {
            return Vec::new();
        };
        let hand = self.players[idx].hand.clone();
        let color = self.players[idx].color;

        let mut actions = Vec::new();

        let combos = honest_combos(&hand, &self.catalog, &self.rules);
        for pawn in self.pawns.iter().filter(|pawn| pawn.owner == color) {
            let Ok(node) = self.topology.node(pawn.position) else {
                continue;
            };
            if node.kind == SpaceKind::Finish {
                continue;
            }
            let is_in_yard = node.kind == SpaceKind::Yard;
            for combo in &combos {
                if is_in_yard
                    && let ExitRule::RequiresCard(required) = self.rules.exit_rule
                    && !combo.contains(&required)
                {
                    continue;
                }
                if !self.combo_is_walkable(pawn.id, combo) {
                    continue;
                }
                actions.push(TurnAction::PlayCard(PlayedCard {
                    declaration: Declaration {
                        pawn: pawn.id,
                        claimed_cards: combo.clone(),
                    },
                    actual_cards: combo.clone(),
                }));
            }
        }

        if self.audits_this_turn < self.rules.max_audits_per_turn {
            let worst_case = self.rules.audit_attempt_cost as usize
                + self.rules.false_accusation_card_cost as usize;
            if hand.len() >= worst_case {
                for pawn in self.pawns.iter().filter(|pawn| pawn.owner != color) {
                    if !self.rules.captured_pawns_remain_auditable
                        && let Ok(node) = self.topology.node(pawn.position)
                        && node.kind == SpaceKind::Yard
                    {
                        continue;
                    }
                    for (index, _) in pawn.auditable_moves() {
                        let attempt_cost_cards = match self.rules.audit_attempt_cost_selection {
                            PaymentSelectionMode::PayerChooses
                                if self.rules.audit_attempt_cost > 0 =>
                            {
                                hand.iter()
                                    .take(self.rules.audit_attempt_cost as usize)
                                    .copied()
                                    .collect()
                            }
                            _ => Vec::new(),
                        };
                        actions.push(TurnAction::Audit(AuditRequest {
                            auditor: player,
                            target_pawn: pawn.id,
                            target_move_index: index,
                            attempt_cost_cards,
                        }));
                    }
                }
            }
        }

        // Never leave an agent with nothing to choose from — whether
        // that's because `no_available_action_behavior` is `AutoSkip`,
        // or because `DrawCard`'s lifeline still wasn't enough (the pile
        // was short), the result looks the same here: no other action
        // exists, so passing is the only legal one.
        if actions.is_empty() {
            actions.push(TurnAction::Pass);
        }

        actions
    }

    fn apply(&mut self, action: TurnAction) -> Result<Vec<GameEvent>, GameError> {
        match action {
            TurnAction::Audit(request) => self.apply_audit(request),
            TurnAction::ForfeitCard(card) => self.apply_forfeit_card(card),
            TurnAction::PlayCard(played) => self.apply_play_card(played),
            TurnAction::Pass => self.apply_pass(),
        }
    }

    fn view_for(&self, player: PlayerId) -> GameView {
        view::build(&self.rules, &self.players, &self.pawns, player)
    }

    fn current_player(&self) -> PlayerId {
        self.current_player
    }
}

/// Every distinct "honest" (claimed == actual) card combination in `hand`,
/// from size 1 up to `rules.max_cards_per_play`, respecting
/// `rules.max_cards_per_category_per_play`. Duplicate-looking combinations
/// (same multiset of ids, different underlying hand slots) are deduplicated.
fn honest_combos(
    hand: &[CardKindId],
    catalog: &CardCatalog,
    rules: &RuleConfig,
) -> Vec<Vec<CardKindId>> {
    let max_k = (rules.max_cards_per_play as usize).min(hand.len());
    let mut seen: HashSet<Vec<u16>> = HashSet::new();
    let mut combos = Vec::new();
    for k in 1..=max_k {
        for indices in index_combinations(hand.len(), k) {
            let combo: Vec<CardKindId> = indices.iter().map(|&i| hand[i]).collect();
            let mut counts: HashMap<CardCategory, u8> = HashMap::new();
            for &id in &combo {
                if let Some(meta) = catalog.get(id) {
                    *counts.entry(meta.category).or_insert(0) += 1;
                }
            }
            let within_limits = counts.iter().all(|(category, count)| {
                rules
                    .max_cards_per_category_per_play
                    .get(category)
                    .is_none_or(|&max| *count <= max)
            });
            if !within_limits {
                continue;
            }
            let mut key: Vec<u16> = combo.iter().map(|c| c.0).collect();
            key.sort_unstable();
            if seen.insert(key) {
                combos.push(combo);
            }
        }
    }
    combos
}

/// Every `k`-combination of indices `0..n`, in lexicographic order.
fn index_combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    if k == 0 || k > n {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut current = Vec::with_capacity(k);
    fn helper(
        start: usize,
        n: usize,
        k: usize,
        current: &mut Vec<usize>,
        out: &mut Vec<Vec<usize>>,
    ) {
        if current.len() == k {
            out.push(current.clone());
            return;
        }
        for i in start..n {
            current.push(i);
            helper(i + 1, n, k, current, out);
            current.pop();
        }
    }
    helper(0, n, k, &mut current, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{NextSpace, PlayerColor};
    use crate::card::AuditOutcome;
    use crate::deck::{Deck, DeckComposition};
    use crate::pawn::tests::bare_pawn;
    use crate::rules::minimal_rules;

    fn board() -> BoardTopology {
        BoardTopology::standard_ring(2, 8, 3, 2).unwrap()
    }

    /// A longer ring, for tests walking far enough that an 8-space ring's
    /// fork would otherwise get in the way.
    fn large_board() -> BoardTopology {
        BoardTopology::standard_ring(2, 24, 3, 2).unwrap()
    }

    fn empty_deck() -> Deck {
        Deck::new(&DeckComposition { counts: Vec::new() })
    }

    fn player(id: u32, color: u8, hand: Vec<CardKindId>) -> Player {
        Player {
            id: PlayerId(id),
            color: PlayerColor(color),
            hand,
            deck: empty_deck(),
            score: 0,
        }
    }

    fn entry_of(topology: &BoardTopology, color: PlayerColor) -> SpaceId {
        let yard = topology.yard_spaces(color)[0];
        match topology.next_space(yard, color).unwrap() {
            NextSpace::Single(space) => space,
            other => panic!("expected a single yard exit edge, got {other:?}"),
        }
    }

    fn steps_from(topology: &BoardTopology, color: PlayerColor, from: SpaceId, n: u32) -> SpaceId {
        let mut here = from;
        for _ in 0..n {
            here = match topology.next_space(here, color).unwrap() {
                NextSpace::Single(space) => space,
                other => panic!("expected a single ring step, got {other:?}"),
            };
        }
        here
    }

    /// The home-lane space immediately before Finish, for `board()`'s
    /// 8-ring/3-lane layout: 7 ring steps from entry reaches the fork, then
    /// the non-ring branch option enters the 3-space lane, then 2 more
    /// steps reaches its last space.
    fn last_home_lane_space(topology: &BoardTopology, color: PlayerColor) -> SpaceId {
        let entry = entry_of(topology, color);
        let fork = steps_from(topology, color, entry, 7);
        let lane_entry = match topology.next_space(fork, color).unwrap() {
            NextSpace::Branch(options) => *options.iter().find(|&&s| s != entry).unwrap(),
            other => panic!("expected a branch at the home-lane fork, got {other:?}"),
        };
        steps_from(topology, color, lane_entry, 2)
    }

    #[test]
    fn playing_a_card_moves_the_pawn_consumes_it_and_advances_the_turn() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let expected = steps_from(&topology, PlayerColor(0), entry0, 1);
        let players = vec![
            player(0, 0, vec![CardKindId(0), CardKindId(4)]),
            player(1, 1, vec![CardKindId(1)]),
        ];
        let pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), entry0)];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        let events = state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            GameEvent::PawnMoved {
                pawn: PawnId(0),
                ..
            }
        )));
        assert_eq!(state.pawns[0].position, expected);
        assert_eq!(state.players[0].hand, vec![CardKindId(4)]);
        assert_eq!(state.current_player, PlayerId(1));
    }

    #[test]
    fn bluffed_claim_moves_the_pawn_by_the_claimed_distance_not_the_real_one() {
        let topology = large_board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let expected = steps_from(&topology, PlayerColor(0), entry0, 8);
        let players = vec![
            player(
                0,
                0,
                vec![CardKindId(3), CardKindId(4), CardKindId(0), CardKindId(6)],
            ),
            player(1, 1, Vec::new()),
        ];
        let pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), entry0)];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(3), CardKindId(4)],
                },
                actual_cards: vec![CardKindId(0), CardKindId(6)],
            }))
            .unwrap();

        assert_eq!(state.pawns[0].position, expected);
        assert_eq!(state.pawns[0].auditable_moves().count(), 1);
        let (_, record) = state.pawns[0].auditable_moves().next().unwrap();
        assert_eq!(record.claimed_cards, vec![CardKindId(3), CardKindId(4)]);
        assert_eq!(record.actual_cards, vec![CardKindId(0), CardKindId(6)]);
        // Only the truly-played cards left the hand.
        let mut hand = state.players[0].hand.clone();
        hand.sort_by_key(|c| c.0);
        assert_eq!(hand, vec![CardKindId(3), CardKindId(4)]);
    }

    #[test]
    fn auditing_a_caught_lie_reverts_the_pawn_and_rewards_the_auditor() {
        let topology = large_board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let players = vec![
            player(
                0,
                0,
                vec![CardKindId(3), CardKindId(4), CardKindId(0), CardKindId(6)],
            ),
            player(1, 1, Vec::new()),
        ];
        let pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), entry0)];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(3), CardKindId(4)],
                },
                actual_cards: vec![CardKindId(0), CardKindId(6)],
            }))
            .unwrap();

        let events = state
            .apply(TurnAction::Audit(AuditRequest {
                auditor: PlayerId(1),
                target_pawn: PawnId(0),
                target_move_index: 0,
                attempt_cost_cards: Vec::new(),
            }))
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            GameEvent::AuditResolved {
                outcome: AuditOutcome::LieCaught,
                ..
            }
        )));
        assert_eq!(state.pawns[0].position, entry0);
        assert_eq!(state.pawns[0].auditable_moves().count(), 0);
        let mut hand = state.players[1].hand.clone();
        hand.sort_by_key(|c| c.0);
        assert_eq!(hand, vec![CardKindId(0), CardKindId(6)]);
    }

    #[test]
    fn auditing_a_caught_lie_that_captured_a_pawn_reinstates_it_and_logs_it() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let victim_at = steps_from(&topology, PlayerColor(0), entry0, 2);
        let players = vec![
            player(0, 0, vec![CardKindId(0)]), // actually plays Take 1
            player(1, 1, vec![CardKindId(0)]), // needs a card to afford auditing
        ];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), entry0),
            bare_pawn(PawnId(1), PlayerColor(1), victim_at),
        ];
        let mut state = GameState::new(
            topology,
            minimal_rules(), // revert_captures_on_lie: true
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        // Claims Take 2 (landing on and capturing pawn 1), actually plays
        // Take 1 -- a lie in identity, matching claimed/actual counts.
        state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(1)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();
        assert_eq!(
            state.pawns[1].position,
            state.topology.yard_spaces(PlayerColor(1))[0],
            "the claimed Take 2 lands on and captures pawn 1"
        );

        let events = state
            .apply(TurnAction::Audit(AuditRequest {
                auditor: PlayerId(1),
                target_pawn: PawnId(0),
                target_move_index: 0,
                attempt_cost_cards: Vec::new(),
            }))
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            GameEvent::PawnReinstated { pawn: PawnId(1), to } if *to == victim_at
        )));
        assert_eq!(
            state.pawns[1].position, victim_at,
            "the position change itself already worked before this fix -- \
             only the event was missing"
        );
    }

    #[test]
    fn automatic_audit_reinstates_a_pawn_captured_during_the_reverted_move() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let victim_at = steps_from(&topology, PlayerColor(0), entry0, 2);
        let attacker_at = steps_from(&topology, PlayerColor(0), entry0, 1);
        let players = vec![
            player(0, 0, vec![CardKindId(6)]), // actually plays Shield
            player(1, 1, vec![CardKindId(0)]), // attacker's Take 1
        ];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), entry0), // A, the bluffer
            bare_pawn(PawnId(1), PlayerColor(1), victim_at), // C, captured by A's claim
            bare_pawn(PawnId(2), PlayerColor(1), attacker_at), // B, the attacker
        ];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        // A claims Take 2 (landing on and capturing C), but actually plays
        // Shield -- a lie about identity, with a real persistent Shield
        // attached as a side effect of what was truly played.
        state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(1)],
                },
                actual_cards: vec![CardKindId(6)],
            }))
            .unwrap();
        assert_eq!(state.pawns[0].position, victim_at);
        assert_eq!(
            state.pawns[1].position,
            state.topology.yard_spaces(PlayerColor(1))[0],
            "C gets captured by A's claimed move"
        );

        // B attempts to capture A. The real Shield is tested automatically
        // (it's A's own move being challenged, not B's claim), found not to
        // match what A claimed, and A's move reverts -- which should also
        // reinstate C. Whether B's own capture attempt succeeds or is
        // blocked by the (real) Shield isn't this test's concern.
        let events = state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(2),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            GameEvent::PawnReinstated { pawn: PawnId(1), to } if *to == victim_at
        )));
        assert_eq!(state.pawns[1].position, victim_at);
    }

    #[test]
    fn false_accusation_with_random_draft_pays_immediately_and_leaves_no_pending_forfeit() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let players = vec![
            player(0, 0, vec![CardKindId(0)]),
            player(1, 1, vec![CardKindId(1), CardKindId(2)]),
        ];
        let pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), entry0)];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();

        let events = state
            .apply(TurnAction::Audit(AuditRequest {
                auditor: PlayerId(1),
                target_pawn: PawnId(0),
                target_move_index: 0,
                attempt_cost_cards: Vec::new(),
            }))
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            GameEvent::AuditResolved {
                outcome: AuditOutcome::ClaimWasTrue,
                ..
            }
        )));
        assert_eq!(state.players[1].hand.len(), 1);
        assert!(state.pending_forfeit.is_none());
    }

    #[test]
    fn false_accusation_with_payer_chooses_creates_a_pending_forfeit_that_clears_on_submission() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let rules = RuleConfig {
            false_accusation_selection: PaymentSelectionMode::PayerChooses,
            ..minimal_rules()
        };
        let players = vec![
            player(0, 0, vec![CardKindId(0)]),
            player(1, 1, vec![CardKindId(1), CardKindId(2)]),
        ];
        let pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), entry0)];
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();
        state
            .apply(TurnAction::Audit(AuditRequest {
                auditor: PlayerId(1),
                target_pawn: PawnId(0),
                target_move_index: 0,
                attempt_cost_cards: Vec::new(),
            }))
            .unwrap();

        assert!(state.pending_forfeit.is_some());
        let legal = state.legal_actions(PlayerId(1));
        assert_eq!(legal.len(), 2);
        assert!(
            legal
                .iter()
                .all(|action| matches!(action, TurnAction::ForfeitCard(_)))
        );

        let to_forfeit = state.players[1].hand[0];
        state.apply(TurnAction::ForfeitCard(to_forfeit)).unwrap();

        assert!(state.pending_forfeit.is_none());
        assert!(state.players[0].hand.contains(&to_forfeit));
        assert!(!state.players[1].hand.contains(&to_forfeit));
    }

    #[test]
    fn pending_forfeit_blocks_every_other_action() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let rules = RuleConfig {
            false_accusation_selection: PaymentSelectionMode::PayerChooses,
            ..minimal_rules()
        };
        let players = vec![
            player(0, 0, vec![CardKindId(0)]),
            player(1, 1, vec![CardKindId(1)]),
        ];
        let pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), entry0)];
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();
        state
            .apply(TurnAction::Audit(AuditRequest {
                auditor: PlayerId(1),
                target_pawn: PawnId(0),
                target_move_index: 0,
                attempt_cost_cards: Vec::new(),
            }))
            .unwrap();

        let err = state
            .apply(TurnAction::Audit(AuditRequest {
                auditor: PlayerId(1),
                target_pawn: PawnId(0),
                target_move_index: 0,
                attempt_cost_cards: Vec::new(),
            }))
            .unwrap_err();
        assert!(matches!(err, GameError::PendingForfeitOwed));
    }

    #[test]
    fn capturing_a_pawn_sends_it_to_yard_and_grants_a_reward() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let landing = steps_from(&topology, PlayerColor(0), entry0, 1);
        let players = vec![player(0, 0, vec![CardKindId(0)]), player(1, 1, Vec::new())];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), entry0),
            bare_pawn(PawnId(1), PlayerColor(1), landing),
        ];
        let shared_pile = SharedPile::new(vec![CardKindId(9); 5]);
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            shared_pile,
            PlayerId(0),
        );

        let events = state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            GameEvent::PawnCaptured {
                pawn: PawnId(1),
                by: PawnId(0)
            }
        )));
        assert_eq!(
            state.pawns[1].position,
            state.topology.yard_spaces(PlayerColor(1))[0]
        );
        assert_eq!(state.players[0].hand.len(), 2);
    }

    #[test]
    fn bluffed_shield_caught_by_a_capture_sends_its_cards_to_the_pile_when_configured() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let bluffer_at = steps_from(&topology, PlayerColor(0), entry0, 2);
        let attacker_at = steps_from(&topology, PlayerColor(0), entry0, 1);
        let players = vec![
            player(0, 0, vec![CardKindId(0)]),
            player(1, 1, vec![CardKindId(0)]),
        ];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), bluffer_at),
            bare_pawn(PawnId(1), PlayerColor(1), attacker_at),
        ];
        let rules = RuleConfig {
            automatic_audit_reward_destination: AutomaticAuditCardDestination::SharedPile,
            // Isolate this test from the unrelated capture-reward mechanic,
            // which would otherwise also draw from the pile this same
            // turn and make the final pile contents harder to reason about.
            capture_reward_from_pile: 0,
            ..minimal_rules()
        };
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        // Pawn 0 claims Shield (id 6) but actually plays Take 1 (id 0) — a
        // lie. Shield's `on_claimed` contributes no steps, so pawn 0 stays
        // put; only the claim (not a real Shield) gets attached.
        state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(6)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();
        assert_eq!(state.pawns[0].position, bluffer_at);

        // Pawn 1 lands exactly on pawn 0, attempting a capture. The claimed
        // Shield gets automatically tested, found false, and pawn 0's
        // bluffing move reverts — its actual card (id 0) has to go
        // somewhere rather than vanish.
        let events = state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(1),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            GameEvent::CardsEnteredPile { cards, source: PileSource::AutomaticAuditSpoils }
                if cards == &vec![CardKindId(0)]
        )));
        assert_eq!(
            state.shared_pile.take(10, &mut rand::rng()),
            vec![CardKindId(0)]
        );
        assert_eq!(state.pawns[0].auditable_moves().count(), 0);
        // The claim was false, so the "shield" never actually blocked
        // anything — pawn 0 also gets captured in the ordinary way.
        assert_eq!(
            state.pawns[0].position,
            state.topology.yard_spaces(PlayerColor(0))[0]
        );
    }

    #[test]
    fn bluffed_shield_caught_by_a_capture_rewards_the_attacker_when_configured() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let bluffer_at = steps_from(&topology, PlayerColor(0), entry0, 2);
        let attacker_at = steps_from(&topology, PlayerColor(0), entry0, 1);
        let players = vec![
            player(0, 0, vec![CardKindId(0)]),
            player(1, 1, vec![CardKindId(0)]),
        ];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), bluffer_at),
            bare_pawn(PawnId(1), PlayerColor(1), attacker_at),
        ];
        let rules = RuleConfig {
            automatic_audit_reward_destination: AutomaticAuditCardDestination::Attacker,
            ..minimal_rules()
        };
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(6)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();

        let events = state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(1),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            GameEvent::CardsTransferred { from: PlayerId(0), to: PlayerId(1), cards }
                if cards == &vec![CardKindId(0)]
        )));
        assert!(state.shared_pile.is_empty());
        // Player 1 played their only hand card to capture, ending their
        // turn empty-handed, then received the bluffer's real card, then
        // (still within the same turn) drew back up to hand_soft_cap from
        // their own empty deck — so the routed card is what remains.
        assert_eq!(state.players[1].hand, vec![CardKindId(0)]);
    }

    #[test]
    fn cards_exhausted_ignored_falls_through_to_gate_two() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let players = vec![player(0, 0, Vec::new()), player(1, 1, vec![CardKindId(0)])];
        let pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), entry0)];
        let mut state = GameState::new(
            topology,
            minimal_rules(), // cards_exhausted_behavior: Ignored, no_available_action_behavior: AutoSkip
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        let mut events = Vec::new();
        state.resolve_turn_start_gates(&mut events);

        assert!(events.is_empty());
        assert!(state.eliminated_players.is_empty());
        assert_eq!(state.legal_actions(PlayerId(0)), vec![TurnAction::Pass]);
    }

    #[test]
    fn cards_exhausted_eliminates_even_with_a_yard_collectible_pawn() {
        let topology = board();
        let yard0 = topology.yard_spaces(PlayerColor(0))[0];
        let players = vec![player(0, 0, Vec::new()), player(1, 1, vec![CardKindId(0)])];
        let mut dormant_pawn = bare_pawn(PawnId(0), PlayerColor(0), yard0);
        dormant_pawn.push_move(
            MoveRecord {
                sequence: 0,
                claimed_cards: vec![CardKindId(1)],
                actual_cards: vec![CardKindId(1)],
                position_before: yard0,
                position_after: yard0,
                captures_caused: Vec::new(),
                reveal: RevealScope::Hidden,
            },
            3,
        );
        let rules = RuleConfig {
            cards_exhausted_behavior: CardsExhaustedBehavior::Eliminated(
                EliminatedPawnHandling::Frozen,
            ),
            ..minimal_rules()
        };
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            vec![dormant_pawn],
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        // Gate 2 alone would NOT trigger here — there IS a yard-collectible
        // pawn — but gate 1 (empty hand + deck) takes priority regardless.
        assert!(!state.has_no_legal_action(PlayerId(0)));

        let mut events = Vec::new();
        state.resolve_turn_start_gates(&mut events);

        assert!(events.iter().any(|e| matches!(
            e,
            GameEvent::PlayerEliminated {
                player: PlayerId(0)
            }
        )));
        assert!(state.is_frozen(PlayerId(0)));
        assert_eq!(state.current_player, PlayerId(1));
    }

    #[test]
    fn eliminated_players_are_skipped_permanently_by_advance_to_next_player() {
        let topology = board();
        let players = vec![
            player(0, 0, vec![CardKindId(0)]),
            player(1, 1, vec![CardKindId(0)]),
            player(2, 0, vec![CardKindId(0)]),
        ];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            Vec::new(),
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );
        state
            .eliminated_players
            .insert(PlayerId(1), EliminatedPawnHandling::Frozen);

        state.advance_to_next_player();

        assert_eq!(state.current_player, PlayerId(2));
    }

    #[test]
    fn eliminating_with_removed_moves_pawns_to_yard_and_drains_history() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let yard0 = topology.yard_spaces(PlayerColor(0))[0];
        let players = vec![player(0, 0, Vec::new()), player(1, 1, vec![CardKindId(0)])];
        let mut pawn = bare_pawn(PawnId(0), PlayerColor(0), entry0); // not already in yard
        pawn.push_move(
            MoveRecord {
                sequence: 0,
                claimed_cards: vec![CardKindId(1)],
                actual_cards: vec![CardKindId(1)],
                position_before: entry0,
                position_after: entry0,
                captures_caused: Vec::new(),
                reveal: RevealScope::Hidden,
            },
            3,
        );
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            vec![pawn],
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        let mut events = Vec::new();
        state.eliminate_player(PlayerId(0), EliminatedPawnHandling::Removed, &mut events);

        assert_eq!(state.pawns[0].position, yard0);
        assert_eq!(state.pawns[0].auditable_moves().count(), 0);
        assert!(events.iter().any(|e| matches!(
            e,
            GameEvent::CardsEnteredPile { cards, source: PileSource::EliminatedPlayerRedirect }
                if cards == &vec![CardKindId(1)]
        )));
        assert_eq!(
            state.shared_pile.take(10, &mut rand::rng()),
            vec![CardKindId(1)]
        );
    }

    #[test]
    fn no_available_action_auto_skip_offers_only_pass_and_advances_the_turn() {
        let topology = board();
        let yard0 = topology.yard_spaces(PlayerColor(0))[0];
        let entry1 = entry_of(&topology, PlayerColor(1));
        let players = vec![player(0, 0, Vec::new()), player(1, 1, vec![CardKindId(0)])];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), yard0),
            bare_pawn(PawnId(1), PlayerColor(1), entry1),
        ];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        assert_eq!(state.legal_actions(PlayerId(0)), vec![TurnAction::Pass]);

        let events = state.apply(TurnAction::Pass).unwrap();

        assert!(events.iter().any(|e| matches!(
            e,
            GameEvent::TurnPassed {
                player: PlayerId(0)
            }
        )));
        assert_eq!(state.current_player, PlayerId(1));
    }

    #[test]
    fn no_available_action_draw_card_grants_a_lifeline_from_the_pile() {
        let topology = board();
        let yard0 = topology.yard_spaces(PlayerColor(0))[0];
        let entry1 = entry_of(&topology, PlayerColor(1));
        let players = vec![player(0, 0, Vec::new()), player(1, 1, vec![CardKindId(0)])];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), yard0),
            bare_pawn(PawnId(1), PlayerColor(1), entry1),
        ];
        let rules = RuleConfig {
            no_available_action_behavior: NoAvailableActionBehavior::DrawCard(2),
            ..minimal_rules()
        };
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(vec![CardKindId(0), CardKindId(0)]),
            PlayerId(0),
        );

        let mut events = Vec::new();
        state.resolve_turn_start_gates(&mut events);

        assert!(events.iter().any(|e| matches!(
            e,
            GameEvent::CardsGrantedFromPile {
                player: PlayerId(0),
                count: 2
            }
        )));
        assert_eq!(state.players[0].hand, vec![CardKindId(0), CardKindId(0)]);
        // With cards in hand now, a real action is available, not just Pass.
        assert!(
            state
                .legal_actions(PlayerId(0))
                .iter()
                .any(|a| matches!(a, TurnAction::PlayCard(_)))
        );
    }

    #[test]
    fn no_available_action_draw_card_falls_back_to_pass_when_pile_is_short() {
        let topology = board();
        let yard0 = topology.yard_spaces(PlayerColor(0))[0];
        let entry1 = entry_of(&topology, PlayerColor(1));
        let players = vec![player(0, 0, Vec::new()), player(1, 1, vec![CardKindId(0)])];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), yard0),
            bare_pawn(PawnId(1), PlayerColor(1), entry1),
        ];
        let rules = RuleConfig {
            no_available_action_behavior: NoAvailableActionBehavior::DrawCard(2),
            ..minimal_rules()
        };
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()), // empty pile — the lifeline can't help
            PlayerId(0),
        );

        let mut events = Vec::new();
        state.resolve_turn_start_gates(&mut events);

        assert!(state.players[0].hand.is_empty());
        assert_eq!(state.legal_actions(PlayerId(0)), vec![TurnAction::Pass]);
    }

    #[test]
    fn false_accusation_against_a_frozen_pawn_redirects_payment_to_the_pile() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let entry1 = entry_of(&topology, PlayerColor(1));
        let players = vec![
            player(0, 0, Vec::new()),
            player(1, 1, vec![CardKindId(0)]), // the auditor's stake, if wrong
        ];
        let mut pawn0 = bare_pawn(PawnId(0), PlayerColor(0), entry0);
        pawn0.push_move(
            MoveRecord {
                sequence: 0,
                claimed_cards: vec![CardKindId(1)],
                actual_cards: vec![CardKindId(1)], // truthful — the accusation will be wrong
                position_before: entry0,
                position_after: entry0,
                captures_caused: Vec::new(),
                reveal: RevealScope::Hidden,
            },
            3,
        );
        let pawns = vec![pawn0, bare_pawn(PawnId(1), PlayerColor(1), entry1)];
        let mut state = GameState::new(
            topology,
            minimal_rules(), // false_accusation_destination: Auditee, cost: 1
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(1),
        );
        state
            .eliminated_players
            .insert(PlayerId(0), EliminatedPawnHandling::Frozen);

        let events = state
            .apply(TurnAction::Audit(AuditRequest {
                auditor: PlayerId(1),
                target_pawn: PawnId(0),
                target_move_index: 0,
                attempt_cost_cards: Vec::new(),
            }))
            .unwrap();

        assert!(events.iter().any(|e| matches!(
            e,
            GameEvent::CardsEnteredPile { cards, source: PileSource::EliminatedPlayerRedirect }
                if cards == &vec![CardKindId(0)]
        )));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, GameEvent::CardsTransferred { .. })),
            "a frozen auditee never actually receives the payment"
        );
    }

    #[test]
    fn capturing_a_frozen_players_pawn_drains_its_history_to_the_pile() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let defender_at = steps_from(&topology, PlayerColor(0), entry0, 2);
        let attacker_at = steps_from(&topology, PlayerColor(0), entry0, 1);
        let players = vec![player(0, 0, Vec::new()), player(1, 1, vec![CardKindId(0)])];
        let mut defender = bare_pawn(PawnId(0), PlayerColor(0), defender_at);
        defender.push_move(
            MoveRecord {
                sequence: 0,
                claimed_cards: vec![CardKindId(1)],
                actual_cards: vec![CardKindId(1)],
                position_before: defender_at,
                position_after: defender_at,
                captures_caused: Vec::new(),
                reveal: RevealScope::Hidden,
            },
            3,
        );
        let pawns = vec![defender, bare_pawn(PawnId(1), PlayerColor(1), attacker_at)];
        let rules = RuleConfig {
            // Isolate from the unrelated capture-reward pile draw, so the
            // only pile activity asserted on below is this fix's.
            capture_reward_from_pile: 0,
            ..minimal_rules()
        };
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(1),
        );
        state
            .eliminated_players
            .insert(PlayerId(0), EliminatedPawnHandling::Frozen);

        let events = state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(1),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();

        assert!(events.iter().any(|e| matches!(
            e,
            GameEvent::CardsEnteredPile { cards, source: PileSource::EliminatedPlayerRedirect }
                if cards == &vec![CardKindId(1)]
        )));
        assert_eq!(state.pawns[0].auditable_moves().count(), 0);
    }

    #[test]
    fn resolve_turn_start_gates_terminates_even_if_every_player_is_eliminated() {
        let topology = board();
        let players = vec![player(0, 0, Vec::new()), player(1, 1, Vec::new())];
        let rules = RuleConfig {
            cards_exhausted_behavior: CardsExhaustedBehavior::Eliminated(
                EliminatedPawnHandling::Frozen,
            ),
            ..minimal_rules()
        };
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            Vec::new(),
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        // The point of this test is that it returns at all: with every
        // player perpetually re-triggering gate 1, an unbounded loop would
        // hang here instead.
        let mut events = Vec::new();
        state.resolve_turn_start_gates(&mut events);

        assert_eq!(state.eliminated_players.len(), 2);
    }

    #[test]
    fn finished_pawn_dumps_history_to_the_pile_when_configured() {
        let topology = board();
        let color0 = PlayerColor(0);
        let last_lane_space = last_home_lane_space(&topology, color0);
        let players = vec![player(0, 0, vec![CardKindId(0)]), player(1, 1, Vec::new())];
        let mut pawn = bare_pawn(PawnId(0), color0, last_lane_space);
        // Seed a dormant history entry so there's something to dump once
        // this pawn crosses into Finish.
        pawn.push_move(
            MoveRecord {
                sequence: 0, // overwritten by push_move
                claimed_cards: vec![CardKindId(7)],
                actual_cards: vec![CardKindId(7)],
                position_before: last_lane_space,
                position_after: last_lane_space,
                captures_caused: Vec::new(),
                reveal: RevealScope::Hidden,
            },
            3,
        );
        let rules = RuleConfig {
            finished_pawn_dumps_history_destination: FinishedPawnHistoryDestination::SharedPile,
            ..minimal_rules()
        };
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            vec![pawn],
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        let events = state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            GameEvent::CardsEnteredPile { cards, source: PileSource::CapturedPawnFinished }
                if cards == &vec![CardKindId(7), CardKindId(0)]
        )));
        assert_eq!(state.pawns[0].auditable_moves().count(), 0);
        assert!(state.players[0].deck.is_empty());
    }

    #[test]
    fn finished_pawn_returns_history_to_owners_reserve_when_configured() {
        let topology = board();
        let color0 = PlayerColor(0);
        let last_lane_space = last_home_lane_space(&topology, color0);
        let players = vec![player(0, 0, vec![CardKindId(0)]), player(1, 1, Vec::new())];
        let mut pawn = bare_pawn(PawnId(0), color0, last_lane_space);
        pawn.push_move(
            MoveRecord {
                sequence: 0, // overwritten by push_move
                claimed_cards: vec![CardKindId(7)],
                actual_cards: vec![CardKindId(7)],
                position_before: last_lane_space,
                position_after: last_lane_space,
                captures_caused: Vec::new(),
                reveal: RevealScope::Hidden,
            },
            3,
        );
        let rules = RuleConfig {
            finished_pawn_dumps_history_destination: FinishedPawnHistoryDestination::OwnerReserve,
            ..minimal_rules()
        };
        let mut state = GameState::new(
            topology,
            rules,
            CardCatalog::standard(),
            players,
            vec![pawn],
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        let events = state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap();

        // Regression test: before the fix, `OwnerReserve` silently did
        // nothing, and since a finished pawn never moves again to trigger
        // history's normal aging-out eviction, the cards would have stayed
        // attached forever — neither in the pile nor recoverable by the
        // owner. Now they return to the reserve, and (since hand size is
        // below `hand_soft_cap`) the same turn's end-of-turn draw pulls
        // them straight back into hand.
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, GameEvent::CardsEnteredPile { .. }))
        );
        assert_eq!(state.pawns[0].auditable_moves().count(), 0);
        let mut hand = state.players[0].hand.clone();
        hand.sort_by_key(|c| c.0);
        assert_eq!(hand, vec![CardKindId(0), CardKindId(7)]);
    }

    #[test]
    fn legal_actions_enumerates_honest_plays_and_eligible_audits() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let entry1 = entry_of(&topology, PlayerColor(1));
        let players = vec![
            player(0, 0, vec![CardKindId(0), CardKindId(4)]),
            player(1, 1, Vec::new()),
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
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );
        state.pawns[1].push_move(
            MoveRecord {
                sequence: 0, // overwritten by push_move
                claimed_cards: vec![CardKindId(1)],
                actual_cards: vec![CardKindId(1)],
                position_before: entry1,
                position_after: entry1,
                captures_caused: Vec::new(),
                reveal: RevealScope::Hidden,
            },
            3,
        );

        let legal = state.legal_actions(PlayerId(0));
        let play_count = legal
            .iter()
            .filter(|action| matches!(action, TurnAction::PlayCard(_)))
            .count();
        let audit_count = legal
            .iter()
            .filter(|action| matches!(action, TurnAction::Audit(_)))
            .count();

        assert_eq!(play_count, 3); // [Take1], [Double], [Take1, Double]
        assert_eq!(audit_count, 1);
    }

    #[test]
    fn legal_actions_is_empty_for_a_player_who_isnt_current() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let entry1 = entry_of(&topology, PlayerColor(1));
        let players = vec![
            player(0, 0, vec![CardKindId(0)]),
            player(1, 1, vec![CardKindId(1)]),
        ];
        let pawns = vec![
            bare_pawn(PawnId(0), PlayerColor(0), entry0),
            bare_pawn(PawnId(1), PlayerColor(1), entry1),
        ];
        let state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        assert!(state.legal_actions(PlayerId(1)).is_empty());
    }

    #[test]
    fn playing_someone_elses_pawn_is_an_error() {
        let topology = board();
        let entry1 = entry_of(&topology, PlayerColor(1));
        let players = vec![
            player(0, 0, vec![CardKindId(0)]),
            player(1, 1, vec![CardKindId(1)]),
        ];
        let pawns = vec![bare_pawn(PawnId(1), PlayerColor(1), entry1)];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        let err = state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(1),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap_err();

        assert!(matches!(
            err,
            GameError::NotYourPawn {
                pawn: PawnId(1),
                player: PlayerId(0)
            }
        ));
    }

    #[test]
    fn claiming_a_card_not_in_hand_is_an_error() {
        let topology = board();
        let entry0 = entry_of(&topology, PlayerColor(0));
        let players = vec![player(0, 0, Vec::new()), player(1, 1, Vec::new())];
        let pawns = vec![bare_pawn(PawnId(0), PlayerColor(0), entry0)];
        let mut state = GameState::new(
            topology,
            minimal_rules(),
            CardCatalog::standard(),
            players,
            pawns,
            SharedPile::new(Vec::new()),
            PlayerId(0),
        );

        let err = state
            .apply(TurnAction::PlayCard(PlayedCard {
                declaration: Declaration {
                    pawn: PawnId(0),
                    claimed_cards: vec![CardKindId(0)],
                },
                actual_cards: vec![CardKindId(0)],
            }))
            .unwrap_err();

        assert!(matches!(err, GameError::CardNotInHand(CardKindId(0))));
    }
}
