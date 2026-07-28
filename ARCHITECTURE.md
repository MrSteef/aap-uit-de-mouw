# Architecture — `aap_uit_de_mouw_core`

This document specifies the Rust implementation of the game described in
[GAME_DESIGN.md](./GAME_DESIGN.md): modules, types, traits, and how they
interact. Read that document first if you haven't — this one assumes
familiarity with the rules it describes and won't re-explain them.

## Scope and context

This repository implements **only** the core game logic, in Rust, with
zero dependency on Unity, C#, or any rendering/UI layer. That's a
deliberate, permanent property of this crate, not a temporary starting
point — the project's plan is:

1. **Core game logic in Rust** (this repository).
2. **Thorough tests validating the rules** — much of this document exists
   to make that testing tractable (see §15).
3. Translate the validated Rust logic to C#, still fully decoupled from
   Unity.
4. Bridge scripts connecting that C# logic to Unity.
5. Unity-side presentation and world/scene scripts.

Only steps 1 and 2 happen in this repository. Steps 3–5 are future work in
a different codebase, but they shape decisions made here: prefer plain
data and pure functions over anything Rust-idiomatic-but-hard-to-port
(no unsafe, no exotic trait machinery, no macro-generated control flow
that would have no obvious C# equivalent); keep hidden information and
its redaction centralized (§11) rather than scattered, since a real
networked/UI-facing version will need the same seam; and treat the
`GameEvent` log (§13) as the contract a future presentation layer consumes
— it should stay a complete, replayable record of everything that
happened, not an incidental side effect.

## How to read the doc comments in this document

Throughout this document, `///`-prefixed comments on structs, fields, and
methods are **design notes for you, the reader** — they often explain
*why* a shape was chosen, what alternative was rejected, or how a piece
fits into the bigger picture. That's appropriate for a spec, but it is
**not** what the real rustdoc on the finished code should look like.

When implementing: write the actual `///` doc comment to describe what
the item *does* and its contract, useful to someone calling it who
doesn't care how it's built or why. Move rationale, alternatives
considered, and implementation notes into regular `//` comments near the
relevant code, or leave them here in this document — don't carry design
narration into rustdoc output. See `CLAUDE.md` for the full coding
standard this applies under.

## Engineering philosophy: default to a gamerule

The game design leans hard on `RuleConfig` — see `GAME_DESIGN.md`'s
Philosophy section for why. That preference should extend to
implementation decisions this document doesn't explicitly cover: if you
hit a case where a mechanic's exact behavior is a judgment call that a
different ruleset might reasonably want to change, default to exposing it
as a new `RuleConfig` field rather than hardcoding a choice. Reach for a
hardcoded constant only for things that aren't really "rules" in the
gameplay sense — geometry/graph mechanics, data-structure choices, and
similar implementation details.

---

## 1. Crate & module layout

```
aap_uit_de_mouw_core/
└── src/
    ├── lib.rs
    ├── board.rs
    ├── rules.rs
    ├── card/
    │   ├── mod.rs                    // CardKindId, CardBehavior, CardCategory, CardMeta, CardCatalog
    │   ├── movement/
    │   │   ├── mod.rs
    │   │   └── move_card.rs
    │   ├── movement_modifier/
    │   │   ├── mod.rs
    │   │   ├── double_modifier_card.rs
    │   │   └── rampage_modifier_card.rs
    │   ├── offense/
    │   │   └── mod.rs                // unpopulated for now — reserved for a future
    │   │                             // card that captures without moving at all
    │   ├── defense/
    │   │   ├── mod.rs
    │   │   └── shield_card.rs
    │   └── deception/
    │       ├── mod.rs
    │       └── stun_trap_card.rs
    ├── context/
    │   ├── mod.rs
    │   ├── play_context.rs
    │   ├── interaction_context.rs
    │   └── audit_context.rs
    ├── deck.rs
    ├── player.rs
    ├── agent/
    │   ├── mod.rs                    // PlayerAgent trait
    │   ├── random_agent.rs
    │   └── scripted_agent.rs
    ├── pawn.rs
    ├── play.rs
    ├── movement.rs
    ├── audit.rs
    ├── view.rs
    ├── driver.rs
    ├── event.rs
    ├── error.rs
    └── game.rs
```

**Dependency direction:**

```
pawn ──> board
play ──> card
context ──> board, rules, card, pawn, event
movement ──> board, rules, pawn
card ──> context
audit ──> card, rules, pawn
deck ──> card
player ──> deck, card
view ──> board, card, pawn, player
agent ──> view, card, game
driver ──> agent, game
game ──> board, rules, card, deck, player, pawn, play, movement, context, audit, view, event, error
```

`board.rs`, `rules.rs`, and `card/`'s data types sit at the bottom of the
graph with no internal dependencies — the easiest things to unit-test
exhaustively without a running game.

---

## 2. Board topology — `board.rs`

The board is an abstract directed graph of spaces — no coordinates, no
literal shape. A standard Ludo ring is simply the boring default case
of this graph; a fork (for a future branching-path board) is the same
structure with more than one edge out of a node. This is also what lets a
Unity scene bind arbitrary objects to spaces later without the Rust side
knowing or caring where anything physically sits.

A yard space's own outgoing edge points wherever that color enters the
shared ring — entry points aren't a separate concept, just an edge like any
other, which also means different yard slots could exit to different
points if a future map wanted that.

At a color's home-lane fork, the shared space just before it has two
same-color-eligible edges: one continuing around the ring, one turning
into the home lane — which, left alone, is a real `Branch` rather than a
forced turn, letting that color choose to loop again. `Edge::forced`
exists to override that where a mandatory turn is wanted instead.

```rust
pub struct SpaceId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpaceKind { Yard, Shared, HomeLane, Finish }

pub struct Edge {
    pub to: SpaceId,
    /// `None` = any pawn may take this edge. `Some(color)` = only that
    /// color may — this is how a shared space "forks" into one color's
    /// private home lane at a different point than another color's.
    pub restricted_to: Option<PlayerColor>,
    /// If true, and this edge is eligible for a given color, every other
    /// edge from the same node is ignored for that color — used to make
    /// entering the home lane mandatory rather than an optional choice
    /// against looping the shared ring again.
    pub forced: bool,
}

pub struct SpaceNode {
    pub id: SpaceId,
    pub kind: SpaceKind,
    pub owner: Option<PlayerColor>,
    pub safe: bool,
    /// More than one edge today only happens at a color-specific fork.
    /// More than one edge with the *same* restriction, neither `forced`,
    /// is a genuine branch — nothing produces one yet, but the shape
    /// already supports it.
    pub edges: Vec<Edge>,
}

pub struct BoardTopology {
    nodes: Vec<SpaceNode>,
    yard_spaces: std::collections::HashMap<PlayerColor, Vec<SpaceId>>,
}

pub enum NextSpace { Single(SpaceId), Branch(Vec<SpaceId>), DeadEnd }

impl BoardTopology {
    pub fn node(&self, id: SpaceId) -> &SpaceNode { &self.nodes[id.0 as usize] }

    pub fn next_space(&self, from: SpaceId, owner: PlayerColor) -> NextSpace {
        let eligible = |e: &&Edge| e.restricted_to.is_none() || e.restricted_to == Some(owner);
        let node = self.node(from);
        // A forced edge, if eligible, wins outright — no other edge from
        // this node is even considered for this color.
        if let Some(forced) = node.edges.iter().find(|e| e.forced && eligible(e)) {
            return NextSpace::Single(forced.to);
        }
        let candidates: Vec<SpaceId> = node.edges.iter().filter(eligible).map(|e| e.to).collect();
        match candidates.len() {
            0 => NextSpace::DeadEnd,
            1 => NextSpace::Single(candidates[0]),
            _ => NextSpace::Branch(candidates),
        }
    }

    /// One recipe for a symmetric ring-plus-home-lanes board. Custom or
    /// asymmetric boards are built the same way — this is a constructor,
    /// not a special case baked into the type.
    pub fn standard_ring(
        num_players: u8, ring_len: u16, home_lane_len: u16, pawns_per_player: u8,
    ) -> Self { todo!() }
}
```

---

## 3. Rule configuration — `rules.rs`

Every optional rule — from which tradition's mechanics apply, to every
cost, reward, and cap in the card economy — lives here as data, not as a
branch buried somewhere in the engine.

```rust
pub struct RuleConfig {
    pub pawn_count: u8,
    pub exit_rule: ExitRule,
    pub blockades_enabled: bool,
    pub capture_sends_to_yard: bool,
    pub bonus_turn_on_capture: bool,
    pub bonus_turn_on_exit: bool,
    pub exact_count_to_finish: bool,

    pub audit_window: u8,
    pub max_audits_per_turn: u8,
    pub revert_captures_on_lie: bool,
    /// Paid unconditionally, the moment a challenge is submitted —
    /// regardless of whether it turns out right or wrong. Zero by default
    /// (challenging is free to attempt); distinct from and additional to
    /// `false_accusation_card_cost` below, which only applies on top of
    /// this when the challenge turns out wrong. Zero is especially
    /// sensible when `auditing_costs_turn` is also true — the lost turn is
    /// already a real cost — but the field is independent and meaningful
    /// either way.
    ///
    /// A player may only submit a challenge at all if their hand can cover
    /// the worst case, `audit_attempt_cost + false_accusation_card_cost` —
    /// otherwise they could end up owing more than they have if they turn
    /// out to be wrong. `legal_actions` enforces this directly.
    pub audit_attempt_cost: u8,
    pub audit_attempt_cost_destination: CardDestination,
    pub audit_attempt_cost_selection: PaymentSelectionMode,
    /// How many *additional* cards a wrong accusation costs the
    /// challenger, on top of `audit_attempt_cost` — one, by default, but a
    /// harsher or gentler ruleset can change it.
    pub false_accusation_card_cost: u8,
    pub false_accusation_destination: CardDestination,
    pub false_accusation_selection: PaymentSelectionMode,
    /// Whether submitting a challenge consumes the challenger's entire
    /// turn — false by default (a challenge is a free action you can
    /// still follow with a card play), but a stricter ruleset can make
    /// challenging cost the whole turn instead.
    pub auditing_costs_turn: bool,
    /// Can a captured pawn's pre-capture moves still be actively challenged
    /// while it sits in the yard? The data is retained regardless (§9) —
    /// this only controls whether `legal_actions` offers them as targets.
    pub captured_pawns_remain_auditable: bool,
    /// Controls the *event log's* visibility, not any lingering game state
    /// (§11) — false means only the auditor/auditee learn which specific
    /// cards were collected; everyone else just sees that a transfer
    /// happened.
    pub reveal_collected_cards_publicly: bool,

    pub playing_card_mandatory: bool,
    pub max_cards_per_play: u8,
    pub max_cards_per_category_per_play: std::collections::HashMap<CardCategory, u8>,
    /// If false (the default), `claimed_cards.len()` must equal
    /// `actual_cards.len()` for a play to be legal — identities can still
    /// differ freely, but a mismatched count is an obvious tell.
    pub allow_card_count_mismatch: bool,

    pub starting_deck_size: u8,
    /// May exceed `hand_soft_cap` deliberately — a one-time game-start
    /// boost, exempt from the overflow rules below (it's the genesis of a
    /// player's economy, not an inflow into it).
    pub starting_hand_size: u8,
    /// Draw target: topped up from a player's own reserve at the **end**
    /// of their turn, never exceeded by that routine draw itself.
    pub hand_soft_cap: u8,
    /// Absolute ceiling, checked only against *external* inflows (audit
    /// rewards, false-accusation transfers, pile grants) — never against
    /// the routine end-of-turn draw. Overflow redirects to the reserve.
    pub hand_hard_cap: u8,
    /// Reserve ceiling, checked the same way against external inflows.
    /// Overflow past this (hand *and* reserve both full) redirects to the
    /// shared pile.
    pub deck_cap: u8,
    /// A pawn's aged-out history returning to its owner's own reserve may
    /// exceed `deck_cap` — an internal transfer, not an inflow, so it's a
    /// soft cap for this source specifically.
    pub aged_out_exempt_from_deck_cap: bool,

    pub starting_pile_size: u8,
    /// Cards granted to a player who successfully captures, drawn from the
    /// shared pile — never an error if the pile is short, just however
    /// many it can currently give.
    pub capture_reward_from_pile: u8,
    /// A Shield-style automatic check (triggered by a capture *attempt*,
    /// not chosen by a player) still fully applies its revert if it
    /// catches a lie — but the collected cards go to the shared pile by
    /// default, not to the attacking player, since they staked nothing.
    pub automatic_audit_reward_destination: bool,
    /// When a pawn reaches Finish, whatever's still attached to its
    /// history (hasn't yet aged out) redirects to the shared pile instead
    /// of ever reaching the owner's reserve — a deliberate cost for
    /// completing a pawn's journey.
    pub finished_pawn_dumps_history_destination: bool,
    /// On a deliberate audit that catches a lie: if true, only the
    /// *directly audited* move's cards go to the auditor, and cards from
    /// the newer moves swept up in the cascade go to the shared pile
    /// instead (the audited move is always the oldest one reverted — older
    /// moves than that are never touched at all). If false, the auditor
    /// collects the whole chain.
    pub cascade_lie_rewards_destination: bool,

    /// Checked first, independently of whether the player could otherwise
    /// still act (e.g. via a dormant collectible pawn in their yard) —
    /// this is specifically about a player's own hand and deck both being
    /// empty, full stop. If this eliminates them, `no_available_action_
    /// behavior` below never gets a chance to apply. Deliberately separate
    /// and harsher: a ruleset can decide that letting your own cards run
    /// out is punished on its own terms, regardless of whatever fallback
    /// options happened to still be available. See §10.
    pub cards_exhausted_behavior: CardsExhaustedBehavior,
    /// What happens to a player who — having survived the check above —
    /// starts their turn with an empty hand and no dormant cards to cash
    /// in from any captured pawn in their yard either, i.e. genuinely no
    /// legal action available. See §10.
    pub no_available_action_behavior: NoAvailableActionBehavior,
}

pub enum ExitRule { Automatic, RequiresCard(CardKindId) }

/// How the payer's cards are picked for either audit-related payment.
/// "Blind" isn't about the recipient — it's about denying the payer any
/// ability to signal something through a deliberate choice of which card
/// to give up.
pub enum PaymentSelectionMode { PayerChooses, RandomDraft }

/// Where a payment goes — independently configurable for each of the two
/// audit payments. `Auditee` doesn't apply cleanly to a case with no
/// specific auditee in view, but both payments described here always have
/// one, so it's available to either.
pub enum CardDestination { SharedPile, Auditee }

pub enum CardsExhaustedBehavior { Ignored, Eliminated(EliminatedPawnHandling) }

/// Renamed from an earlier `EmptySupplyBehavior` — the actual condition
/// that triggers this is having no legal action at all, not specifically
/// an empty card supply, even though an empty supply is the usual cause.
pub enum NoAvailableActionBehavior {
    /// Simply can't play that turn (regardless of `playing_card_mandatory`,
    /// which can't force the impossible) and stays in the game — a capture
    /// or a favorable false accusation can still pull them back in later.
    AutoSkip,
    /// `n` cards are drawn from the shared pile at the *start* of the turn
    /// (never an error if the pile is short — draws fewer than `n`, and
    /// falls back to `AutoSkip`-like behavior for whatever's still missing).
    /// A movement card among them can be played normally; otherwise one can
    /// still be spent on `audit_attempt_cost` to attempt a challenge — `n`
    /// is worth setting above 1 precisely so a player isn't left with just
    /// enough for one option but not the other, if `audit_attempt_cost` is
    /// more than a single card. Deliberately a start-of-turn check rather
    /// than folded into the routine end-of-turn draw, since a pawn of
    /// theirs could be captured during another player's turn in between,
    /// handing them a fresh collect-from-yard option that wasn't there a
    /// moment ago.
    DrawCard(u8),
    /// Running out is itself a loss condition.
    Eliminated(EliminatedPawnHandling),
}

pub enum EliminatedPawnHandling { Frozen, Removed }
```

---

## 4. Card contexts — `context/`

The restricted API surface every card hook acts through — narrow on
purpose, so a card can't do anything the context doesn't expose, and so
card behavior is testable in isolation with a mock context rather than a
full running game.

```rust
// context/play_context.rs
pub enum CaptureMode { LandingSquareOnly, EveryStepPassed }

/// What a play's claimed cards accumulate into before one movement
/// resolves. A plain movement card adds to `steps`; a double-style
/// modifier multiplies `multiplier`; a rampage-style modifier upgrades
/// `capture_mode`. Cards that don't affect movement (Shield, Stun Trap)
/// simply don't touch it.
pub struct MovementProposal {
    pub steps: u8,
    pub multiplier: u8,
    pub capture_mode: CaptureMode,
}
impl Default for MovementProposal {
    fn default() -> Self {
        Self { steps: 0, multiplier: 1, capture_mode: CaptureMode::LandingSquareOnly }
    }
}

/// Anchors a persistent effect to a pawn (follows it wherever it goes) or
/// a space (stays behind after whichever pawn triggered it leaves).
pub enum EffectAnchor { Pawn(PawnId), Space(SpaceId) }

pub enum CaptureOutcome { Proceeds, Blocked }

pub struct PlayContext<'a> {
    topology: &'a BoardTopology,
    rules: &'a RuleConfig,
    catalog: &'a CardCatalog,
    pawns: &'a mut Vec<Pawn>,
    space_effects: &'a mut std::collections::HashMap<SpaceId, Vec<PersistentEffectState>>,
    mover: PawnId,
    events: Vec<GameEvent>,
}

impl<'a> PlayContext<'a> {
    pub fn mover(&self) -> PawnId { self.mover }

    /// Called once, after every claimed card's `on_claimed` has contributed
    /// to a `MovementProposal` — resolves the combined walk, calling
    /// `attempt_capture` for every square touched that qualifies under
    /// `capture_mode`. A no-op if `proposal.steps == 0` (a play made
    /// entirely of non-movement cards, e.g. Shield alone).
    pub fn resolve_movement(&mut self, proposal: MovementProposal) { todo!() }

    /// General-purpose capture — used internally by `resolve_movement`,
    /// but also directly callable by a future card with no movement
    /// involved at all. Independently checks the target's *real*
    /// persistent effects (dispatching `on_capture_attempted_as_played` to
    /// each) and its outstanding *claimed* ones (dispatching
    /// `on_capture_attempted_as_claimed`) — no priority between the two;
    /// whichever exist get called. Blocked if any dispatched hook says so.
    pub fn attempt_capture(&mut self, target: PawnId) -> CaptureOutcome { todo!() }

    /// Attaches whichever card is currently executing `on_played` to
    /// `anchor` — the context already knows which card that is.
    pub fn attach_persistent_effect(&mut self, anchor: EffectAnchor) { todo!() }
    pub fn emit(&mut self, event: GameEvent) { self.events.push(event); }
}
```

```rust
// context/interaction_context.rs
pub struct InteractionContext<'a> {
    pub attacker: PawnId,
    pub defender: PawnId,
    /// false = a mid-path square being passed through; true = the move's
    /// final resting square — a fact a card can check if it cares, rather
    /// than a distinct hook of its own.
    pub is_landing: bool,
    events: &'a mut Vec<GameEvent>,
}

impl<'a> InteractionContext<'a> {
    pub fn reveal_publicly(&mut self) { todo!() }
    /// Compares the outstanding claimed-vs-actual card for whatever is
    /// currently being tested and, if they differ, applies the same
    /// cascading-revert consequences as a deliberate audit — with the
    /// *attacking* pawn's owner standing in as auditor. No penalty applies
    /// if the claim turns out true; nobody chose to gamble here. Idempotent
    /// per capture attempt: whichever of `_as_played`/`_as_claimed` calls
    /// this first resolves it; the other's call, if any, is a no-op.
    pub fn trigger_automatic_audit(&mut self) { todo!() }
    pub fn emit(&mut self, event: GameEvent) { self.events.push(event); }
}
```

```rust
// context/audit_context.rs
pub struct AuditContext<'a> {
    pub auditor: PlayerId,
    pub auditee: PlayerId,
    pub target_pawn: PawnId,
    forfeit_auditor_turn: &'a mut bool,
}

impl<'a> AuditContext<'a> {
    pub fn forfeit_auditor_turn(&mut self) { *self.forfeit_auditor_turn = true; }
}
```

**Implementation status.** Fully implemented (§16 steps 3, 4, and 7), with
several deviations from what's shown above:

- `EffectAnchor` actually lives in `pawn.rs`: `PersistentEffectState` (§8)
  needs it, and this section's own dependency graph (§1) has `context`
  depend on `pawn`, never the reverse — defining it here would create a
  cycle.
- `resolve_movement` returns `Result<Vec<(PawnId, SpaceId)>, MoveError>`,
  not `()`. The `Result` is for the same reason as before (`movement::walk`
  can fail); the `Vec` reports every pawn it captured, as
  `(pawn, position)` pairs, since the caller needs that to build the
  mover's eventual `MoveRecord.captures_caused` — nothing else surfaces
  this data.
- `PlayContext` has a `new(...)` constructor (all its fields are private)
  and a `current_card: Option<CardKindId>` field, set by a `begin_card(id)`
  method the caller invokes just before dispatching `on_played`/
  `on_claimed` for a given card. This is the concrete mechanism behind "the
  context already knows which card that is" in `attach_persistent_effect`'s
  doc comment above.
- `attach_persistent_effect` gains an `expires: Option<ExpiryCondition>`
  parameter — needed so `ShieldCard` (§5) can actually populate
  `PersistentEffectState.expires`; there was no way to do so otherwise.
  `PlayContext` also gains `attach_claimed_effect(&mut self, anchor:
  EffectAnchor)`, entirely new: something has to attach a
  `ClaimedEffectState` when a card is *claimed* (as opposed to `on_played`
  attaching the real one), and nothing above shows how — `ShieldCard`'s
  `on_claimed` (§5) calls it. Only `EffectAnchor::Pawn` is meaningful for
  claims; `ClaimedEffectState` has no space-anchored equivalent
  (§8), so a `Space`-anchored claim is silently dropped.
- `attempt_capture` takes an added `is_landing: bool` parameter — only the
  caller (`resolve_movement`) knows whether a given square is the move's
  final one or a mid-path square under `CaptureMode::EveryStepPassed`, and
  `InteractionContext.is_landing` needs that fact from somewhere.
- `InteractionContext` gains `topology`, `rules`, and `pawns` fields (all
  types `context` already depends on elsewhere, so no new dependency-graph
  edge) so `trigger_automatic_audit` can actually perform a revert — see
  its own implementation-status note below.
- `AuditContext` gains a `new(...)` constructor, for the same private-field
  reason as `PlayContext`.

`trigger_automatic_audit`'s revert mechanics are shared with
`audit::resolve` (§9) via `pawn::revert` — see §8's implementation-status
note for why that lives in `pawn.rs` rather than `audit.rs`. Two
simplifications, both flagged in code comments:
- It tests the defender's *newest* auditable move, not necessarily the
  specific move that attached the effect being tested —
  `PersistentEffectState`/`ClaimedEffectState` don't record which history
  entry created them, and safely linking one (indices shift as older moves
  age out) is a bigger structural change than this step's scope. In
  practice a capture attempt follows shortly after the relevant claim/play,
  so the newest move is almost always the right one.
- It clears *all* of the defender's claimed effects once tested (matching
  `claimed_effects`'s own doc comment: "resolved... the moment
  `trigger_automatic_audit` tests them, one way or another"), not just the
  one actually responsible, for the same reason.

Also out of scope, matching `audit::resolve`'s own scoping (§9): routing
any collected cards to the shared pile
(`RuleConfig::automatic_audit_reward_destination`) — that's `GameState`'s
job (§16 step 8).

---

## 5. Cards — `card/`

Cards are data-driven behavior, not a closed enum — a new card is a new
type implementing `CardBehavior`, registered in the catalog, not a new
branch threaded through the engine.

```rust
// card/mod.rs
pub struct CardKindId(pub u16);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum CardCategory { Movement, MovementModifier, Offense, Defense, Deception }

pub trait CardBehavior {
    /// Fires the instant this card is *actually* consumed from hand. Cards
    /// that attach hidden state (Shield) act here; plain movement cards
    /// need nothing.
    fn on_played(&self, _ctx: &mut PlayContext) {}

    /// Fires once per *claimed* card in a play. Movement-contributing
    /// cards mutate `proposal`; everything else acts through `ctx`
    /// directly. `resolve_movement` runs once, after every claimed card in
    /// the play has had a turn at `proposal`.
    fn on_claimed(&self, _ctx: &mut PlayContext, _proposal: &mut MovementProposal) {}

    /// Any touch — landing or passing — on a space where this card is
    /// attached, as this pawn's *actual* state. Purely informational;
    /// nothing built so far needs it, kept general for a future
    /// non-combat trigger (a toll, say).
    fn on_passed_as_played(&self, _ctx: &mut InteractionContext) {}
    fn on_passed_as_claimed(&self, _ctx: &mut InteractionContext) {}

    /// Specifically an attempted capture — via landing, via a rampage-style
    /// pass-through, or via a direct `attempt_capture` call with no
    /// movement at all. The one hook that can actually block it.
    fn on_capture_attempted_as_played(&self, _ctx: &mut InteractionContext) -> CaptureOutcome { CaptureOutcome::Proceeds }
    fn on_capture_attempted_as_claimed(&self, _ctx: &mut InteractionContext) -> CaptureOutcome { CaptureOutcome::Proceeds }

    fn on_audited_as_played(&self, _outcome: AuditOutcome, _ctx: &mut AuditContext) {}
    fn on_audited_as_claimed(&self, _outcome: AuditOutcome, _ctx: &mut AuditContext) {}
}

pub struct CardMeta {
    pub id: CardKindId,
    pub display_name: &'static str,
    pub category: CardCategory,
    pub behavior: Box<dyn CardBehavior + Send + Sync>,
}

pub struct CardCatalog { definitions: Vec<CardMeta> }
impl CardCatalog {
    pub fn get(&self, id: CardKindId) -> &CardMeta { &self.definitions[id.0 as usize] }
    pub fn standard() -> Self { todo!() }
}
```

**Implementation status.** `CardCatalog::get` returns `Option<&CardMeta>`
rather than panicking on an unknown id, consistent with the
panic-avoidance approach used throughout (see `board.rs`'s `node()`).
`standard()` now registers every card built so far under ids 0–7 (`Take
1`–`Take 4`, `Double`, `Rampage`, `Shield`, `Stun Trap`) — `offense/` is
still unpopulated, reserved for a future card that captures without
moving at all.

```rust
// card/movement/move_card.rs
pub struct MoveCard { pub steps: u8 }
impl CardBehavior for MoveCard {
    fn on_claimed(&self, _ctx: &mut PlayContext, proposal: &mut MovementProposal) {
        proposal.steps += self.steps;
    }
}
```

```rust
// card/movement_modifier/double_modifier_card.rs
/// A pure modifier — no base steps of its own. Needs a `MoveCard` (or
/// another steps-contributing card) in the same play to do anything;
/// `RuleConfig::max_cards_per_category_per_play` bounds how many of these
/// one play may stack.
pub struct DoubleModifierCard { pub multiplier: u8 }
impl CardBehavior for DoubleModifierCard {
    fn on_claimed(&self, _ctx: &mut PlayContext, proposal: &mut MovementProposal) {
        proposal.multiplier *= self.multiplier;
    }
}
```

```rust
// card/movement_modifier/rampage_modifier_card.rs
/// Also a pure modifier, for the same reason as Double — it upgrades
/// whatever movement is already being claimed rather than carrying its own
/// step count.
pub struct RampageModifierCard;
impl CardBehavior for RampageModifierCard {
    fn on_claimed(&self, _ctx: &mut PlayContext, proposal: &mut MovementProposal) {
        proposal.capture_mode = CaptureMode::EveryStepPassed;
    }
}
```

```rust
// card/defense/shield_card.rs
/// How long a played Shield stays active — a property of the specific
/// card, not a `RuleConfig` toggle: different `CardKindId`s in the catalog
/// can each wrap `ShieldCard` with a different duration, so "1-turn
/// Shield" and "until it ages out Shield" can coexist as distinct cards.
pub enum ShieldDuration {
    Turns(u8),
    UntilPawnMoves,
    UntilHistoryExpires,
}

pub struct ShieldCard { pub duration: ShieldDuration }
impl CardBehavior for ShieldCard {
    fn on_played(&self, ctx: &mut PlayContext) {
        ctx.attach_persistent_effect(EffectAnchor::Pawn(ctx.mover()));
    }
    /// The real card being tested — apply its actual effect, and trigger
    /// the automatic audit so the corresponding claim (if any) resolves as
    /// a side effect of being tested.
    fn on_capture_attempted_as_played(&self, ctx: &mut InteractionContext) -> CaptureOutcome {
        ctx.trigger_automatic_audit();
        CaptureOutcome::Blocked
    }
    /// Only ever reached when there's a claimed Shield with no real one
    /// backing it — `attempt_capture`'s independent checks mean the played
    /// hook above already handles the case where a real one exists. All
    /// this needs to do is reveal that via the same audit trigger; it never
    /// blocks anything itself.
    fn on_capture_attempted_as_claimed(&self, ctx: &mut InteractionContext) -> CaptureOutcome {
        ctx.trigger_automatic_audit();
        CaptureOutcome::Proceeds
    }
}
```

```rust
// card/deception/stun_trap_card.rs
pub struct StunTrapCard;
impl CardBehavior for StunTrapCard {
    fn on_audited_as_played(&self, _outcome: AuditOutcome, ctx: &mut AuditContext) {
        ctx.forfeit_auditor_turn();
    }
}
```

**Implementation status.** `StunTrapCard` is exactly as shown above.
`ShieldCard` matches too, except its `duration` field is typed
`pawn::ExpiryCondition` directly rather than a separate `ShieldDuration`
enum: the two had the exact same three variants
(`Turns(u8)`/`UntilPawnMoves`/`UntilHistoryExpires` vs.
`AfterTurns(u8)`/`OnPawnMoved`/`WithSourceHistoryItem`), so keeping both
and converting between them would have been pure ceremony.
`ShieldCard` also has an `on_claimed` override — not shown above — that
calls the new `PlayContext::attach_claimed_effect` (§4); nothing in this
section shows how a claimed Shield's `ClaimedEffectState` ever gets
attached otherwise. See §4's implementation-status note for that and the
other `PlayContext`/`InteractionContext` additions this needed.

Shield's exact bookkeeping is genuinely one of the fiddlier pieces here.
A *claimed* Shield and a *real* Shield need to be tracked separately,
because they can already anchor differently today — claiming Shield (which
would attach to the pawn) while actually playing a plain `MoveCard` (which
attaches nothing at all) is already a mismatch. So a single combined
"claim+actual" record per attachment doesn't work, even for a single card
type. Kept as two independent lists on `Pawn` (§8) rather than one, for
that reason. Both are reasonable starting guesses, not final answers —
see §4's implementation-status note for the simplifications
`trigger_automatic_audit` actually landed on.

Two more card ideas are worth flagging as hooks this trait will eventually
need, without building them out now:
- **"Capturing this pawn rewards you with an extra card"** — the reverse
  of Shield's shape: it needs to fire on a *confirmed* capture, not an
  *attempted* one, so it needs a genuinely different hook
  (`on_captured_as_played`, say) rather than a variant of the existing
  capture-attempt pair.
- **Cards that raise a player's `hand_soft_cap` / `hand_hard_cap` /
  `deck_cap`** — these act on the owning player's own economy rather than
  the board, so they'd need `PlayContext` to expose a way to adjust a
  player's effective caps, likely via `on_played` alone.

---

## 6. Movement resolution — `movement.rs`

The one traversal algorithm — a plain function rather than a trait, since
there's exactly one implementation and no reason for indirection. Invoked
with an already-combined `MovementProposal`, not a single card's numbers.

```rust
pub struct MovementOutcome { pub squares_passed: Vec<SpaceId>, pub final_space: SpaceId }
pub enum MoveError { BlockedByBlockade, Overshoot, UnresolvedBranch, DeadEnd }

pub fn walk(
    topology: &BoardTopology, rules: &RuleConfig, pawns: &[Pawn],
    owner: PlayerColor, from: SpaceId, steps: u8,
) -> Result<MovementOutcome, MoveError> { todo!() }
```

**Implementation status.** Fully implemented (§16 step 3), including
blockade checks (two or more of the same color stacked on a space blocks
everyone, when `rules.blockades_enabled`) and the finish-line rule
(`rules.exact_count_to_finish`). `MoveError` gained a fifth variant,
`InvalidBoard(#[from] BoardError)`, so an unknown `SpaceId` propagates as a
`Result` instead of panicking, consistent with `board.rs`'s own
panic-avoidance fix. Reaching the per-color home-lane fork (the one place
`next_space` returns a real `Branch`) yields `MoveError::UnresolvedBranch`
— there's no branch-resolution mechanism yet, so a pawn can't cross its own
fork via `walk` alone.

---

## 7. The claim/actual split — `play.rs`

The pairing that makes bluffing possible: what was announced, and what
really happened.

```rust
pub struct Declaration {
    pub pawn: PawnId,
    pub claimed_cards: Vec<CardKindId>,
}

pub struct PlayedCard {
    pub declaration: Declaration,
    pub actual_cards: Vec<CardKindId>,
}
```

`RuleConfig::max_cards_per_play` / `max_cards_per_category_per_play` bound
how large `claimed_cards` may legally be, enforced when `legal_actions`
enumerates possible plays — not by the type itself.

---

## 8. Pawns, persistent effects & move history — `pawn.rs`

A pawn's history persists until it next leaves the yard — capture only
clears its `position` and possibly `persistent_effects`, never `history`
itself. That's what makes "reinstate with history intact" possible at all. 
No separate field is needed to remember a pre-capture position: it's already
sitting in the *capturing* pawn's own `MoveRecord` (`captures_caused`), and
it's only ever needed while that record is still within the capturing
pawn's own audit window anyway. Cards tied up in such a captured pawn remain
tied up in that pawn until it either leaves the yard or the player manually
reclaims the cards, thereby forfeiting the possiblity of reinstating that pawn.

```rust
pub struct Pawn {
    pub id: PawnId,
    pub owner: PlayerId,
    pub position: SpaceId,
    persistent_effects: Vec<PersistentEffectState>,
    /// Outstanding *claims* of a persistent effect, tracked separately from
    /// real ones — see the anchor-mismatch note in §5. Resolved (removed)
    /// the moment `trigger_automatic_audit` tests them, one way or another.
    claimed_effects: Vec<ClaimedEffectState>,
    history: std::collections::VecDeque<MoveRecord>,   // capacity == rules.audit_window
}

/// No separate `PersistentEffect` tag needed — `source_card` already
/// identifies which `CardBehavior` to dispatch to via the catalog.
pub struct PersistentEffectState {
    pub source_card: CardKindId,
    pub anchor: EffectAnchor,
    pub revealed: bool,
    /// Some effects expire on their own — Shield's `ShieldDuration` (§5)
    /// needs to live and tick down somewhere, and this is that somewhere.
    /// Exact shape left open until Shield is actually implemented.
    pub expires: Option<ExpiryCondition>,
}

pub enum ExpiryCondition { AfterTurns(u8), OnPawnMoved, WithSourceHistoryItem }

pub struct ClaimedEffectState {
    pub source_card: CardKindId,
    pub anchor: EffectAnchor,
}

#[derive(Clone)]
pub struct MoveRecord {
    pub claimed_cards: Vec<CardKindId>,
    pub actual_cards: Vec<CardKindId>,
    pub position_before: SpaceId,
    pub position_after: SpaceId,
    pub captures_caused: Vec<(PawnId, SpaceId)>,
    pub reveal: RevealScope,
}

/// Only meaningful for records that *stay* in history — a move proven true
/// by a failed accusation. A caught lie's records leave history entirely
/// for the auditor's hand (§10), so there's nothing left here to track
/// visibility for; `VisibleTo(...)`-style partial visibility was
/// considered and dropped for exactly that reason.
pub enum RevealScope { Hidden, Public }
```

**Implementation status.** Fully built (§16 step 6): `ClaimedEffectState`,
`MoveRecord` (plus `PartialEq`/`Eq`/`Debug`, not just `Clone`, for test
assertions), and `RevealScope` all match this section exactly. `Pawn` has
its full field set now (`persistent_effects`, `claimed_effects`, `history`
all present).

`Pawn.owner` is `PlayerColor` here, not `PlayerId` as shown above:
`movement::walk`'s blockade check needs to know which *board color* owns
each pawn, and it only takes a bare `pawns: &[Pawn]` slice with no
accompanying `Player` list to resolve `PlayerId` → color. Revisited in
`audit.rs` (§9), which turned out to be the first place that genuinely
needs a `PlayerId` (for `AuditContext.auditee`): rather than change
`Pawn` again, `audit::resolve` takes a `players: &[Player]` slice and
looks up the target pawn's owning color there. `Pawn` itself stays as-is.

`EffectAnchor` is defined here rather than in `context/play_context.rs` as
§4 shows it — see that section's implementation-status note.

```rust
impl Pawn {
    pub fn push_move(&mut self, record: MoveRecord, window: usize) { todo!() }
    pub fn auditable_moves(&self) -> impl Iterator<Item = (usize, &MoveRecord)> { todo!() }

    /// Clears `persistent_effects`, moves `position` to `yard_slot`.
    /// Deliberately does *not* touch `history` — those records' cards stay
    /// attached and dormant until one of the two paths below.
    pub fn capture_to(&mut self, yard_slot: SpaceId) { todo!() }

    /// Called when this pawn's first move *out* of the yard resolves.
    /// Every still-attached record is treated exactly like a natural
    /// age-out at this point — its cards go to the owner's reserve (§10).
    pub fn clear_history_on_exit(&mut self) { todo!() }

    /// The early-cashout alternative to waiting for `clear_history_on_exit`:
    /// the owner may collect a captured pawn's attached cards straight to
    /// hand now, at the cost of losing that pawn's reinstatement
    /// eligibility — there's nothing left to revert it to afterward.
    pub fn collect_early_forfeiting_reinstatement(&mut self) -> Vec<CardKindId> { todo!() }

    pub fn revert_from(&mut self, index: usize) -> Vec<MoveRecord> { todo!() }
}
```

**Implementation status.** `impl Pawn` is fully built, with three
signature changes from what's shown above, all to avoid a panic where the
doc's `()` return type couldn't report one:
- `push_move` returns `Option<MoveRecord>` (the aged-out record, if any) —
  not `()`. Something has to route that record's `actual_cards` back to
  the owner's reserve, and silently dropping it would leak cards out of
  the closed economy described in §10.
- `clear_history_on_exit` returns `Vec<CardKindId>` (every drained
  record's `actual_cards`, flattened) — not `()`, for the same reason. It
  shares a private `drain_history_cards` helper with
  `collect_early_forfeiting_reinstatement`, since the two only differ in
  *when* they're called, not in what they do.
- `revert_from` treats `index >= history.len()` as "nothing to revert"
  (returns an empty `Vec` instead of letting `VecDeque::split_off` panic).
  `audit.rs` already validates the index before calling this, but `Pawn`
  is a public type and shouldn't trust an out-of-range index from every
  possible caller.

`auditable_moves` and `capture_to` match this section exactly (`capture_to`
also clears `claimed_effects`, which isn't shown above but follows the
same logic as `persistent_effects` — a captured pawn's outstanding claims
are moot once it's off the board).

`RuleConfig::captured_pawns_remain_auditable` governs whether
`auditable_moves()` results for a pawn currently sitting in the yard are
offered by `legal_actions` at all — the data's there either way; this only
controls whether it's actively usable as an audit target while parked.

**Additions from §16 step 7**, needed once `ShieldCard`/`StunTrapCard`
exercised the rest of this section for real:
- `persistent_effects()`/`claimed_effects()` (read-only slices) and
  `attach_persistent_effect`/`attach_claimed_effect` (push a new one) —
  `PlayContext` (§4) needs somewhere to actually store what it attaches.
- `clear_claimed_effects()` — see `claimed_effects`'s own doc comment
  above ("resolved... the moment `trigger_automatic_audit` tests them").
- `MoveRecord::is_a_lie()` — the multiset comparison described just above
  the original code block, as a method rather than a free function, so
  both `audit::resolve` (§9) and `InteractionContext::trigger_automatic_audit`
  (§4) can call it without needing to depend on each other.
- A `Reversion` struct and `pub fn revert(pawns: &mut [Pawn], topology:
  &BoardTopology, target_index: usize, move_index: usize,
  reinstate_captures: bool) -> Reversion` free function — the actual
  revert-and-reinstate mechanics (calling `revert_from` on the target,
  then working out and applying which captures get reinstated). Lives
  here, not in `audit.rs`, for the same reason `AuditOutcome` lives in
  `card/mod.rs`: `context` (via `trigger_automatic_audit`) needs this same
  mechanic too, and `context` can't depend on `audit` without a cycle
  (`card ──> context ──> audit ──> card`, per §1's graph). `audit::resolve`
  now calls this instead of duplicating the logic itself.

---

## 9. Auditing — `audit.rs`

```rust
pub enum AuditOutcome { LieCaught, ClaimWasTrue }

pub struct AuditRequest {
    pub auditor: PlayerId,
    pub target_pawn: PawnId,
    pub target_move_index: usize,
    /// Only populated under `PaymentSelectionMode::PayerChooses` for
    /// `audit_attempt_cost_selection` — the specific cards the auditor is
    /// spending up front, paid unconditionally the instant the request is
    /// applied (unlike the false-accusation cost, there's no outcome to
    /// wait on before deciding). Left empty under `RandomDraft`, where
    /// `apply()` selects them internally instead, and always empty when
    /// `audit_attempt_cost` is 0 (the default). Sent to whichever
    /// `CardDestination` `audit_attempt_cost_destination` specifies.
    pub attempt_cost_cards: Vec<CardKindId>,
}
```

`legal_actions` only offers `Audit` for a given move at all if the
auditor's hand can cover the worst case —
`audit_attempt_cost + false_accusation_card_cost` — since otherwise they
could end up owing more than they have left if the challenge turns out
wrong.

```rust
pub struct RevertOutcome {
    pub pawn: PawnId,
    pub reverted_to: SpaceId,
    pub cards_collected: Vec<CardKindId>,   // flattened actual_cards across every discarded record
    pub reinstated_captures: Vec<(PawnId, SpaceId)>,
}

pub enum AuditConsequence { FalseAccusation, LieCaught(RevertOutcome) }

pub struct AuditResolution {
    pub outcome: AuditOutcome,
    pub consequence: AuditConsequence,
    /// Independent of `outcome` — driven by the *actually played* card's
    /// `on_audited_as_played` hook (see `StunTrapCard`), so it can fire
    /// even when the accusation was wrong.
    pub forfeits_auditor_turn: bool,
}
```

A `MoveRecord` is judged a lie if its `claimed_cards` and `actual_cards`
differ as multisets — order doesn't matter, so claiming `[Take4, Double]`
but having played `[Double, Take1]` is not a lie. The audited record's
`reveal` is set to `Public` if it turns out `ClaimWasTrue` (it stays in
history, now provably honest); on `LieCaught` the record and everything
after it are simply gone from history, redistributed as described in §10.

**Implementation status.** Fully implemented (§16 step 6), with three
deviations from the above:

- `AuditOutcome` is defined in `card/mod.rs`, not here — `CardBehavior`'s
  `on_audited_as_*` hooks need it, and `audit ──> card` per §1's
  dependency graph means `card` can't depend on `audit`. Same fix as
  `EffectAnchor` moving to `pawn.rs`: the shared type moves to whichever
  module is lower in the graph among the ones that need it.
- `RevertOutcome.cards_collected` is split into `directly_audited_cards`
  and `swept_up_cards` instead of one flattened list. A single list loses
  exactly the distinction `RuleConfig::cascade_lie_rewards_destination`
  needs (whether the cascade's swept-up cards follow the directly-audited
  move to the auditor, or go to the pile instead) — with nothing marking
  where the directly-audited move's cards end and the swept-up ones
  begin, that rule could never be honored downstream. Which destination
  each list actually lands in is still the caller's decision, same as
  everything else deferred below.
- A new `AuditError` (`UnknownPawn`, `UnknownMoveIndex`, `UnknownAuditee`)
  lets the resolution function below report a bad `AuditRequest` as a
  `Result` instead of panicking.
- The actual revert-and-reinstate mechanics (originally a private helper
  here) moved to `pawn::revert` (§8, added in step 7) once
  `context::InteractionContext::trigger_automatic_audit` needed the exact
  same logic — `resolve` now just calls that and repackages the result as
  a `RevertOutcome`.

The doc above doesn't show an explicit resolution function — it only
describes the *behavior* narratively. That behavior lives in
`audit::resolve(request: &AuditRequest, catalog: &CardCatalog, topology:
&BoardTopology, rules: &RuleConfig, players: &[Player], pawns: &mut
[Pawn]) -> Result<AuditResolution, AuditError>`. `players` exists solely
to resolve the target pawn's `PlayerColor` into a `PlayerId` for
`AuditContext.auditee` — see §8's note on `Pawn.owner`.

Deliberately out of scope for `resolve`: paying `audit_attempt_cost` /
`false_accusation_card_cost`, and routing `RevertOutcome`'s cards to
hands/decks/the shared pile. Those touch multiple players' economies at
once — `GameState`'s job (§16 step 8), not this function's. `resolve`
only reports what happened to the audited pawn; a reinstated capture's
"unless it has since moved" check (GAME_DESIGN.md) is done by checking
whether the captured pawn's *current* position is still one of its own
`topology.yard_spaces(...)` — if it's moved on since, it's left alone.

---

## 10. Card economy — `deck.rs`

There is no discard pile. Treat the whole game — every player plus the
shared pile — as one closed loop: a card is always either in a hand, in a
personal reserve, attached to a pawn's history, or in the shared pile, and
it only ever moves between those, never out of the loop entirely.

```rust
pub struct DeckComposition {
    /// How many of each card kind a fresh personal deck starts with.
    /// Exact numbers are a balance/playtesting question, not an
    /// architecture one — this is just the shape that holds them.
    pub counts: Vec<(CardKindId, u8)>,
}

/// A player's personal reserve. A played card's only "away from hand"
/// state is being attached to a pawn's history (§8); it comes back here,
/// not to hand, once that history item resolves.
pub struct Deck {
    reserve: Vec<CardKindId>,
}

impl Deck {
    pub fn new(composition: &DeckComposition) -> Self { todo!() }

    /// Removes up to `count` cards for drawing into hand. Returns fewer
    /// than requested if the reserve is short — never an error, just
    /// however many are actually available.
    pub fn take(&mut self, count: u8, rng: &mut impl rand::Rng) -> Vec<CardKindId> { todo!() }

    /// Adds a card back — from an aged-out history item, or as overflow
    /// redirected from a hand already at `hand_hard_cap`. `bypass_cap` is
    /// set for aged-out returns specifically
    /// (`aged_out_exempt_from_deck_cap`); if the cap blocks it and
    /// `bypass_cap` is false, the card is handed back un-added so the
    /// caller can redirect it to `SharedPile`.
    pub fn give(&mut self, card: CardKindId, cap: u8, bypass_cap: bool) -> Option<CardKindId> { todo!() }

    pub fn len(&self) -> usize { self.reserve.len() }
}

/// The single cross-player pool — the only way cards move between players
/// other than a direct challenge outcome.
pub struct SharedPile { cards: Vec<CardKindId> }

impl SharedPile {
    pub fn new(seed: Vec<CardKindId>) -> Self { Self { cards: seed } }
    pub fn take(&mut self, count: u8, rng: &mut impl rand::Rng) -> Vec<CardKindId> { todo!() }
    pub fn add(&mut self, card: CardKindId) { self.cards.push(card); }
}
```

**Implementation status.** `Deck`/`SharedPile` are fully implemented (§16
step 5): both `take` methods draw uniformly at random without replacement
via `Vec::swap_remove`, sharing one private `take_random` helper. `rand`
0.10 splits `random_range` onto a separate `RngExt` trait, imported
alongside `Rng`. Both types also gained an `is_empty()` alongside `len()`
(clippy's `len_without_is_empty` convention) — not shown above but not a
behavioral change.

`Player` carries a `deck: Deck` field alongside `hand: Vec<CardKindId>`.
`GameState` carries a `shared_pile: SharedPile` (§13). `Player` itself
(§12) is now implemented as shown there — plain data, no methods.

**Full lifecycle of a card:**

1. **Game start** — exempt from every cap below, since this is the genesis
   of a player's economy, not an inflow into it: each player's `Deck` is
   built from `DeckComposition` to `starting_deck_size`, then
   `starting_hand_size` cards are drawn straight to hand, deliberately
   allowed to exceed `hand_soft_cap` as a one-time boost. `SharedPile` is
   seeded to `starting_pile_size`.
2. **End of turn**: if hand size is below `hand_soft_cap`, draw from the
   reserve up to it (fewer if the reserve is short). This is the only
   routine draw — there is no separate draw phase at the start of a turn.
3. **Play**: cards move from hand to being attached to the new
   `MoveRecord`. This is their location for as long as that history item
   is live, not a discard.
4. **A history item resolves**, one of three ways:
   - **Ages out** unaudited: cards return to the *owner's* reserve, exempt
     from `deck_cap` (`aged_out_exempt_from_deck_cap`) — an internal
     transfer, not an inflow.
   - **Caught by a deliberate audit**: cards go to the *auditor's* hand
     (checked against `hand_hard_cap`, overflowing to their reserve,
     checked against `deck_cap`, further overflowing to the pile). The
     audited move is always the *oldest* one reverted — everything from it
     up to the present gets swept up too, and older moves than that are
     never touched. If `cascade_lie_rewards_destination` is set, only the
     directly-audited move's own cards follow the path above; the newer,
     merely-swept-up moves' cards go straight to the pile instead.
   - **Caught by an automatic audit** (a bluffed Shield tested by a capture
     attempt): the revert still fully applies — position, any reinstated
     captures, all of it, since that's a fact about what happened, not a
     reward for whoever noticed. But by default
     (`automatic_audit_reward_destination`) the cards go to the shared pile,
     not the attacking player.
5. **Capture**: the victim's own currently-attached history isn't touched
   or taxed — it stays dormant until either `clear_history_on_exit` (same
   as a natural age-out, to the owner's reserve) or the owner voluntarily
   calls `collect_early_forfeiting_reinstatement` (straight to hand, at the
   cost of that pawn's reinstatement eligibility). Separately, the
   *capturing* player is granted `capture_reward_from_pile` cards from the
   shared pile — the main way the pile acts as a comeback mechanic for a
   player who's fallen behind, since aggression is what refills it.
6. **Pawn reaches Finish**: if `finished_pawn_dumps_history_destination`,
   whatever's still attached to that pawn's history redirects to the pile
   instead of ever reaching the owner's reserve — a deliberate cost for
   completing a pawn's journey, and also where a real tactical wrinkle
   lives: a player nearing Finish has a genuine reason to bluff with their
   *worst* remaining cards on the last few moves, since whatever's still
   attached when it crosses the line is forfeit either way.
7. **A challenge is submitted**: `audit_attempt_cost` cards leave the
   auditor's hand immediately, regardless of outcome, heading to whichever
   `CardDestination` `audit_attempt_cost_destination` specifies (the shared
   pile, by default). If the challenge then turns out wrong,
   `false_accusation_card_cost` further cards move from the auditor's hand
   to whichever `CardDestination` `false_accusation_destination` specifies
   (the auditee, by default) — subject to that recipient's own hand-cap →
   deck-cap → pile overflow chain, same as any other external inflow.

The state a player needs to track is three dynamic numbers
(`hand_soft_cap`, `hand_hard_cap`, `deck_cap` — each independently
upgradable by future cards, per §5) plus their current hand and reserve
contents. Presenting three numbers cleanly is a UI concern for the Unity
layer, not a Rust one, but worth flagging here since it came up directly in
design discussion.

**Running out** is handled by two independent gates, checked in order.

**First, `RuleConfig::cards_exhausted_behavior`** — checked purely on
whether a player's own hand *and* deck are both empty, regardless of
whether they could otherwise still act (say, via a dormant collectible
pawn in their yard). `Ignored` does nothing here, leaving the second gate
below to decide. `Eliminated(Frozen | Removed)` treats letting your own
liquid supply run out as a loss condition in its own right, on purpose
harsher than the second gate — if it eliminates a player, the second gate
never gets a chance to apply to them at all.

**Second, `RuleConfig::no_available_action_behavior`** — only reached by
players who survived the first gate (because it didn't apply to them, or
because `cards_exhausted_behavior` is `Ignored`). Checked on genuinely
having no legal action: an empty hand *and* no dormant cards collectible
from any pawn in the yard either.
- **`AutoSkip`**: they simply can't play that turn, and stay in the
  game — a capture or a lucky false accusation can still pull them back in.
- **`DrawCard(n)`**: `n` cards are drawn from the shared pile at the
  *start* of the turn (never an error if the pile is short — fewer than
  `n`, down to zero, is the same outcome as `AutoSkip` for whatever's still
  missing). A movement card among them can be played normally; another can
  be spent to cover `audit_attempt_cost` and attempt a challenge instead —
  which is exactly why `n` is worth setting above 1 if that cost is more
  than a single card, so the draw doesn't leave a player with just enough
  for one option but not the other. This check happens at the start of the
  turn rather than folded into the routine end-of-turn draw, because
  another player could capture one of this player's pawns in between
  turns, handing them a fresh collect-from-yard option that wasn't there a
  moment earlier.
- **`Eliminated(Frozen | Removed)`**: running out is itself a loss
  condition here too — the same `Frozen`/`Removed` handling as the first
  gate, since a player could reach this point either by exhausting their
  cards with `cards_exhausted_behavior` set to `Ignored`, or by having some
  cards but no dormant collectible pawn to make use of an empty hand.

Under either gate's `Frozen` case, any card flow that would otherwise land
in the eliminated player's hand or reserve redirects to the shared pile
instead, since they can no longer act to claim or use it — specifically, a
wrong accusation against a frozen pawn sends its forfeit to the pile
rather than the eliminated player, and capturing a frozen pawn sends its
dormant attached history straight to the pile immediately, rather than
waiting on an exit that will never happen. A frozen pawn otherwise stays
fully interactable for everyone else — still capturable, still auditable,
still shown on the board — it's only the eliminated player's own agency
and card claims that are gone.

---

## 11. Per-player views — `view.rs`

The single place hidden information gets redacted, so "can this player
legally know X" only has to be gotten right once. This is what an agent —
AI or, eventually, the Unity-side human — makes decisions from, never the
raw `GameState` directly.

```rust
pub struct GameView {
    pub rules: RuleConfig,
    pub players: Vec<PlayerPublicInfo>,
    pub my_id: PlayerId,
    pub my_hand: Vec<CardKindId>,
    pub pawns: Vec<PawnView>,
}

pub struct PlayerPublicInfo { pub id: PlayerId, pub color: PlayerColor, pub hand_size: usize, pub score: i32 }

pub struct PawnView {
    pub id: PawnId,
    pub owner: PlayerId,
    pub position: SpaceId,
    pub persistent_effects: Vec<Option<CardKindId>>,   // None = hidden from this viewer
    pub history: Vec<MoveRecordView>,
}

pub struct MoveRecordView {
    pub claimed_cards: Vec<CardKindId>,
    pub actual_cards: Option<Vec<CardKindId>>,   // Some only once `RevealScope::Public`
}
```

Which specific cards a player learns from a `CardsTransferred` or similar
event (as opposed to just the fact that a transfer happened) is a property
of how that event gets redacted per-viewer here, not of any lingering
state — this same redaction pattern will need to extend to the event log
itself, not just state snapshots, once there's a real per-player delivery
layer in Step 4/5.

---

## 12. Players as agents — `player.rs`, `agent/`, `driver.rs`

`Player` is plain data; decision-making is a separate trait, so a human, a
random bot, and a scripted-for-tests bot are all just different
implementors.

```rust
// player.rs
pub struct Player {
    pub id: PlayerId,
    pub color: PlayerColor,
    pub hand: Vec<CardKindId>,
    pub deck: Deck,
    pub score: i32,
}
```

```rust
// agent/mod.rs
pub trait PlayerAgent {
    fn choose_action(&mut self, view: &GameView, legal: &[TurnAction]) -> TurnAction;
}
```

`agent/random_agent.rs` and `agent/scripted_agent.rs` provide a real bot
and a deterministic one for golden-log tests, respectively. A `HumanAgent`
deliberately doesn't exist at this layer — real human input belongs to the
Unity bridge in Step 4/5; at the Rust-only stage, `ScriptedAgent` and
`RandomAgent` are what testing needs.

```rust
// driver.rs
/// Runs exactly one turn: zero or more audits (each possibly followed by a
/// forced ForfeitCard), then one turn-ending action — always a PlayCard,
/// and also an Audit if `RuleConfig::auditing_costs_turn` is set.
/// `apply()` itself doesn't know about "turns"; it just validates and
/// applies one action at a time. This loop is what decides when a turn is
/// over, based on which kind of action just landed.
pub fn play_one_turn(
    engine: &mut impl GameEngine, agents: &mut [Box<dyn PlayerAgent>],
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
        if would_end_turn && !matches!(engine.legal_actions(current).get(0), Some(TurnAction::ForfeitCard(_))) {
            return Ok(all_events);
        }
    }
}
```

---

## 13. Orchestration — `game.rs`

```rust
pub enum TurnAction {
    Audit(AuditRequest),
    /// Only ever legal when a forfeit is pending for you — the moment
    /// right after a `PaymentSelectionMode::PayerChooses` false accusation.
    /// While pending, `legal_actions` offers exactly one of these per card
    /// in hand and nothing else, and it takes as many submissions as
    /// `RuleConfig::false_accusation_card_cost` to clear.
    ForfeitCard(CardKindId),
    PlayCard(Declaration),
}

pub struct GameState {
    pub topology: BoardTopology,
    pub rules: RuleConfig,
    pub catalog: CardCatalog,
    pub players: Vec<Player>,
    pub pawns: Vec<Pawn>,
    pub shared_pile: SharedPile,
    pub current_player: PlayerId,
    audits_this_turn: u8,
    forfeited_next_turn: std::collections::HashSet<PlayerId>,
    /// Who still owes a forfeit, and how many cards remain — decremented
    /// by one per `ForfeitCard` submitted, removed once it reaches zero.
    pending_forfeit: Option<(PlayerId, u8)>,
    space_effects: std::collections::HashMap<SpaceId, Vec<PersistentEffectState>>,
}

pub trait GameEngine {
    fn legal_actions(&self, player: PlayerId) -> Vec<TurnAction>;
    fn apply(&mut self, action: TurnAction) -> Result<Vec<GameEvent>, GameError>;
    fn view_for(&self, player: PlayerId) -> GameView;
    fn current_player(&self) -> PlayerId;
}
```

```rust
// event.rs
pub enum GameEvent {
    PawnMoved { pawn: PawnId, from: SpaceId, to: SpaceId },
    PawnCaptured { pawn: PawnId, by: PawnId },
    CardConsumed { player: PlayerId },
    PersistentEffectRevealed { pawn: PawnId, card: CardKindId, was_real: bool },
    AuditResolved { request: AuditRequest, outcome: AuditOutcome },
    CardsTransferred { from: PlayerId, to: PlayerId, cards: Vec<CardKindId> },  // redacted per-viewer, see §11
    /// Pile contents are public — there's no bluffing angle to hide there
    /// — but *drawing from* it into a specific hand is redacted the same
    /// way a normal draw would be: the count is public, the specific cards
    /// aren't, to protect the drawing player's hand privacy.
    CardsGrantedFromPile { player: PlayerId, count: usize },
    CardsEnteredPile { cards: Vec<CardKindId>, source: PileSource },
    TurnForfeited { player: PlayerId },
    PlayerEliminated { player: PlayerId },   // only reachable under NoAvailableActionBehavior::Eliminated
    PlayerWon { player: PlayerId },
}

pub enum PileSource { AgedOutOverflow, CapturedPawnFinished, CascadedAuditSpoils, AutomaticAuditSpoils }
```

**Implementation status.** `GameState`/`GameEngine`/`GameError` are implemented
(§16 step 8), with several deviations:

- `TurnAction::PlayCard` wraps `PlayedCard`, not a bare `Declaration`. A
  bare `Declaration` only carries the claim — with no way to say what was
  truly played, bluffing (the entire point of this game) could never
  reach the engine at all. `play::PlayedCard` exists specifically to pair
  a claim with the truth, so `PlayCard` wraps that instead.
- `pending_forfeit` is `Option<PendingForfeit>` (a private struct: `owed_by:
  PlayerId, target: PaymentTarget, remaining: u8`), not `Option<(PlayerId,
  u8)>`. The tuple has no way to say *who receives* the forfeited cards —
  `false_accusation_destination` can be `SharedPile` or `Auditee`, and
  once a `PayerChooses` forfeit is set up, that destination has to survive
  across however many separate `ForfeitCard` submissions it takes to
  clear. `PaymentTarget` (`SharedPile` or `Player(PlayerId)`) resolves the
  destination once, up front.
- `GameEvent::AuditResolved` carries `auditor: PlayerId, target_pawn:
  PawnId, target_move_index: usize, outcome: AuditOutcome` instead of a
  full `AuditRequest` — referencing `audit::AuditRequest` from `event.rs`
  would create a cycle (`event ──> audit ──> card ──> context ──> event`,
  since `context` already depends on `event`), so the fields actually
  needed are inlined instead (see `event.rs`'s own doc comment on the
  variant). `PileSource` gained three variants
  (`AuditAttemptCostOverflow`, `FalseAccusationOverflow`,
  `GrantBounceback`) not shown above, for overflow points this section's
  card-payment logic actually has that the original four don't cover —
  see `event.rs`'s own doc comments on each.
- `legal_actions`'s `PlayCard` enumeration only offers *honest* plays
  (claimed == actual) — enumerating every possible *lie* isn't bounded
  (a claim can name any card in the catalog, not just what's in hand), so
  there's no sensible "legal claims" menu to build. An agent that wants to
  bluff takes one of these honest baselines and swaps its
  `declaration.claimed_cards` before submitting — `apply` only validates
  that `actual_cards` are genuinely in hand and that counts/categories are
  within `RuleConfig`'s limits; it never checks the claim against hand
  contents, since a claim isn't supposed to be checkable that way.
- Not implemented at all: `RuleConfig::cards_exhausted_behavior` /
  `no_available_action_behavior` (§3, §10) — the elimination/lifeline
  gates for a player running out of cards. This is a real, acknowledged
  gap, not an oversight rediscovered later: the rest of the turn loop
  (`apply`, `legal_actions`, card payments/overflow, capture rewards, turn
  advancement including `StunTrapCard`'s skip) is implemented and tested,
  but running-out handling is deferred as its own self-contained follow-up
  given this step's already-large scope.
- A reinstated capture's position change (from `audit::resolve`'s
  `RevertOutcome`) is applied but not separately logged — `GameEvent` has
  no variant describing "a pawn was reinstated," and adding one felt like
  scope creep against the size this step already reached. The state
  change itself is correct; only the event log doesn't narrate it.

---

## 14. A turn, step by step

**Scenario A — a multi-card bluff, matching card counts (the default):**

1. Blue plays `Declaration { pawn: P3, claimed_cards: [TAKE_4, DOUBLE_2] }`
   (claiming 8 total) — but actually consumes `[TAKE_1, SHIELD]`. Same
   count, 2-for-2, satisfying `allow_card_count_mismatch = false`; the lie
   is entirely in the identities. A modest real move, dressed up as an
   aggressive double-move claim, with a real Shield quietly attached on
   the side.
2. `apply()` iterates `claimed_cards`, feeding a shared `MovementProposal`:
   `TAKE_4.on_claimed` sets `steps = 4`; `DOUBLE_2.on_claimed` sets
   `multiplier = 2`. One `resolve_movement` call moves P3 eight spaces —
   the board shows 8, even though only 1 was real.
3. `MoveRecord` records `claimed_cards: [TAKE_4, DOUBLE_2]`,
   `actual_cards: [TAKE_1, SHIELD]`.
4. Red later audits this move: multisets differ entirely — `LieCaught`.
   Both `[TAKE_1, SHIELD]` move to Red's hand, including the Shield Blue
   never publicly admitted to having played at all.

**Scenario B — Shield, the independent-dispatch model:**

1. Green truthfully plays Shield on P1 — `on_played` attaches a real
   effect anchored to P1, and (since it was also truthfully claimed) an
   outstanding claimed effect too.
2. Yellow's move attempts to land on P1. `attempt_capture` independently
   checks both lists: the real one exists, so it dispatches
   `on_capture_attempted_as_played`, which triggers the automatic audit
   (finds claim and actual match — `ClaimWasTrue`, no penalty to anyone)
   and returns `Blocked`. The claimed one also exists, so
   `on_capture_attempted_as_claimed` fires too — but its own audit trigger
   is a no-op, since the played hook already resolved it.
3. Contrast: if Green had only *claimed* Shield without playing it, step 2
   finds nothing in the real list (so `on_capture_attempted_as_played`
   never fires at all), but the claimed one still exists — its hook fires,
   triggers the automatic audit (claim with no matching actual —
   `LieCaught`, Yellow stands in as auditor, cards go to the shared pile
   per `automatic_audit_reward_destination`), and returns `Proceeds`. The
   capture goes through.

**Scenario C — a false accusation, under each `PaymentSelectionMode`:**

1. Yellow audits a move that turns out to have been truthful —
   `outcome: ClaimWasTrue`, `consequence: FalseAccusation`, with
   `false_accusation_card_cost` set to 2 for this game.
2. Under `RandomDraft`: `apply()` picks two random cards straight out
   of Yellow's hand and hands them to the auditee, all inside the same
   call. `pending_forfeit` never gets set; Yellow's turn continues exactly
   as if nothing else happened.
3. Under `PayerChooses`: `apply()` sets
   `pending_forfeit = Some((Yellow, 2))`. `legal_actions(Yellow)` now
   returns only `ForfeitCard(c)` for each `c` in Yellow's hand — no
   `Audit`, no `PlayCard` — until it's cleared. Yellow submits
   `ForfeitCard` twice, decrementing the pending count each time; once it
   reaches zero, `pending_forfeit` clears and normal options return.

---

## 15. Testing strategy

- **`board.rs`** — graph traversal correctness, including yard-exit edges
  specifically, since there's no separate entry-point field to fall back
  on if one is missing.
- **`card/`** — one test per `CardBehavior` impl via a mock context;
  multi-card combination tests (steps, multiplier, and capture mode all
  landing correctly from separately-played cards).
- **`pawn.rs`** — history survives a capture and is still auditable in-yard
  (when the rule allows it); clears correctly on the *next* exit, not the
  capture itself.
- **`audit.rs`** — cascading revert, multiset lie-detection, window
  boundaries, `StunTrapCard` forfeiting regardless of outcome.
- **`deck.rs`** — cap enforcement at every overflow point (hand soft → hard
  → deck → pile), `aged_out_exempt_from_deck_cap`'s bypass behaving
  correctly, `SharedPile::take` never fabricating cards it doesn't hold.
- **`game.rs` / `driver.rs`** — golden-event-log scenarios via
  `ScriptedAgent`, asserting the exact `Vec<GameEvent>` for a fixed action
  sequence. These double as acceptance tests for the eventual C# port:
  replaying the same fixture against the port and diffing event logs is
  how that translation gets verified.
- `RandomAgent` vs. `RandomAgent` self-play, run in bulk, is a cheap fuzz
  test for "does the engine ever panic or reach an inconsistent state,"
  independent of testing any specific rule.

---

## 16. Suggested build order

1. `board.rs` + tests — graph traversal correctness first.
2. `rules.rs`, `card/mod.rs` skeleton (category, catalog, trait — no
   concrete cards yet).
3. `context/` skeleton, then `movement.rs`.
4. `MoveCard`, `DoubleModifierCard`, `RampageModifierCard` against
   `context/` — get multi-card bluffing working end-to-end before Shield.
5. `deck.rs` + `player.rs` wiring — the economy underneath everything.
6. `pawn.rs`'s history/revert + `audit.rs` — the bluffing core.
7. `ShieldCard` and `StunTrapCard` last, once the plumbing is solid.
8. `view.rs`, `agent/`, `driver.rs`, `game.rs` — then golden scenario tests.

---
