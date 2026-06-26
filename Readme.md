# lc0v6-binpack-converter

A command-line tool that converts [Leela Chess Zero](https://lczero.org/) V6
training data (`.gz` files of `V6TrainingData` records) into [Stockfish
binpack](https://github.com/official-stockfish/Stockfish) format, with
optional inspection/printing modes and optional correction of game results
using [Syzygy endgame tablebases](https://syzygy-tables.info/).

## What it does

lc0 self-play games are stored as fixed-size binary records (8356 bytes
each), gzip-compressed, one file per game. Each record encodes a single
training position: the board (as input planes), the search policy, the best
move found, and the eventual game result from that position's
perspective.

This tool:

- **Parses** those records directly from the raw byte layout (no
  dependency on lc0 itself).
- **Reconstructs a FEN** for each position, including castling rights and
  en passant, from the encoded bitboard planes.
- **Validates** every position/move pair against [shakmaty](https://github.com/niklasf/shakmaty)
  to catch and skip Chess960/FRC positions that slip through lc0's
  `input_format` filter.
- **Converts** each record into a binpack training entry (FEN, move, score,
  ply, result) using [`sfbinpack`](https://crates.io/crates/sfbinpack).
- **Optionally corrects results** using Syzygy tablebases: if a position
  late in a game has ≤7 pieces and is found in the tablebase, the
  game-theoretic result is propagated through the game, replacing
  lc0's recorded (possibly wrong, e.g. adjudicated) outcome.
- **Optionally prints** human-readable per-record output for inspection
  and debugging, or aggregate statistics across a whole dataset.

## Building

```bash
cargo build --release
```

The binary is built with the `syzygy` feature enabled by default. To build
without Syzygy support (smaller binary, no `shakmaty-syzygy` dependency):

```bash
cargo build --release --no-default-features
```

With Syzygy support disabled, `--syzygy-path` will print an error and exit
if passed; everything else works unchanged.

## Usage

```
lc0v6-binpack-converter [OPTIONS] [<file> ...]
```

### Options

| Flag | Description |
|---|---|
| `-b`, `--brief` | One-line summary per record |
| `-n`, `--normal` | Key fields per record (FEN, value targets, move info, policy top-5) |
| `-f`, `--full` | Everything in `--normal`, plus the full policy vector and non-zero input planes |
| `-l`, `--limit N` | Only process the first `N` records |
| `-s`, `--skip N` | Skip the first `N` records |
| `--summary` | Print aggregate statistics (versions, formats, Q/D averages, adjudication rate, etc.) at the end |
| `-o`, `--output <file>` | Export to a `.binpack` file (appends if it already exists) |
| `-d`, `--input-dir <dir>` | Process every `.gz` file in a directory, sorted by filename. Avoids OS/shell glob limits — useful when a training run has thousands of game files. Can be combined with explicit file arguments. |
| `--syzygy-path <paths>` | Colon-separated (semicolon on Windows) list of Syzygy tablebase directories. Positions with ≤7 pieces are probed; the first hit in a game overrides the result and is propagated forward through the rest of the game. |
| `--backpropagate` | Also propagate the first Syzygy hit **backward to move 1** (unlimited distance). Requires `--syzygy-path`. |
| `--backpropagate-limit N` | Like `--backpropagate`, but only patches the `N` plies immediately **before** the hit, instead of going all the way back to move 1. Enables backward propagation on its own — you don't need `--backpropagate` as well. See [Choosing a backpropagation mode](#choosing-a-backpropagation-mode) below. |
| `-h`, `--help` | Show usage |

Without `-b`/`-n`/`-f`, the tool shows a live progress bar instead of
per-record output. Both raw binary and gzip-compressed (`.gz`) files are
accepted as input; the tool detects gzip by file extension.

Non-classical (FRC/Chess960 or other variant) records are always skipped,
regardless of other flags — this tool only handles standard chess.

### Examples

```bash
# Process an entire directory of self-play games into one binpack file
lc0v6-binpack-converter -d /data/training -o out.binpack

# Windows — no more PowerShell glob workarounds for tens of thousands of files
lc0v6-binpack-converter -d C:\training\run1 -o out.binpack

# Export with Syzygy result correction (forward-only)
lc0v6-binpack-converter -d /data/training -o out.binpack --syzygy-path /tb/syzygy

# Export with full back+forward propagation
lc0v6-binpack-converter -d /data/training -o out.binpack --syzygy-path /tb/syzygy --backpropagate

# Export with backprop capped at 30 plies before each tablebase hit
lc0v6-binpack-converter -d /data/training -o out.binpack --syzygy-path /tb/syzygy --backpropagate-limit 30

# Inspect the first 10 records of a single file
lc0v6-binpack-converter --normal --limit 10 game.gz

# Mix a directory with extra explicit files
lc0v6-binpack-converter -d /data/training extra1.gz extra2.gz -o out.binpack
```

## Syzygy tablebase correction

lc0 self-play games sometimes end with a recorded result that doesn't match
the true game-theoretic outcome — most commonly because the game was
**adjudicated** (stopped early based on an eval/visit heuristic) or drawn by
repetition/50-move rule inside a position that a perfect player would have
won. Syzygy tablebases give the *proven* result for any position with ≤7
pieces, so this tool can use them to correct the training labels.

### How propagation works

For each game (one `.gz` file = one game):

1. Positions are probed in order until the first tablebase hit. Probing
   then stops for the rest of that game — once you're on a known mainline
   into a tablebase position, the outcome from that point on is determined,
   so there's no need for further disk I/O.
2. **Forward propagation** (always applied once there's a hit): every
   position from the hit to the end of the game has its result replaced
   with the tablebase value, with the sign flipped each ply (since result
   is always side-to-move relative).
3. **Backward propagation** (controlled by `--backpropagate` /
   `--backpropagate-limit`, off by default): positions *before* the hit
   are also patched, working backward.

### Choosing a backpropagation mode

Forward propagation is low-risk: once a position is inside the tablebase
horizon, the remaining outcome is provably correct. Backward propagation
makes a much stronger assumption — that the game's theoretical result was
*already* the same one or more moves earlier. That assumption gets weaker
the further back you go: a position 100 plies before the hit may have gone
through a completely different theoretical outcome (a winning advantage
squandered to a draw, a draw lost to a blunder, etc.) that the tablebase hit
says nothing about.

Three modes are available, in increasing order of risk/reach:

| Mode | Flag | Behavior |
|---|---|---|
| Off (default) | *(none)* | Forward-only. Never overwrites a position based on an assumption — only ever writes labels that are provably correct. |
| Limited | `--backpropagate-limit N` | Patches only the `N` plies immediately before the hit. A reasonable middle ground: short enough that "the result was already decided" is usually true, while still correcting more of the game than forward-only alone. |
| Unlimited | `--backpropagate` | Patches everything back to move 1. Maximizes the amount of corrected data, at the cost of trusting the "result was already decided" assumption arbitrarily far back. |

**Precedence when both flags are given:** `--backpropagate-limit N` always
wins. It's treated as strictly more specific than the unlimited flag, so
`--backpropagate --backpropagate-limit 30` behaves identically to
`--backpropagate-limit 30` alone. `--backpropagate-limit` does not require
`--backpropagate` to also be passed — it enables backward propagation on
its own.

If neither flag is given but `--syzygy-path` is, you get forward-only
correction, which is always safe to enable.

### Load-time table inventory

When `--syzygy-path` is given, the tool reports how many table files were
found and the largest piece count among them, e.g.:

```
♟  Syzygy: tables loaded from /home/jshriver/syzygy
♟  Syzygy: found 1020 table files (up to 6-men)
```

This comes directly from `shakmaty_syzygy::Tablebase`'s own bookkeeping
(`add_directory`'s returned file count, summed across all configured
directories, and `max_pieces()` read once after loading) — not from
guessing at filenames — so it accurately reflects whatever the probing
backend actually has access to.

### Reading the summary stats

After a run with `--syzygy-path`, you'll see something like:

```
📊 Original  WDL  — wins:   255204  draws:   744725  losses:   256603
♟  Syzygy: 1559 games had a TB hit
♟  Syzygy: 9237 positions corrected by propagation
📊 Corrected WDL  — wins:   255281  draws:   744525  losses:   256726
```

These two Syzygy counters answer different questions, and it's easy to
conflate them:

- **`N games had a TB hit`** counts *games*, not positions. Probing stops
  after a game's first hit, so this number is capped at one per game —
  it does **not** grow when you enable backpropagation, because
  backpropagation doesn't create more hits, it just makes the existing
  hits reach further into the game.
- **`N positions corrected by propagation`** counts individual training
  *positions* whose result actually changed value versus the original
  `result_q`. This is the number that grows with `--backpropagate` /
  `--backpropagate-limit`, since the same hit now reaches (and potentially
  corrects) many more positions per game.

It's normal for "Original WDL" and "Corrected WDL" to differ, with or
without backpropagation — that's the entire point of running Syzygy
correction. If lc0's self-play recorded a draw in a position that was
actually a proven win, correction will shift that draw to a win (or vice
versa). A correction run that left the aggregate WDL completely unchanged
would suggest the tablebase never disagreed with lc0's self-play, which
would be unusual on a large dataset with adjudicated games.

## Output formats

### `--brief`

```
[   142] training.123.gz  ver=6 fmt=1 stm=White rule50=  4 visits=    812 Q=+0.1823 (+128cp) D=0.1102 M=34.5 best=e2e4 (42.31%)  rnbqkbnr/... w KQkq - 4 1
```

### `--normal`

A multi-line block per record: FEN, header bytes, value targets
(root/best/orig/played Q-D-M, result Q/D, plies-left/MLH), move info
(played/best move with index), policy summary (visited-move count, top
move, top-5 by probability).

### `--full`

Everything in `--normal`, plus the complete 1858-entry policy vector and a
printed 8×8 grid for every non-zero input plane (useful for debugging the
plane-decoding logic itself).

### `--summary`

Aggregate statistics across every record processed in the run: total
records, total/average visits, average root Q/D, average plies-left,
average policy KL-divergence, adjudication rate, proven-best-move rate, and
a breakdown of versions and input formats seen.

> **Note:** averages are simple running sums divided by count. If any
> record in the dataset has a `NaN` value in a field being averaged (e.g.
> `root_q`), the corresponding average will print as `NaN` for the entire
> run, since `NaN` poisons any sum it's added to. A `NaN` average is a
> signal worth investigating in the source data — it does not mean the
> tool itself failed.

## How FEN reconstruction works

lc0 stores positions as 104 bitboard planes (8 history slots × 13 planes),
with each plane bit-reversed-per-byte relative to a standard `a1=0`
bitboard, and the board mirrored vertically whenever Black is to move (lc0
always orients the board so the side to move's pieces are at the bottom).
`fen.rs` undoes both transforms to reconstruct a standard White-at-bottom
FEN, including:

- **Castling rights**, validated against actual king/rook placement so
  that an FRC position which slipped through the format filter doesn't
  produce a self-contradictory FEN — inconsistent rights are silently
  dropped rather than emitted.
- **En passant**, recovered differently depending on encoding:
  - *Canonical* input formats encode the ep file directly as a bitmask.
  - *Classical* format (the only one this tool processes — see below)
    encodes ep via "phantom pawns" placed on rank 1/8 of the side-to-move
    frame, per lc0's own internal convention; `fen.rs` strips these
    phantoms from the rendered board after extracting the ep square from
    them.

## Move and policy index handling

lc0's 1858-entry policy vector is indexed by a fixed move table
(`moves.rs`, transcribed from lc0's `encoder.cc`). Policy/move indices are
always given from the side-to-move's frame of reference; when Black is to
move, `binpack.rs` mirrors the UCI move vertically before applying it to
the standard-orientation FEN, and before checking it for legality or
probing the tablebase.

## FRC / Chess960 filtering

Even though only `input_format == 1` (classical) records are processed,
some Chess960 positions can still appear with that format tag if the
self-play run mixed variants. Rather than relying on heuristic geometry
checks, every move is independently validated for legality in **standard**
chess using `shakmaty`. Any position or move shakmaty rejects is treated as
an FRC record and skipped (tallied separately in the summary and progress
output, not counted as an error).

## Performance

Records are read in fixed 8356-byte chunks directly from a buffered
reader (gzip-decompressing on the fly for `.gz` input), with no
intermediate string parsing. On typical hardware this processes on the
order of tens of thousands of records per second, dominated mostly by
gzip decompression and tablebase disk I/O (when enabled) rather than CPU
parsing time.

## License

GPL-3.0-or-later. See source file headers for the full notice.

```
Copyright (C) 2026 Joshua Shriver <jshriver@gmail.com>

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.
```
## Acknowledgements

* The Stockfish and LC0 team for their hard work over the years and the wonderful engines that resulted.
* The people who donate their computing time for the Lc0 training network and the Stockfish fishtest system.
* Disservin for the very useful sfbinpack crate
* Jamie Whiting for his bullet training system.




