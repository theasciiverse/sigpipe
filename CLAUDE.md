# SIGPIPE

Asciiverse pipe-laying race against the flow. Rust + Ratatui (+ half-blocks for the
liquid). A spiritual successor to Pipe Mania (The Assembly Line, Amiga, 1989; ported
by Lucasfilm Games as *Pipe Dream*): lay pipe pieces from a queue onto a grid before
the flow arrives, and keep it going as long as you can. Mechanics free, everything
else our own.

**Status** (2026-10-05): first playable cut. Classic rules, the Unix data-stream
theme (chosen by David on 2026-10-05), endless levels that tighten, locked cells,
the `nul` sink, buffers and valves, synthesised sound, versus over iroh on the
LAN, keyboard, mouse and gamepad, session logging, a saved hi score.
The rules and numbers below are a first proposal for David to play and tune; not
yet approved. Own git repo, private at github.com/theasciiverse/sigpipe.

```bash
cargo run --release                        # play (--level N skips the title, --mute)
RUST_LOG=debug cargo run --release 2>sigpipe.log
cargo test
# headless: play SECS seconds, print the screen (or --cells | tools/shot.py as a PNG)
cargo run -- --shot 45 --seed 7 --deal "+7JL|F--" --moves "PRPDPLPUUPUPRPRP"
cargo run -- --shot 45 --seed 7 --deal "+7JL|F--" --moves "PRPDPLPUUPUPRPRP" --cells | python3 tools/shot.py out.png
```

Code: `src/engine.rs` (board, pieces, flow, scoring, level numbers, tests),
`src/render.rs` (tile art, half-block data, HUD, title, lobby), `src/input.rs`,
`src/audio.rs`, `src/versus.rs` (the match), `src/proto.rs`, `src/net.rs` and
`src/lan.rs` (iroh and LAN discovery, from Panzer), `src/main.rs` (screens, the
loop, headless shots).

## First cut, as built (2026-10-05)

- **Board** 10 by 7, each cell a 6 by 3 tile (square on screen); fits 80x24.
  Walls are the double box-drawing set, the inside is half-block pixels, 6 by 6 per
  tile, lit as the data front passes. Five pieces queued, next on top; the cursor
  shows the next piece as a ghost.
- **Theme**: the source is a process box labelled with a command (`cat`, `yes`,
  `seq`, ...); the data is green with a ripple running along the stream. Pause is
  `[1]+ Stopped` (P: fg, Esc: kill %1); losing is `cat: write error: Broken pipe`,
  `[exit 141]`; passing is `[exit 0]`. The spill pours out green if the target was
  met, red if not.
- **Rules**: lay or replace pieces until the countdown ends (replacing costs 50 and
  locks laying for 0.45 s). Then the data runs through what you laid. The
  **crossover** is the 1989 original's: drawn as a bridge (the vertical pipe on
  top, the horizontal one passing under), it carries data straight on, each way
  once, and pays +500 when the data loops back through it the other way. The
  **tee** (`┬ ┴ ├ ┤`, ours, in the bag from level 2) splits the data into both
  its other arms at once, and from there it floods like water wherever a pathway
  is open; branches that meet just join. The first time any of it reaches an
  opening that leads nowhere (the edge, an empty or locked cell, a piece that
  doesn't open back) the pipe is broken. Pipe `required` segments by then and the
  level passes (`[exit 0]`). Seal every end (loops, `nul`) and it ends cleanly
  with no spill, passing if long enough, otherwise `short write`. F toggles fast
  flow (and starts the data at once during the countdown).
- How we got here (2026-10-05): the first cut had the classic crossover, drawn
  as an open `╬`-like joint, which David read as a junction ("the water only
  flows linearly"). Next came full flooding everywhere ("flow in all directions
  like real water, whenever there is an open pathway"). Then, after checking that
  the original's flooz is a single stream through a crossover (Wikipedia; Pipe
  Dream reviews), David picked crossover plus tee: both mechanics, each piece
  honest about what it does.
- **Board specials**, fixed in place, amber like the process boxes: **buffers**
  (`buf`, from level 4, two from 7), straights the data takes four segments'
  time to fill, +300; **valves** (from level 5, two from 8), straights with
  arrows in their walls that let data run only that way: coming at one from the
  arrow's end breaks the pipe. Both always have both ends on the board.
- **Score**: 100 a segment (a crossover counts once per pass), 200 on fast flow,
  +500 for each loop back through a crossover, +300 for a buffer; at the end of
  a passed level +5000 for five or more loops (the original's big bonus for
  five crossovers), +1000 if `nul` swallowed some; -50 for each laid piece the
  data never reached. The status line counts loops once there is one.
- **Levels** (`Level::new`): required 8 + 2 per level (max 30), countdown 24 s
  down to 8, 2.2 s a segment times 0.9 per level (min 0.6), one more locked cell
  per level (max 8), the `nul` sink from level 3. Endless; failing ends the run.
- **Sound**, all synthesised, no files: a relay click to lay, a crunch to
  replace, a buzz when refused, ticks for the last three seconds of the
  countdown, a modem chirp when the data starts; every segment plays the next
  note up a pentatonic scale, a chord when the target is met; arpeggios for a
  loop and the five-loop bonus, a gurgle for a buffer, a drain swirl for `nul`,
  hiss and falling gurgle for a broken pipe. While data flows, a hum and a
  modem-like chatter whose rate follows the flow (busier with more fronts). M
  mutes, `--mute` starts muted.
- **Versus over iroh** (LAN; Tetrix's attack model): V on the title hosts, C
  lists matches on the LAN (beacon on UDP 31459, next to Tetrix's 31457 and
  Panzer's 31458). Each round both get the same level, board and bag and play
  their own copy at once; every 3 segments you pipe, and every loop through a
  crossover, locks a free cell on the rival's board (never the source's front,
  `nul`'s mouth or their cursor). Passing beats failing, then more segments,
  then score; first to 3 rounds, each a level harder. No pausing a race; Esc
  leaves. The rival's board is drawn live beside yours when the terminal is 144
  columns or wider, otherwise their progress sits in the status line. Tested
  2026-10-05 with two instances on one machine: discovery, joining, rounds,
  results, junk landing, the side-by-side view.
- Pads are ignored while the terminal reports it has lost focus: David was
  playing another game with the same pad and SIGPIPE, open in another window,
  took the presses (2026-10-05).
- Pieces dealt from a shuffled bag of 13 (two of each straight and corner, one
  crossover), plus one of each tee from level 2 (17), Tetris-style. Plain random odds (corners 14% each) were tried first
  and left David waiting on one corner for too long (2026-10-05); with the bag a
  shape is never more than 23 pieces away, usually far less.

## Name

**SIGPIPE**, chosen by David on 2026-10-04. In Unix, SIGPIPE is the signal a
process gets when it writes into a pipe whose reader is gone; unhandled, it kills
the process. That is exactly how you lose: the flow reaches an open end and spills.
Unix's own word for game over, and every terminal user already knows it.

- **SIGPIPE**: the name, in-game and on the store page.
- Game over reads `Broken pipe`, the shell's own error text. Tagline idea:
  "write error: Broken pipe".
- Crate, binary, repo and folder: `sigpipe`. The name is taken on crates.io (a
  small utility that resets the signal), but SIGPIPE ships through GitHub and the
  catalog, not crates.io, so that doesn't matter.

Checked 2026-10-04: no game called SIGPIPE on Steam or itch.io. **TMview** (EUIPO,
USPTO, UK, PRV and most national offices): no SIGPIPE mark anywhere ("contains"
finds none). Fuzzy finds 20, none for games: Bigpipe (telecoms, tobacco), SIPIPE
(Siemens industrial software, class 9), SIGPIP (Portuguese real-estate software,
class 9, 2024), SILPIPE, SIMPIPE. Clear. Runner-up: **Roer** (Swedish *rör*, pipe, the
same `ö` to `oe` as Moraen; also clear). Rejected: Plumb (crowded on Steam, crate
taken), Broken Pipe (*Broken Pipes* and *broken pipe* on itch.io), Backflow (*Back
Flow* on itch.io), mkfifo (means nothing to non-programmers).

## Rights (first pass, to finish)

- Mechanics are free, on the same footing as uXo and Moraen.
- History: The Assembly Line made it (1989); Empire Interactive published it in
  Europe and Lucasfilm Games ported it as *Pipe Dream*. Empire published a 2008
  remake by Razorworks; on Steam that remake is published by Strategy First.
- **Marks** (TMview, 2026-10-04): every PIPE MANIA mark is expired or ended (New
  World IP in the EU and UK, classes 9/28/41; Empire Interactive and New World IP
  in the US). The game PIPE DREAM marks (Lucasfilm, LucasArts, New World IP, Empire's
  PIPE DREAMS 3D) are all expired or ended too. Live "Pipe Dream" marks belong to
  unrelated owners (Sweden class 41, 2018; Brazil and Colombia class 28). Who holds
  the game's copyright today is still unknown; it doesn't matter for mechanics.
  Neither name goes in our title or store copy.

## Existing terminal versions (checked 2026-10-04)

- **`pipes` in nbsdgames** (abakh): a real Pipe Mania in ncurses: cursor, place or
  replace pieces, sewage flowing after a delay, `f` for fast flow, mouse, high
  scores. One screen, no levels. CC0 (LICENSE read); in Debian main (6.0.1 in
  sid/forky, 5 in trixie). Free to borrow from, and a candidate for the catalog
  itself under the OLD! banner.
- **GareginChorokhyan/PipeMania**: C++ ncurses, AGPL-3.0, menu, options, records;
  four commits, early.
- In Debian but graphical, not terminal: vodovod (SDL flow puzzle), pipewalker
  (rotation puzzle).

## Why the terminal suits it

- **The tiles already exist**: the box-drawing set `═ ║ ╔ ╗ ╚ ╝ ╠ ╣ ╦ ╩ ╬` is pipes.
  Glyph mode is not a fallback here; it is the natural look.
- **The liquid** fills each segment pixel by pixel in half-blocks (the uXo and
  Moraen renderer), following the pipe's shape, so you see the flow front creep.
- **Unix pipes as theme**: pieces could be commands, the flow could be data, a
  `tee` piece splits the stream, a `/dev/null` sink swallows it. To weigh against
  plain water and sewage when the design is done.

## Ideas to weigh (not approved)

- Classic rules first: grid, a queue of upcoming pieces, replacing a placed piece
  costs time, the flow starts after a countdown, a minimum length to pass, the
  crossing piece scores a bonus when the flow goes through it twice.
- Obstacles and specials from the genre: blocked cells, reservoirs that slow the
  flow, one-way pieces, end pieces, the fast-flow button.
- Versus over iroh: two boards, laid pipe sends junk to the rival (Tetrix's attack
  model).
- Gamepads first-class, as in every Asciiverse game; session logging per the
  convention.

## Neighbour to watch

**Ohm** (circuit-building puzzle) is close in spirit. Keep them apart: SIGPIPE is
real-time pressure against a moving front; Ohm is a solver puzzle.

## Open questions

- [x] Name: SIGPIPE
- [x] TMview trademark search for SIGPIPE: clear
- [x] Pipe Mania / Pipe Dream marks: all lapsed (copyright owner unknown)
- [x] Theme: Unix data streams (David, 2026-10-05)
- [x] Crate name: `sigpipe`. Not going to crates.io (GitHub and the Asciiverse
  catalog only), so the taken crates.io name doesn't matter; checked 2026-10-05,
  no `sigpipe` command or Debian package on this system to clash with
- [ ] Play and tune the first cut's numbers
- [x] Crossover plus `tee` splitter (David, 2026-10-05)
- [x] Buffers, one-way valves, the five-loop bonus (2026-10-05)
- [x] Sound (2026-10-05), synthesised in `src/audio.rs` after Panzer's
- [x] Versus over iroh on the LAN (2026-10-05)
- [ ] Next: versus over the internet (an iroh ticket to paste, as the LAN
  beacon can't cross networks), a pad-driven way into versus (V and C are
  keyboard only)
