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

//! Convert V6TrainingData records to Stockfish binpack format.
//!
//! Uses the sfbinpack crate's CompressedTrainingDataEntryWriter.
//!
//! ## Position / move pairing
//!
//! In lc0 V6 each record is self-contained: the FEN and the search data
//! (best_idx, best_q, policy) all describe the same position.  There is no
//! off-by-one between records.
//!
//! ## Move coordinate system
//!
//! lc0 always stores policy indices from the perspective of the side to move,
//! with the board oriented so that side's pieces are at the bottom (ranks 1-2).
//! When Black is to move the board is stored flipped vertically, so the move
//! indices are also in flipped coordinates.
//!
//! Because our FEN is always in standard (White-at-bottom) coordinates, we
//! must mirror move squares vertically when Black is to move:
//!   mirrored_rank = 7 - rank   (0-indexed)
//! e.g. e2e4 (Black to move) -> e7e5 in standard coords.
//!
//! ## Score
//!   best_q -> centipawns via q_to_cp().  Proven mates: ±32000 cp.
//!
//! ## Result
//!   Derived from result_q (+1 win, 0 draw, -1 loss, side-to-move perspective).
//!   When Syzygy tables are loaded the first in-range probe hit overrides
//!   result_q and is propagated through the game buffer (see below).
//!
//! ## Ply / chaining
//!
//! `sfbinpack`'s `CompressedTrainingDataEntryWriter` keeps the last entry it
//! wrote and only delta-compresses the next one (skipping the full FEN) when
//! `TrainingDataEntry::is_continuation` holds:
//!   - `ply` increases by exactly 1, AND
//!   - applying the previous entry's move to its position produces exactly
//!     the next entry's position, AND
//!   - `result` flips sign between the two (this falls out automatically
//!     since lc0's `result_q` is already side-to-move relative).
//!
//! The caller (see `main.rs::process_file`) is responsible for handing us a
//! `ply` that increments by one per record within a game and starts over at
//! each new file/game. We just thread it through to the entry unchanged.
//!
//! ## En passant
//!
//! Classical-format records encode en passant via phantom pawns in plane 6
//! (see fen.rs).  `record_to_fen` now emits the correct ep square into the
//! FEN, so `sfbinpack`'s `Position::from_fen` will have full knowledge of the
//! ep square.  `parse_uci_move` uses the position's ep square to distinguish
//! a genuine en-passant capture from an illegal diagonal pawn move.
//!
//! ## FRC detection
//!
//! Some FRC positions slip through the `input_format == 1` filter.
//! Rather than heuristic geometry checks, we use shakmaty to validate move
//! legality in standard chess.  If shakmaty rejects the move the record is
//! an FRC position and is skipped.  This catches all FRC cases regardless of
//! whether castling is involved or how far the king has moved.
//!
//! ## Syzygy tablebase integration
//!
//! When a `SyzygyProber` is provided and a position has ≤7 pieces, we probe
//! the WDL value for that position.  On the first hit within a game:
//!
//! - All positions from the hit forward to end-of-game have their result
//!   patched to the TB value (with sign flipped per ply, STM-relative).
//! - If `--backpropagate` is set, all positions from the hit backward to
//!   move 0 are also patched (again flipping sign per ply from the hit).
//!
//! Probing stops after the first hit — no further disk I/O is needed since
//! the WDL outcome is monotonically consistent along the mainline.
//!
//! ## Game buffer
//!
//! To support result patching we buffer all entries for a game before writing.
//! Call `flush_game(backpropagate)` at the end of each game (i.e. each .gz
//! file) and `flush()` once at the very end to finalise the stream.

use std::fs::OpenOptions;
use std::{fs::File, io::{self, BufWriter}, path::Path};
use sfbinpack::{
    CompressedTrainingDataEntryWriter, TrainingDataEntry,
    chess::{
        color::Color,
        coords::Square,
        piece::Piece,
        piecetype::PieceType,
        position::Position,
        r#move::Move,
        castling_rights::{CastleType, CastlingTraits},
    },
};
use shakmaty::{Chess, CastlingMode, fen::Fen, uci::UciMove as ShakmatyUci};
use crate::{
    fen::record_to_fen,
    moves::MOVE_STRS,
    record::{V6Record, q_to_cp},
    syzygy::SyzygyProber,
};

// ── Error type ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum BinpackError {
    Fen(String),
    InvalidMove(String),
    FrcCastle(String),
    InvalidPosition(String),
    Io(io::Error),
    Writer(String),
}

impl std::fmt::Display for BinpackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fen(e)             => write!(f, "FEN error: {e}"),
            Self::InvalidMove(e)     => write!(f, "Invalid move: {e}"),
            Self::FrcCastle(e)       => write!(f, "FRC position (skipping): {e}"),
            Self::InvalidPosition(e) => write!(f, "Invalid position: {e}"),
            Self::Io(e)              => write!(f, "IO error: {e}"),
            Self::Writer(e)          => write!(f, "Writer error: {e}"),
        }
    }
}

impl From<io::Error> for BinpackError {
    fn from(e: io::Error) -> Self { Self::Io(e) }
}

// ── Square mirroring ──────────────────────────────────────────────────────────

/// Mirror a square string vertically (rank 1 ↔ rank 8).
///
/// lc0 policy indices are in the side-to-move frame (their pieces at the
/// bottom). When Black is to move we must un-flip squares before applying
/// them to a standard White-at-bottom FEN.
fn mirror_square(sq_str: &str) -> Option<String> {
    if sq_str.len() != 2 { return None; }
    let bytes = sq_str.as_bytes();
    let file = bytes[0];
    let rank = bytes[1];
    if !(b'a'..=b'h').contains(&file) || !(b'1'..=b'8').contains(&rank) {
        return None;
    }
    let mirrored_rank = b'1' + (b'8' - rank);
    Some(format!("{}{}", file as char, mirrored_rank as char))
}

/// Mirror a full UCI move string vertically when Black is to move.
///
/// Handles 4-char normal moves and 5-char promotion moves.
fn mirror_uci_move(uci: &str) -> Option<String> {
    if uci.len() < 4 || uci.len() > 5 { return None; }
    let from = mirror_square(&uci[0..2])?;
    let to   = mirror_square(&uci[2..4])?;
    if uci.len() == 5 {
        Some(format!("{}{}{}", from, to, &uci[4..5]))
    } else {
        Some(format!("{}{}", from, to))
    }
}

// ── Move parsing ──────────────────────────────────────────────────────────────

/// Parse a UCI move string (already in standard White-at-bottom coords) into
/// an sfbinpack Move.
///
/// Move type classification:
///
/// - **Promotion**: 5-char UCI (e.g. `e7e8q`).
/// - **Castling**: king on its classical starting square (e1/e8) moves more
///   than one file AND the position has castling rights for that direction.
///   Encoded as king-captures-own-rook.
/// - **FRC castle detected (king not on start square)**: king moves more than
///   one file but is not on e1/e8. Returns `BinpackError::FrcCastle`.
/// - **FRC castle detected (no rights)**: king is on e1/e8, moves more than
///   one file, but the position has no castling rights for that direction.
///   Returns `BinpackError::FrcCastle`.
/// - **En passant**: pawn moves diagonally to an empty square that matches
///   the position's ep square.
/// - **Normal**: everything else.
fn parse_uci_move(uci: &str, pos: &Position) -> Result<Move, BinpackError> {
    if uci.len() < 4 || uci.len() > 5 {
        return Err(BinpackError::InvalidMove(format!("bad length: {uci}")));
    }

    let from = Square::from_string(&uci[0..2])
        .ok_or_else(|| BinpackError::InvalidMove(format!("bad from square: {uci}")))?;
    let to = Square::from_string(&uci[2..4])
        .ok_or_else(|| BinpackError::InvalidMove(format!("bad to square: {uci}")))?;

    // ── Promotion ─────────────────────────────────────────────────────────────
    if let Some(promo_char) = uci.chars().nth(4) {
        let color = pos.side_to_move();
        let pt = match promo_char {
            'q' => PieceType::Queen,
            'r' => PieceType::Rook,
            'b' => PieceType::Bishop,
            'n' => PieceType::Knight,
            c   => return Err(BinpackError::InvalidMove(format!("bad promo piece: {c}"))),
        };
        return Ok(Move::promotion(from, to, Piece::new(pt, color)));
    }

    let moving_piece = pos.piece_at(from);

    // ── Castling ──────────────────────────────────────────────────────────────
    if moving_piece.piece_type() == PieceType::King {
        let file_diff = (to.index() & 7) as i32 - (from.index() & 7) as i32;
        if file_diff.abs() > 1 {
            let color = pos.side_to_move();

            let expected_king_sq = match color {
                Color::White => Square::E1,
                Color::Black => Square::E8,
            };
            if from != expected_king_sq {
                return Err(BinpackError::FrcCastle(format!(
                    "king on {from:?} (not e1/e8) moved {file_diff:+} files: {uci}"
                )));
            }

            let kingside = file_diff > 0;
            let has_right = if kingside {
                pos.castling_rights().contains(
                    CastlingTraits::castling_rights(color, CastleType::Short)
                )
            } else {
                pos.castling_rights().contains(
                    CastlingTraits::castling_rights(color, CastleType::Long)
                )
            };

            if has_right {
                let rook_sq = match (kingside, color) {
                    (true,  Color::White) => Square::H1,
                    (true,  Color::Black) => Square::H8,
                    (false, Color::White) => Square::A1,
                    (false, Color::Black) => Square::A8,
                };
                return Ok(Move::castle(from, rook_sq));
            } else {
                return Err(BinpackError::FrcCastle(format!(
                    "king move {uci} looks like FRC {} castle but no rights",
                    if kingside { "kingside" } else { "queenside" },
                )));
            }
        }
    }

    // ── En passant ────────────────────────────────────────────────────────────
    if moving_piece.piece_type() == PieceType::Pawn {
        let from_file = (from.index() & 7) as i32;
        let to_file   = (to.index()   & 7) as i32;
        if from_file != to_file && pos.piece_at(to) == Piece::none() {
            let ep_sq = pos.ep_square();
            if ep_sq == to {
                return Ok(Move::en_passant(from, to));
            } else {
                return Err(BinpackError::InvalidMove(format!(
                    "diagonal pawn to empty non-ep square: {uci} \
                     (position ep square is {ep_sq:?})"
                )));
            }
        }
    }

    // ── Normal move ───────────────────────────────────────────────────────────
    Ok(Move::normal(from, to))
}

// ── Score conversion ──────────────────────────────────────────────────────────

/// Convert `best_q` to a centipawn score for the binpack entry.
///
/// Uses `q_to_cp` which clamps to ±32 000 and returns `i16` directly.
/// Exact ±1.0 (proven mates) saturate to ±32 000.
fn best_q_to_score(rec: &V6Record) -> i16 {
    q_to_cp(rec.best_q).unwrap_or(0)
}

// ── Result conversion ─────────────────────────────────────────────────────────

/// Convert `result_q` to a binpack result (side-to-move perspective).
/// Returns +1 (win), 0 (draw), or -1 (loss).
/// result_q from lc0 is typically exactly +1.0, 0.0, or -1.0.
fn result_q_to_result(result_q: f32) -> i16 {
    if      result_q >  0.5 {  1 }
    else if result_q < -0.5 { -1 }
    else                    {  0 }
}

// ── Shakmaty legality check + optional TB probe ───────────────────────────────

/// Outcome of the shakmaty legality check, bundling the parsed position so
/// the caller can pass it directly to the Syzygy prober without re-parsing.
struct LegalityResult {
    /// The validated shakmaty position.
    shakmaty_pos: Chess,
}

/// Validate that a UCI move string is legal in standard chess using shakmaty.
///
/// Returns `Err(BinpackError::FrcCastle)` if the position cannot be parsed as
/// standard chess or the move is not legal.  This is the primary FRC filter —
/// any position or move that is illegal in standard chess is assumed to be an
/// FRC record that slipped through the classical input_format filter.
///
/// On success, returns `LegalityResult` carrying the parsed `Chess` position
/// so it can be reused for a Syzygy probe without parsing the FEN again.
fn check_legal_standard_chess(uci: &str, fen: &str) -> Result<LegalityResult, BinpackError> {
    let shakmaty_pos = Fen::from_ascii(fen.as_bytes())
        .ok()
        .and_then(|f| f.into_position::<Chess>(CastlingMode::Standard).ok());

    let pos = match shakmaty_pos {
        Some(p) => p,
        None => return Err(BinpackError::FrcCastle(format!(
            "position invalid in standard chess (FRC): {fen}"
        ))),
    };

    let is_legal = uci.parse::<ShakmatyUci>()
        .ok()
        .and_then(|m| m.to_move(&pos).ok())
        .is_some();

    if !is_legal {
        return Err(BinpackError::FrcCastle(format!(
            "move {uci} illegal in standard chess (FRC): {fen}"
        )));
    }

    Ok(LegalityResult { shakmaty_pos: pos })
}

// ── Entry construction ────────────────────────────────────────────────────────

/// A fully converted entry plus any Syzygy WDL result found at this position.
struct ConvertedEntry {
    entry:      TrainingDataEntry,
    /// STM-relative result from the tablebase, if a probe hit occurred.
    /// `None` means either no tables loaded, position had >7 pieces,
    /// or position was not found in the tablebase.
    tb_result:  Option<i16>,
}

/// Convert a single V6Record to a TrainingDataEntry, optionally probing
/// the Syzygy tablebase.
///
/// The FEN, move, and evaluation are all self-contained within one record.
/// When Black is to move the move index is in flipped coordinates and must
/// be mirrored vertically before being applied to the standard-orientation FEN.
///
/// `ply` is the caller-tracked position of this record within its game.
/// `prober` is the Syzygy tablebase prober; probe is skipped if `None` is
/// passed or if `prober.probe()` returns `None` (e.g. >7 pieces, no hit).
fn record_to_converted(
    rec:    &V6Record,
    ply:    u16,
    prober: &SyzygyProber,
) -> Result<ConvertedEntry, BinpackError> {
    let fen = record_to_fen(rec).map_err(BinpackError::Fen)?;
    let pos = Position::from_fen(&fen)
        .map_err(|e| BinpackError::InvalidPosition(format!("{e:?} for FEN: {fen}")))?;

    let best_idx = rec.best_idx as usize;
    if best_idx >= MOVE_STRS.len() {
        return Err(BinpackError::InvalidMove(format!(
            "best_idx {best_idx} out of range (max {})",
            MOVE_STRS.len() - 1
        )));
    }

    let black_to_move = pos.side_to_move() == Color::Black;
    let uci = if black_to_move {
        mirror_uci_move(MOVE_STRS[best_idx])
            .ok_or_else(|| BinpackError::InvalidMove(format!(
                "could not mirror move: {}", MOVE_STRS[best_idx]
            )))?
    } else {
        MOVE_STRS[best_idx].to_string()
    };

    // Legality check — also returns the shakmaty Chess position for TB probe.
    let legality = check_legal_standard_chess(&uci, &fen)?;

    // Syzygy probe — reuses the already-parsed shakmaty position.
    let tb_result = prober.probe(&fen, &legality.shakmaty_pos);

    let mv = parse_uci_move(&uci, &pos)?;

    let entry = TrainingDataEntry {
        pos,
        mv,
        score:  best_q_to_score(rec),
        ply,
        result: result_q_to_result(rec.result_q),
    };

    Ok(ConvertedEntry { entry, tb_result })
}

// ── Buffered game entry ───────────────────────────────────────────────────────

/// One position buffered for a game, ready to be patched and written.
struct BufferedEntry {
    entry:     TrainingDataEntry,
    /// STM-relative TB result if the probe hit at this position.
    tb_result: Option<i16>,
}

// ── Result propagation ────────────────────────────────────────────────────────

/// Patch result fields of buffered entries using the first TB hit.
///
/// Strategy:
/// 1. Find the first entry index where `tb_result` is `Some`.
/// 2. From that index forward (to end): set result to the TB value,
///    flipping sign for each ply step away from the hit.
/// 3. If `backpropagate`: from that index backward (to start): same logic.
///
/// The sign convention is STM-relative throughout.  At the hit ply the
/// TB result is correct for the STM at that ply.  One ply earlier the
/// opponent is to move, so the result flips.  Two plies earlier it is
/// the same side again, so it flips back.  In general:
///
///   result_at_ply_n = tb_result_at_hit * (-1)^(hit_ply - n)
///
/// Which in integer arithmetic is:
///   if (hit_ply - n) is even  → same sign as tb_result
///   if (hit_ply - n) is odd   → negated
fn propagate_tb_result(entries: &mut Vec<BufferedEntry>, backpropagate: bool) {
    // Find the first TB hit.
    let hit_idx = match entries.iter().position(|e| e.tb_result.is_some()) {
        Some(i) => i,
        None    => return,  // No TB hit in this game — nothing to do.
    };

    let tb_result = entries[hit_idx].tb_result.unwrap();

    // Propagate forward from the hit (inclusive) to end-of-game.
    for (offset, e) in entries[hit_idx..].iter_mut().enumerate() {
        // offset 0 → hit ply, sign unchanged.
        // offset 1 → one ply after hit, opponent STM, sign flipped.
        e.entry.result = if offset % 2 == 0 { tb_result } else { -tb_result };
    }

    // Propagate backward from the hit (exclusive) to start-of-game.
    if backpropagate && hit_idx > 0 {
        for (offset, e) in entries[..hit_idx].iter_mut().rev().enumerate() {
            // offset 0 → one ply before hit, sign flipped.
            // offset 1 → two plies before hit, sign unchanged.
            e.entry.result = if offset % 2 == 0 { -tb_result } else { tb_result };
        }
    }
}

// ── WDL counters ──────────────────────────────────────────────────────────────

/// Win / draw / loss counts for written entries.
///
/// Results are always STM-relative (+1 win, 0 draw, -1 loss).
#[derive(Default, Clone, Copy)]
pub struct WdlCounts {
    pub wins:   usize,
    pub draws:  usize,
    pub losses: usize,
}

impl WdlCounts {
    fn tally(&mut self, result: i16) {
        match result {
             1 => self.wins   += 1,
             0 => self.draws  += 1,
            -1 => self.losses += 1,
            _  => {}
        }
    }
}

// ── Writer ────────────────────────────────────────────────────────────────────

/// Writes V6Records to a `.binpack` file, appending if the file already exists.
///
/// Records are buffered per game (one `.gz` file = one game).  Call
/// `flush_game(backpropagate)` at the end of each game and `flush()` once
/// at the very end to finalise the stream.
pub struct BinpackWriter {
    inner:             Option<CompressedTrainingDataEntryWriter<BufWriter<File>>>,
    game_buf:          Vec<BufferedEntry>,
    written:           usize,
    skipped:           usize,
    tb_hits:           usize,
    /// WDL counts from the original lc0 result_q values (before any TB patch).
    pub wdl_original:  WdlCounts,
    /// WDL counts from the final result written to binpack (after TB patch).
    pub wdl_corrected: WdlCounts,
}

impl BinpackWriter {
    /// Create (or append to) a binpack file at the given path.
    pub fn create(path: &Path) -> Result<Self, BinpackError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        let inner = CompressedTrainingDataEntryWriter::new(BufWriter::new(file))
            .map_err(|e| BinpackError::Writer(e.to_string()))?;

        Ok(Self {
            inner:         Some(inner),
            game_buf:      Vec::new(),
            written:       0,
            skipped:       0,
            tb_hits:       0,
            wdl_original:  WdlCounts::default(),
            wdl_corrected: WdlCounts::default(),
        })
    }

    /// Buffer one record at the given game-relative `ply`.
    ///
    /// The Syzygy probe is skipped for this position if `prober` has no
    /// tables loaded, the position has >7 pieces, or the position is not
    /// in the tablebase.  Probing is also skipped once a TB hit has already
    /// been recorded in the current game buffer — pass `tb_hit_found` as
    /// `true` once the caller detects a hit to avoid further disk I/O.
    ///
    /// Returns:
    /// - `Ok((true, hit))`  — entry buffered; `hit` is `true` if this record
    ///                        produced the first TB hit in the current game.
    /// - `Ok((false, _))`  — record skipped due to recoverable conversion error.
    /// - `Err(_)`           — fatal IO / writer error.
    pub fn buffer_record(
        &mut self,
        rec:           &V6Record,
        ply:           u16,
        prober:        &SyzygyProber,
        tb_hit_found:  bool,
    ) -> Result<(bool, bool), BinpackError> {
        // Once we have a TB hit in this game we stop probing to avoid
        // unnecessary disk I/O.  Achieve this by passing a disabled prober.
        let effective_prober: &SyzygyProber;
        let disabled = SyzygyProber::disabled();
        if tb_hit_found {
            effective_prober = &disabled;
        } else {
            effective_prober = prober;
        }

        match record_to_converted(rec, ply, effective_prober) {
            Ok(converted) => {
                let new_hit = converted.tb_result.is_some();
                if new_hit {
                    self.tb_hits += 1;
                }
                self.game_buf.push(BufferedEntry {
                    entry:     converted.entry,
                    tb_result: converted.tb_result,
                });
                Ok((true, new_hit))
            }
            Err(e @ BinpackError::Io(_)) | Err(e @ BinpackError::Writer(_)) => Err(e),
            Err(_) => {
                self.skipped += 1;
                Ok((false, false))
            }
        }
    }

    /// Flush the current game buffer to the binpack stream.
    ///
    /// Applies Syzygy result propagation (forward always; backward if
    /// `backpropagate` is `true`), then writes all buffered entries.
    ///
    /// Tallies `wdl_original` from the pre-propagation results and
    /// `wdl_corrected` from the final written results.
    ///
    /// Call this once per game (i.e. once per `.gz` file).
    pub fn flush_game(&mut self, backpropagate: bool) -> Result<(), BinpackError> {
        // Snapshot original results before any TB patching.
        for e in &self.game_buf {
            self.wdl_original.tally(e.entry.result);
        }

        // Patch results using TB hits (if any).
        propagate_tb_result(&mut self.game_buf, backpropagate);

        let writer = self.inner.as_mut().expect("flush_game called after flush");
        for buffered in self.game_buf.drain(..) {
            // Tally corrected result (may be identical to original if no TB hit).
            self.wdl_corrected.tally(buffered.entry.result);
            writer
                .write_entry(&buffered.entry)
                .map_err(|e| BinpackError::Writer(e.to_string()))?;
            self.written += 1;
        }

        Ok(())
    }

    /// Flush and finalise the binpack stream.  Safe to call multiple times.
    pub fn flush(&mut self) {
        if let Some(mut inner) = self.inner.take() {
            inner.flush_and_end();
        }
    }

    pub fn written(&self)  -> usize { self.written  }
    pub fn skipped(&self)  -> usize { self.skipped  }
    pub fn tb_hits(&self)  -> usize { self.tb_hits  }
}

impl Drop for BinpackWriter {
    fn drop(&mut self) {
        self.flush();
    }
}