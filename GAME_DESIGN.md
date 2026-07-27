# Game Design — Aap uit de mouw

This document describes the game itself: the rules, mechanics, and player
experience. No code, no data structures — just what a player would need to
know to play, and what a designer or engineer would need to know to build
it faithfully. For how the Rust implementation is structured, see
[ARCHITECTURE.md](./ARCHITECTURE.md).

## What this game is

A digital board game combining *Ludo* / *Mens erger je niet* with
card-driven movement, bluffing, and a deduction-style auditing mechanic —
players don't just race pawns around a board, they also have to decide
whether to trust what their opponents claim about their own moves.

The eventual target platform is Unity (C#); this repository currently
implements only the core game logic in Rust, deliberately decoupled from
any engine or UI, per the project's phased plan (see `CLAUDE.md`).

## Base game

The board is a Ludo / *Mens erger je niet* hybrid: each player has a set
number of pawns starting in a private home yard, a shared track that winds
around the board, and a private "home lane" leading to that player's
finish. Which specific rules from either tradition apply — blockades from
two stacked pawns, whether capturing sends a pawn all the way back to the
yard, whether a pawn needs an exact roll to finish, bonus turns for
capturing or leaving the yard, and so on — is independently configurable
per game rather than fixed to one tradition or the other.

The board itself is built to support far more than a fixed four-player
ring: player count, pawns per player, route length, and even asymmetric
starting positions are all just configuration, with room for future ideas
like branching paths or special event spaces.

## Cards instead of dice

Movement is driven by cards instead of a die roll — a card might simply
read "Take 3 steps." Cards are played face-down along with a spoken claim
of what was played, and that claim does not have to be true: a player may
covertly play "Take 1 step" while claiming "Take 4 steps." The board always
reflects the *claim*, not the hidden reality — that's what makes bluffing
meaningful, since the claimed effect is what visibly happens.

A single turn can combine more than one card at once — for instance,
claiming a "Take 4" together with a "Double" for a claimed total of 8. By
default, the number of cards actually played must match the number
claimed, even though their identities can differ freely; a ruleset can
relax this if it wants mismatched counts to be part of the bluff too.

## Auditing

On your own turn, you may challenge a recent move made by another player,
instead of or alongside playing your own card, at some card cost (zero, by
default) paid up front regardless of how the challenge turns out. Only a
bounded number of a pawn's most recent moves remain challengeable at all —
older ones aren't auditable any more, though a pawn's auditable recent
history remains visible on the board (shown as "ghost" markers at its past
few positions) so players can decide what's worth challenging (although
this visual aspect is out of scope for this core Rust implementation).

- **If the challenge catches a lie**: the offending pawn is rewound all the
  way back to its position before that move, undoing everything it did
  since — including reinstating any pawns it captured along the way,
  unless one of those pawns has since moved under its own power, in which
  case it simply stays where it currently is. The challenger collects the
  real cards involved as a reward.
- **If the challenge is wrong** (the move was actually honest): the
  challenger owes some further number of cards (one, by default) to the
  player they wrongly accused, on top of whatever they already paid to
  attempt it — either the challenger's own choice, or a blind pull from
  their hand, depending on the ruleset. Nothing else happens; the honest
  move stands. The audited cards remain visible thereafter.

By default, auditing doesn't cost you your own turn — you can challenge a
move and still play a card afterward, up to a configurable number of
challenges per turn — though a stricter ruleset can make a challenge
consume the whole turn instead.

## Cards with extra effects

Beyond simple movement, some cards do more:
- **Double** a move's steps, when played alongside a movement card.
- **Rampage** — captures every pawn along the *entire* path of a move, not
  just the square it lands on.
- **Shield** — protects a pawn from capture. If genuinely played, it blocks
  the next attempted capture and is revealed to everyone in the process. If
  only *claimed* without really being played, the bluff is exposed the
  moment someone tries to capture through it anyway — an automatic,
  no-extra-cost check distinct from a deliberately spent audit.
- **Stun Trap** — a card built to punish curiosity: challenging a move
  where this card was the one truly played costs the challenger their next
  turn outright, regardless of whether the challenge also happened to
  catch a lie.

More cards are expected over time, including ones that reward a player
directly for capturing, and ones that permanently grow how many cards a
player can hold.

## The card economy

Each player has their own personal hand and reserve (deck) — there is no
shared draw pile between players. At the end of a turn, a player draws
back up to a target hand size from their own reserve. Playing a card
doesn't discard it in the usual sense: it stays tied to whichever pawn used
it, as part of that pawn's recent move history, until that history item is
eventually resolved one way or another — caught in a challenge, or safely
aging past the audit window and returning quietly to its owner's reserve.

A single shared communal pile also exists, seeded at the start of the
game, and it's the *only* way new cards ever enter an otherwise closed
economy. It's fed by a pawn completing its full journey to the finish (a
deliberate small cost on whatever cards are still tied up in its recent
history at that moment), by the cards left over when a big lie's
punishment cascades further back than the single move that was actually
challenged, by the rarer case of exposing a bluffed Shield (since whoever
stumbles into that didn't deliberately risk anything the way a real
challenger does, so they don't collect the spoils either), and by whatever
overflows a player's hand and reserve when both are already full. It's
drawn from by capturing an opponent's pawn, which rewards the capturer
directly from the pile — making the pile a natural comeback mechanic for a
player who's fallen behind, since aggression is what refills it.

Hand and reserve sizes are capped — hands have both a soft target (drawn up
to, routinely) and a hard ceiling (which only external windfalls can ever
threaten), and the reserve has its own ceiling. A windfall that would
overflow a full hand spills into the player's own reserve instead; one that
overflows a full reserve spills into the shared pile.

A captured pawn's history isn't simply lost — the cards tied to its recent
moves sit dormant until either the pawn leaves the yard again (returning
them to its owner's reserve as normal) or its owner chooses to cash them in
immediately, at the cost of giving up any chance of that pawn later being
reinstated if the capture turns out to have been based on a lie.

If a player starts their turn with an empty hand and no such dormant
cards to cash in from any pawn sitting in their yard — truly no other
option — what happens next is itself a ruleset choice: they might simply
be unable to play that turn and stay in the game waiting for fortune to
bring them a card again; they might be given one or more lifeline cards
straight from the shared pile, usable either to move or as the stake for
an audit; or, in the strictest ruleset, running dry like this is itself a
loss condition. A separate, harsher rule can also treat a player's own
hand and reserve both being empty as a loss condition outright, even if
they could technically still act through a captured pawn waiting in their
yard — letting your own supply run out being its own kind of mistake,
independent of whether a workaround happened to exist.

## Philosophy

Nearly everything above — which traditional rules apply, exact costs and
rewards, cap sizes, card counts — is deliberately left tunable rather than
fixed, so the balance of the game can be adjusted without touching code.

## Every currently configurable rule
 
The sections above describe the concepts; this is the full list of knobs,
grouped the same way. Names are given as they appear in code
(`RuleConfig`'s fields in `ARCHITECTURE.md` §3) since that's the precise
anchor if you want to go find or change one.
 
**Base game mechanics**
- `pawn_count` — how many pawns each player controls.
- `exit_rule` — whether pawns start already in play, or a specific card is
  required to bring one out of the yard.
- `blockades_enabled` — the *Mens erger je niet* rule that two of your own
  pawns stacked on one space form a wall nothing can pass.
- `capture_sends_to_yard` — whether a captured pawn goes all the way back
  to its yard.
- `bonus_turn_on_capture` — an extra turn for capturing an opponent's pawn.
- `bonus_turn_on_exit` — an extra turn for bringing a new pawn into play.
- `exact_count_to_finish` — whether a pawn must land exactly on the final
  space, or overshooting is fine.
**Auditing**
- `audit_window` — how many of a pawn's most recent moves stay
  challengeable at all.
- `max_audits_per_turn` — how many challenges a player may make in one
  turn.
- `revert_captures_on_lie` — whether captures a caught liar made along the
  way get undone too.
- `audit_attempt_cost` — cards paid just to submit a challenge, win or
  lose.
- `audit_attempt_cost_destination` — whether that upfront cost goes to the
  shared pile or straight to the player being challenged.
- `audit_attempt_cost_selection` — whether the challenger picks which of
  their own cards pay that cost, or it's drafted at random.
- `false_accusation_card_cost` — the further cost paid only if the
  challenge turns out to be wrong.
- `false_accusation_destination` — where that wrong-accusation cost goes
  (shared pile or the wrongly-accused player).
- `false_accusation_selection` — the same choice-vs-random-draft question,
  for the wrong-accusation cost.
- `auditing_costs_turn` — whether making a challenge uses up your whole
  turn, or you can still play a card afterward.
- `captured_pawns_remain_auditable` — whether a captured pawn's old moves
  can still be challenged while it waits in the yard.
- `reveal_collected_cards_publicly` — whether everyone learns exactly
  which cards changed hands after a challenge, or just the two players
  involved. (the audited cards themselves will always be shown publicly)
**Playing cards**
- `playing_card_mandatory` — whether you must play a card on your turn if
  you're able to.
- `max_cards_per_play` — the most cards you may combine into a single
  play.
- `max_cards_per_category_per_play` — finer-grained limits per card
  category within one play (e.g. at most one Movement card, at most one
  Modifier). This may be a different number per category.
- `allow_card_count_mismatch` — whether you may claim a different
  *number* of cards than you actually played, not just different
  identities.
**The card economy**
- `starting_deck_size` — how many cards each player's personal reserve
  starts with.
- `starting_hand_size` — how many of those are dealt straight into a
  player's hand at the very start.
- `hand_soft_cap` — the hand size a player draws back up to at the end of
  their turn.
- `hand_hard_cap` — the absolute ceiling a windfall can't push a hand past.
- `deck_cap` — the ceiling on a player's own reserve.
- `aged_out_exempt_from_deck_cap` — whether a pawn's safely-aged-out cards
  can push a reserve over its cap, since they're returning home rather
  than arriving from outside.
- `starting_pile_size` — how many cards seed the shared pile at the start
  of the game.
- `capture_reward_from_pile` — cards granted to a player for successfully
  capturing an opponent's pawn.
- `automatic_audit_reward_destination` — whether cards from an
  automatically-caught bluff (like a fake Shield) go to the pile instead
  of whoever happened to stumble onto it.
- `finished_pawn_dumps_history_destination` — whether a pawn reaching Finish
  sends whatever cards are still tied to its recent history into the
  pile, as a cost of completing its journey.
- `cascade_lie_rewards_destination` — whether a big caught lie's spoils are
  split (the directly-challenged move to the challenger, everything swept
  up along with it to the pile instead) rather than all going to the
  challenger.
**Running out of cards**
- `cards_exhausted_behavior` — what happens, independent of anything
  else, the instant a player's hand and reserve are both completely
  empty: ignored (nothing special), or an elimination that either freezes
  that player's pawns in place or removes them from the board entirely.
- `no_available_action_behavior` — what happens to a player who — having
  survived the check above — starts their turn with no legal action at
  all (empty hand, nothing collectible from the yard either): they simply
  skip the turn; they're handed one or more lifeline cards from the
  shared pile to work with; or it's treated as elimination, same
  freeze-or-remove choice as above.
---
