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
//!   best_q -> centipawns via q_to_cp().  Proven mates: ±30000 cp.
//!
//! ## Result
//!   Derived from result_q (+1 win, 0 draw, -1 loss, side-to-move perspective).
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
    // King moves more than one file: could be a standard castle or an FRC
    // castle that slipped through the classical filter.
    //
    // We require the king to be on its classical starting square (e1/e8).
    // If it isn't, this is an FRC position — skip the record.
    // If it is, we check castling rights: present → standard castle encoded
    // as king-captures-own-rook; absent → FRC position, skip.
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
    // A pawn moving diagonally to an empty square is en passant IFF the
    // destination matches the position's ep square.  The ep square in `pos`
    // comes from the FEN produced by `record_to_fen`, which correctly recovers
    // the ep square from lc0's phantom-pawn encoding in plane 6 for classical
    // format.  If there is no ep square the move is illegal — return an error
    // so the record is skipped rather than written with a bad move type.
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
/// Saturates at ±30 000 cp (exact ±1.0 is only reachable for proven mates).
fn best_q_to_score(rec: &V6Record) -> i16 {
    let q = rec.best_q;
    if q >=  1.0 { return  30_000; }
    if q <= -1.0 { return -30_000; }
    q_to_cp(q)
        .map(|cp| cp.clamp(-30_000, 30_000) as i16)
        .unwrap_or(0)
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

// ── Shakmaty legality check ───────────────────────────────────────────────────

/// Validate that a UCI move string is legal in standard chess using shakmaty.
///
/// Returns `Err(BinpackError::FrcCastle)` if the position cannot be parsed as
/// standard chess or the move is not legal.  This is the primary FRC filter —
/// any position or move that is illegal in standard chess is assumed to be an
/// FRC record that slipped through the classical input_format filter.
fn check_legal_standard_chess(uci: &str, fen: &str) -> Result<(), BinpackError> {
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

    Ok(())
}

// ── Entry construction ────────────────────────────────────────────────────────

/// Convert a single V6Record to a TrainingDataEntry.
///
/// The FEN, move, and evaluation are all self-contained within one record.
/// When Black is to move the move index is in flipped coordinates and must
/// be mirrored vertically before being applied to the standard-orientation FEN.
pub fn record_to_entry(rec: &V6Record) -> Result<TrainingDataEntry, BinpackError> {
    // Build FEN — this correctly encodes the ep square for classical format
    // and drops geometrically inconsistent castling rights.
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

    // lc0 policy indices are in the side-to-move frame (board flipped when
    // Black to move).  Mirror squares back to standard coords for Black.
    let black_to_move = pos.side_to_move() == Color::Black;
    let uci = if black_to_move {
        mirror_uci_move(MOVE_STRS[best_idx])
            .ok_or_else(|| BinpackError::InvalidMove(format!(
                "could not mirror move: {}", MOVE_STRS[best_idx]
            )))?
    } else {
        MOVE_STRS[best_idx].to_string()
    };

    // Validate legality in standard chess via shakmaty.  This is the primary
    // FRC filter — it catches all FRC positions regardless of whether castling
    // is involved or how far pieces have moved from their starting squares.
    check_legal_standard_chess(&uci, &fen)?;

    let mv = parse_uci_move(&uci, &pos)?;

    Ok(TrainingDataEntry {
        pos,
        mv,
        score:  best_q_to_score(rec),
        ply:    0,
        result: result_q_to_result(rec.result_q),
    })
}

// ── Writer ────────────────────────────────────────────────────────────────────

/// Writes V6Records to a `.binpack` file, appending if the file already exists.
pub struct BinpackWriter {
    inner:   Option<CompressedTrainingDataEntryWriter<BufWriter<File>>>,
    written: usize,
    skipped: usize,
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

        Ok(Self { inner: Some(inner), written: 0, skipped: 0 })
    }

    /// Write one record.
    ///
    /// Returns `Ok(true)` on success, `Ok(false)` if the record was skipped
    /// due to a recoverable conversion error (bad FEN, illegal move, FRC
    /// position, etc.).  Fatal IO/writer errors are propagated.
    pub fn write_record(&mut self, rec: &V6Record) -> Result<bool, BinpackError> {
        match record_to_entry(rec) {
            Ok(entry) => {
                self.inner
                    .as_mut()
                    .expect("write_record called after flush")
                    .write_entry(&entry)
                    .map_err(|e| BinpackError::Writer(e.to_string()))?;
                self.written += 1;
                Ok(true)
            }
            Err(e @ BinpackError::Io(_)) | Err(e @ BinpackError::Writer(_)) => Err(e),
            Err(_) => {
                self.skipped += 1;
                Ok(false)
            }
        }
    }
    /// Flush and finalise the binpack stream.  Safe to call multiple times.
    pub fn flush(&mut self) {
        if let Some(mut inner) = self.inner.take() {
            inner.flush_and_end();
        }
    }

    pub fn written(&self) -> usize { self.written }
    pub fn skipped(&self) -> usize { self.skipped }
}

impl Drop for BinpackWriter {
    fn drop(&mut self) {
        self.flush();
    }
}