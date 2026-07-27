# CLAUDE.md

Guidance for Claude Code working in this repository.

## What this project is

A digital board game: *Ludo* / *Mens erger je niet*, with dice replaced by
cards, bluffing about which card was played, and a deduction-style
auditing mechanic. Full rules: [`GAME_DESIGN.md`](./GAME_DESIGN.md).
Full technical design: [`ARCHITECTURE.md`](./ARCHITECTURE.md). Read both
before writing code — this file is a steering summary, not a replacement
for either.

**Current phase**: core game logic only, in Rust (`aap_uit_de_mouw_core`), with no
dependency on Unity, C#, or any UI. Thorough test coverage of the rules is
part of this phase, not a later cleanup step. Translating to C# and
building Unity bridge/presentation layers are future, separate work — see
`ARCHITECTURE.md`'s "Scope and context" for how that constrains choices
made now (favor plain data and portable patterns; keep the `GameEvent` log
complete and replayable; keep hidden-information redaction centralized).

## Working style

- **Follow the build order in `ARCHITECTURE.md` §16.** Implement and test
  one module at a time rather than attempting the whole crate in one pass.
  Each module has a natural stopping point with its own tests; stop there,
  make sure `cargo test` and `cargo clippy` are clean, and confirm before
  moving to the next.
- **Default to making things a `RuleConfig` gamerule.** This is a
  deliberate project-wide bias — see `ARCHITECTURE.md`'s "Engineering
  philosophy" section. If you hit a judgment call this document doesn't
  cover, lean toward a new `RuleConfig` field over a hardcoded choice.
- **The `///` comments inside `ARCHITECTURE.md` are design narration, not
  literal rustdoc.** Don't copy them verbatim into the implementation —
  see the dedicated section in that document for how to translate them
  into real doc comments vs. plain `//` comments.
- When a design doc and a concrete implementation need diverge (you find
  a genuine bug or gap while coding), fix the code correctly and flag the
  discrepancy rather than silently working around it — these docs should
  stay accurate as the source of truth.

## Coding standards

- **Idiomatic, maintainable Rust.** Prefer the standard library and
  well-established patterns over cleverness. Small, focused modules
  matching the layout in `ARCHITECTURE.md` §1.
- **Doc comments (`///`) describe the public contract, written for
  whoever calls that item** — what it does, what it returns, what to
  expect at the boundary. They do not explain *why* something is built the
  way it is, alternatives considered, or internal mechanics — that's what
  plain `//` comments near the relevant code are for. If you're tempted to
  write "this is because..." in a `///` comment, it almost certainly
  belongs as a `//` comment instead, or doesn't need to be written down
  outside this design doc at all.
- **Thoroughly tested.** Unit tests per module (see `ARCHITECTURE.md` §15
  for what each module's tests should cover), plus scripted-agent
  golden-log scenario tests for full turn sequences. These golden logs
  will double as acceptance tests for the eventual C# port — keep that in
  mind when deciding what a test should assert on (the full event
  sequence, not just a final state).
- **Errors**: use `thiserror` for error enums (`GameError`, `MoveError`,
  etc. from `ARCHITECTURE.md`) rather than hand-rolling `Display`/`Error`
  impls.
- **`RuleConfig`'s builder**: use `bon`'s derive macro rather than a
  hand-written builder, given how many fields it has.
- **Weigh every other dependency critically.** A new crate needs to earn
  its place — prefer std or a small amount of extra code over a dependency
  that only saves a little boilerplate. But prefer a mature dependency over
  a large amount of extra code. Add dependencies with `cargo add <name>`
  rather than hand-typing versions into `Cargo.toml`.
- Lints are already configured in `Cargo.toml`'s `[lints]` table
  (`clippy::all` at `warn`, `unsafe_code` forbidden) so they show up live
  in-editor. Before considering any module done, additionally run
  `cargo fmt`, `cargo clippy --all-targets -- -D warnings` (hard failure on
  anything the table would otherwise just warn about), and `cargo test`.

## Repository layout

```
Cargo.toml        # package manifest + [lints] table — already set up
rustfmt.toml      # minimal; just pins the edition
.gitignore
src/              # see ARCHITECTURE.md §1 for the full module tree — create
                  # this scaffold first if it doesn't exist yet
GAME_DESIGN.md
ARCHITECTURE.md
CLAUDE.md
```
