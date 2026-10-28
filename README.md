# SIGPIPE

**THE DATA IS COMING. LAY PIPE. FAST.**

A process is about to start writing, and its data has nowhere to go. Grab
pieces from the queue and lay pipe across the grid before the countdown runs
out, then stay ahead of the green data as it creeps along, a note higher with
every segment. Loop it through crossovers, split it with tees, slow it with
buffers, sink it in `nul`. Let it reach an open end and that's the signal:
`cat: write error: Broken pipe`, `[exit 141]`. Endless levels, each one
tighter, and versus on the local network.

## Play

With [av](https://asciiverse.sh), the Asciiverse client:

```
av install sigpipe
av play sigpipe
```

Or download the archive for your system from this repository's Releases,
unpack it and run `./sigpipe`.

To build it yourself: `cargo run --release`.

Fits an 80x24 terminal.

## Keys

| Key | |
|---|---|
| Arrows, W A S D, H J K L | move the cursor |
| Enter, Space, mouse click | lay the next piece (or replace one) |
| F | fast flow, double points |
| P | pause |
| M | sound on or off |
| Esc / Q | back / quit |
| V / C (title) | host / join a versus match on the local network |

Gamepad: d-pad moves, bottom button lays, left or top button or right
trigger for fast flow, Start pauses.

## Options

`--level N` starts at level N, `--mute` starts silent.

Session log for bug reports: `RUST_LOG=debug sigpipe 2>sigpipe.log`

## Credits

The mechanics are those of the late-1980s pipe-laying puzzle games;
everything else is new. Sound is synthesised in the game. Networking uses
[iroh](https://iroh.computer).

## Licence

MIT; see [LICENSE](LICENSE).
