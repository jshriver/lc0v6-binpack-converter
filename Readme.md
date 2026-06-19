# LC0 V6 Training Data → Binpack Converter

A fast utility for converting **Leela Chess Zero (Lc0) V6 training data** (`.gz` files) into **Stockfish-compatible binpack** format.

## Features

* 🚀 Converts one or more LC0 V6 training chunks into a single binpack file
* 📊 Optional summary statistics
* 🎯 Supports filtering to **classical** games only
* 🗜️ Reads compressed `.gz` training files directly

## Building

Compile the project in release mode:

```bash
cargo build --release
```

The resulting executable will be located at:

```text
target/release/lc0_parser
```

## Usage

Convert one or more training files into a binpack:

```bash
./lc0_parser --summary --classical-only -o <filename>.binpack <directory>/*.gz
```

To process an entire directory of training chunks:

```bash
time find <directory> \
    -maxdepth 1 \
    -name "*.gz" \
    -print0 | \
xargs -0 ./lc0_parser --summary --classical-only -o <filename>.binpack
```

## Why use `find` and `xargs`?

Some training runs (such as **T91**) contain enough `.gz` files that using a shell glob like:

```bash
directory/*.gz
```

may exceed your operating system's **maximum command-line argument length** ("Argument list too long").

Using `find` together with `xargs` avoids this limitation and allows the converter to process arbitrarily large collections of training files.

## Note

Since a lot of the files exceed the arg limit requiring xargs it opens the output binpack in append mode.  Since xargs essentially re-runs the parser
for each slice of files it's handling.  So make sure to use a new name for the output binpack else it will append to an existing one.


Produces:

* `<filename>.binpack` — the generated binpack file
* Summary statistics (when `--summary` is enabled)

## License

GPLv3


