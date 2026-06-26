// Copyright (C) 2026 Joshua Shriver <jshriver@gmail.com>
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://gnu.org>.
//! Syzygy endgame tablebase probe wrapper.
//!
//! ## Feature flag
//!
//! This module is compiled only when the `syzygy` feature is enabled
//! (the default).  When the feature is absent the public API is replaced
//! by a zero-cost stub so the rest of the codebase compiles unchanged.
//!
//! ## Piece-count guard
//!
//! We skip the probe entirely for positions with more than 7 pieces.
//! This avoids pointless filesystem lookups on the vast majority of
//! middlegame positions and is cheap — `shakmaty`'s `Board::occupied()`
//! is a single bitboard popcount.
//!
//! ## WDL → result
//!
//! `probe_wdl_after_zeroing` ignores the 50-move clock, which is correct
//! for training data: we want the unconditional game-theoretic outcome.
//!
//! `Wdl::signum()` collapses the five WDL values to +1/0/-1, treating
//! CursedWin as a win and BlessedLoss as a loss — the right choice for
//! training signal quality.
//!
//! The returned value is **side-to-move relative**, matching the convention
//! of `TrainingDataEntry::result` in sfbinpack.
//!
//! ## Load-time table inventory
//!
//! `load()` reports how many table files it found and the largest piece
//! count among them (e.g. "found 1742 tables, up to 6-men"). Both numbers
//! come directly from `shakmaty_syzygy::Tablebase`'s own API rather than
//! anything we infer ourselves:
//!
//! - `Tablebase::add_directory()` returns `io::Result<usize>`, where the
//!   `usize` is the count of table files it added from that directory.
//!   Summing this across all configured directories gives `file_count`.
//! - `Tablebase::max_pieces()` returns the largest material count seen
//!   across every table file added so far. It's a plain field on the
//!   struct that's updated via `max(self.max_pieces, pieces)` each time a
//!   file is added, so it's fully data-driven — calling it once after all
//!   directories are loaded gives the true maximum across everything we
//!   loaded, not a hardcoded constant.
//!
//! (Verified against the vendored source for shakmaty-syzygy 0.24.0 and
//! 0.28.1; both versions expose the same signatures.)

// ── Feature-enabled implementation ───────────────────────────────────────────

#[cfg(feature = "syzygy")]
mod inner {
    use std::path::Path;
    use shakmaty::{Chess, Position};
    use shakmaty_syzygy::Tablebase;

    /// Maximum piece count for which Syzygy tables exist.
    const MAX_PIECES: usize = 7;

    /// Summary of the table files discovered at load time.
    #[derive(Default, Clone, Copy)]
    pub struct TableInventory {
        /// Total number of table files added, summed across all configured
        /// directories (each directory's contribution comes straight from
        /// `Tablebase::add_directory`'s return value).
        pub file_count:   usize,
        /// Largest material count (`max_pieces()`) across every table file
        /// added. `0` if no tables were found (mirrors the crate's own
        /// default of `0` before any file is added).
        pub max_pieces:   usize,
    }

    /// Thin wrapper around a `shakmaty_syzygy::Tablebase<Chess>`.
    ///
    /// Constructed once at startup and shared for the lifetime of the run.
    /// `probe` is the only public entry point; it is designed to be called
    /// on every position until the first hit, after which the caller should
    /// stop probing and propagate the result.
    pub struct SyzygyProber {
        tables:    Tablebase<Chess>,
        loaded:    bool,
        inventory: TableInventory,
    }

    impl SyzygyProber {
        /// Create a prober with no tables loaded.  All probes will return `None`.
        pub fn disabled() -> Self {
            Self {
                tables:    Tablebase::new(),
                loaded:    false,
                inventory: TableInventory::default(),
            }
        }

        /// Load all `.rtbw` / `.rtbz` files found under one or more paths.
        ///
        /// `path` may contain multiple directories separated by `:` (or `;`
        /// on Windows), e.g. `--syzygy-path /tb/3-4-5:/tb/6-piece`.
        /// Each directory is added in order; duplicates are harmless.
        ///
        /// Returns an error string if any directory cannot be read.
        ///
        /// On success, also builds a `TableInventory` from the same calls
        /// used to load the tables: `file_count` is the sum of each
        /// `add_directory()` call's returned file count, and `max_pieces`
        /// is read once via `Tablebase::max_pieces()` after every directory
        /// has been added.
        pub fn load(path: &Path) -> Result<Self, String> {
            let separator = if cfg!(windows) { ';' } else { ':' };
            let path_str  = path.to_string_lossy();
            let dirs: Vec<&str> = path_str.split(separator).collect();

            let mut tables = Tablebase::new();
            let mut file_count = 0usize;

            for dir in &dirs {
                let p = std::path::Path::new(dir);
                let added = tables
                    .add_directory(p)
                    .map_err(|e| format!("syzygy: failed to load {}: {e}", p.display()))?;
                file_count += added;
            }

            let inventory = TableInventory {
                file_count,
                max_pieces: tables.max_pieces(),
            };

            Ok(Self { tables, loaded: true, inventory })
        }

        /// Returns `true` if any tables were loaded.
        pub fn is_loaded(&self) -> bool { self.loaded }

        /// Inventory of table files discovered at load time (file count and
        /// the largest material count among them). Zeroed if `load()` was
        /// never called successfully.
        pub fn inventory(&self) -> TableInventory { self.inventory }

        /// Probe the position described by `fen`.
        ///
        /// Returns `Some(result)` where `result` is +1 (STM wins), 0 (draw),
        /// or -1 (STM loses), or `None` if:
        /// - No tables are loaded.
        /// - The position has more than 7 pieces.
        /// - The position is not in the tablebase.
        /// - The FEN cannot be parsed as standard chess (should not happen
        ///   since `check_legal_standard_chess` already validated it).
        /// - The probe fails for any other reason.
        pub fn probe(&self, _fen: &str, shakmaty_pos: &Chess) -> Option<i16> {
            if !self.loaded {
                return None;
            }
            // Skip positions with too many pieces — no table can cover them.
            let piece_count = shakmaty_pos.board().occupied().count();
            if piece_count > MAX_PIECES {
                return None;
            }
            match self.tables.probe_wdl_after_zeroing(shakmaty_pos) {
                Ok(wdl) => Some(wdl.signum() as i16),
                Err(_)  => None,
            }
        }
    }
}

// ── Zero-cost stub when feature is disabled ───────────────────────────────────

#[cfg(not(feature = "syzygy"))]
mod inner {
    use std::path::Path;
    use shakmaty::Chess;

    #[derive(Default, Clone, Copy)]
    pub struct TableInventory {
        pub file_count: usize,
        pub max_pieces: usize,
    }

    pub struct SyzygyProber;

    impl SyzygyProber {
        pub fn disabled() -> Self { Self }
        pub fn load(_path: &Path) -> Result<Self, String> {
            Err("syzygy feature not compiled in".to_string())
        }
        pub fn is_loaded(&self) -> bool { false }
        pub fn inventory(&self) -> TableInventory { TableInventory::default() }
        pub fn probe(&self, _fen: &str, _pos: &Chess) -> Option<i16> { None }
    }
}

// Re-export so callers just use `syzygy::SyzygyProber` / `syzygy::TableInventory`.
pub use inner::{SyzygyProber, TableInventory};