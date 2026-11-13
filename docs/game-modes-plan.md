# Game modes: plan

Status: planned, not started (2026-11-13). Private fork work: nothing here goes upstream or into a
public build until the Roomtone PR's reception decides between more PRs and a fork.

The park builder is a sandbox today. This plan adds game modes on top of it, opponents to play
against, and multiplayer. Everything targets the clean tier first: it is what a store build would
run, its engine is ours to change (github.com/rhythmerc/skatecore), and its skater is animated from
the engine's state alone, so another player can be drawn from their state without their inputs.

## What there is to build on

- The clean engine's events: pop, trick (with its name), land, bail, grind and grind end, a banked
  combo score, a new best, push, respawn. Game modes are made from these.
- The clean tier's scoring and the trick readout (`xr::ui::hud`).
- Build mode: placed pieces with ids, colours (`Tint`) and a saved layout (`ar-layout.json`, plain
  JSON, so parks can be shared).
- The Skate 3 tier's free-skate multiplayer (`multiplayer.rs`, skate-net: ten players, each
  simulating themselves, direct UDP or Steam). It doesn't reach the clean tier and Steam doesn't run
  on Quest, but its shape (state sync, not lockstep) is the one to follow.

## Modes

| Mode | Rules |
|---|---|
| **Trick Attack** | A fixed time (two minutes); highest total score wins. Solo against your best, or a shared scoreboard. |
| **Gaps** | Air from one placed piece to another is a named gap ("Kicker to Ledge"), found by skating and scored. |
| **HORSE / S.K.A.T.E.** | The setter lands a combo and sets its **score**, not its tricks. The others must land a combo scoring at least that much, or take a letter. A setter who bails passes the turn. Spell the word and you're out. |
| **Graffiti** | **Tricking on a piece claims it** in your colour, with no comparison to anyone else's trick: the last player to trick on it owns it. Most pieces owned when the time runs out wins. |
| **King of the Hill** | There is **one crown and no zone**. It starts somewhere in the park; the first player to reach it grabs it. Whoever holds it scores for the time they hold it until another player steals it, and so on back and forth. Most time held wins. |
| **Spot challenges** | In build mode a challenge can be attached to a piece ("boardslide this rail", "gap it"); a shared park carries its challenges. |
| **Room letters** | S-K-A-T-E letters put on the real room's tables, couch and shelves from the room scan; collect them in one session. AR only, solo. |

### Open questions

- **HORSE:** must the answering combo start on the same piece, or anywhere? Is there a time limit
  per attempt? Does one bail on the answer cost the letter, or is there a second try?
- **Graffiti:** what counts as tricking on a piece: a grind or stall on it, a trick landed on it, air
  off it? Does anything in the air above a piece count?
- **King of the Hill:** how the crown is stolen: touching the holder, landing a trick near them, or
  outscoring them while close? Does a bailing holder drop it where they fell?
- Shared: round lengths, and how each mode's state shows in the headset (the trick readout, or a
  scoreboard panel).

## Opponents

1. **Ghosts.** A run recorded as the skater's state every frame and played back as a translucent
   skater. Positions are replayed, not inputs re-simulated, so a ghost recorded on one device plays
   on any other. Ghosts give "beat your friend's run" without netcode, show a setter's combo in
   HORSE, and are the fallback opponent in every mode.
2. **A bot.** It drives the engine through the same input a player does: picks a piece, lines up,
   pushes, pops at the lip, does a trick from the engine's trick table, lands. Difficulty comes from
   its trick choices and deliberate mistiming. The engine is cheap to step, so the bot can try
   candidate inputs ahead before committing. Clean tier only; a bot for the Skate 3 engine is out of
   scope.

## Multiplayer

- **Same room:** two headsets, one shared park on the same table, through Horizon OS colocation and
  shared spatial anchors so every player sees each piece in the same real place. To check first:
  what anchor sharing requires (parts of it may need Meta's platform SDK and a store entitlement).
- **Remote:** each player's park is anchored in their own room, so only the placed pieces are
  common (everyone's furniture differs). Suits the turn-based modes (HORSE, ghosts, challenges)
  better than the live ones.
- **Netcode:** each player simulates themselves and sends their state (position, rotations, mode,
  trick, score) about 20-30 times a second; others are drawn from it with the same animation code.
  Game state (letters, owners, the crown) has one authority, the host.

## Order

1. A mode framework (start, timer, results, a scoreboard panel), then Trick Attack and Gaps, solo.
2. Ghosts: recording, playback, saving and sharing them.
3. HORSE: pass-the-headset and against ghosts.
4. The bot.
5. Same-room multiplayer, then Graffiti and King of the Hill on it.
6. Remote multiplayer, spot challenges and park sharing; room letters whenever convenient.
