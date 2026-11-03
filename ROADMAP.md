# Roadmap

SIGPIPE is at 0.3.0 (2026-10-28): the first playable cut, with its rules
and numbers still a first proposal. 1.0.0 is when everything below is in.

## 0.3.0: built

- [x] Classic rules on a 10 by 7 board: the countdown, replacing pieces, fast flow
- [x] The Unix data-stream theme: process boxes, `[exit 0]`, `Broken pipe`
- [x] Crossover and `tee`, buffers, one-way valves, the `nul` sink, locked cells
- [x] Endless levels that tighten; a saved hi score
- [x] Synthesised sound
- [x] Versus over iroh on the LAN, first to three rounds
- [x] Keyboard, mouse and gamepad; session logging

## 0.3.1: built

- [x] Gamepads work on Windows: gilrs reads them through XInput, since its
  default (Windows Gaming Input) only reports to a program with a focused
  window of its own, which a terminal game never has

## 0.4.0: the rules settled

- [ ] Play the first cut and tune its numbers: countdown, flow speed, required
  length, locked cells, scores, the piece bag
- [ ] Rules approved, and the "first proposal" notes in `CLAUDE.md` updated

## 0.5.0: versus anywhere

- [ ] Versus on two physical machines on a real LAN (so far only tested with
  two copies on one machine)
- [ ] Versus over the internet: an iroh ticket to paste, since the LAN
  beacon can't cross networks
- [ ] A way into versus with a gamepad (V and C are keyboard only)

## 1.0.0

- [ ] Everything above played through and tuned
- [ ] What else 1.0 needs is not decided yet
