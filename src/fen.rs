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


//! FEN generation from V6 training data planes.
//!
//! Key facts from lc0 source:
//!
//! - Planes are stored with ReverseBitsInBytes applied (reader.cc line:
//!     `plane = ReverseBitsInBytes(planes[plane_idx++].mask)`)
//!   So we must reverse bits-in-bytes to recover the original bitboard.
//!
//! - The 104 planes are 8 history slots × 13 planes each. Slot 0 (planes 0–12)
//!   is the most recent position — the one we want for FEN.
//!
//! - Plane layout per slot (encoder.h / encoder.cc):
//!     0  our pawns       6  their pawns
//!     1  our knights     7  their knights
//!     2  our bishops     8  their bishops
//!     3  our rooks       9  their rooks
//!     4  our queens      10 their queens
//!     5  our kings       11 their kings
//!                        12 repetition flag (all-ones or all-zeros)
//!
//! - "Our" = side to move. For classical format: side_to_move_or_enpassant==1
//!   means Black to move, so "ours" are Black's pieces.
//!   For canonical formats the side is in invariance_info bit 7.
//!
//! - When Black is to move the board is stored mirrored (flipped vertically),
//!   so rank 1 in the bitboard corresponds to Black's back rank. We flip back
//!   before building the FEN so squares are always from White's perspective.
//!
//! - Castling: for classical format the castling bytes are 0 or 1 flags.
//!   However, some FRC positions slip through with input_format==1. We
//!   validate the king and rook are on their classical squares before emitting
//!   a castling right; inconsistent rights are silently dropped so the FEN
//!   remains self-consistent.
//!
//! ## En passant encoding
//!
//! **Canonical formats** (`input_format != 1`):
//!   `side_to_move_or_enpassant` encodes the ep column as a bitmask
//!   (`1 << file`). Side-to-move is in `invariance_info` bit 7.
//!
//! **Classical format** (`input_format == 1`):
//!   `side_to_move_or_enpassant` is purely a side-to-move flag (0=White,
//!   non-zero=Black). En passant is encoded inside the pawn planes using
//!   *phantom pawns* on ranks 1 and 8 of the side-to-move frame
//!   (lc0 `board.h`: "A pawn on rank 1 in their_pieces means the
//!   corresponding white pawn on rank 4 can be taken en passant. Rank 8
//!   is the same for black pawns.").
//!
//!   Concretely: plane 6 (`their_pawns`) may have bits set in rank 1
//!   (bits 0..7, squares a1..h1 in the side-to-move frame).  These are
//!   *not* real pawns — they are ep markers.  The file of the set bit
//!   gives the ep file; the ep target rank in standard (White-at-bottom)
//!   coordinates is rank 6 when White is to move, rank 3 when Black is to
//!   move.
//!
//!   We must read the phantom before the vertical-flip step, because the
//!   flip moves those bits off rank 1.

use crate::record::V6Record;

const INPUT_CLASSICAL: u32 = 1;

// ── Bit manipulation ──────────────────────────────────────────────────────────

/// Undo the ReverseBitsInBytes transform applied when writing planes.
/// Swaps bit order within each byte (mirrors each rank).
fn reverse_bits_in_bytes(v: u64) -> u64 {
    let mut result = 0u64;
    for byte_idx in 0..8 {
        let byte = (v >> (byte_idx * 8)) as u8;
        result |= (byte.reverse_bits() as u64) << (byte_idx * 8);
    }
    result
}

/// Flip the board vertically (swap rank 1 ↔ rank 8).
/// Used to convert from Black-to-move perspective back to White's perspective.
fn flip_vertical(v: u64) -> u64 {
    v.swap_bytes()
}

// ── Plane decoding ────────────────────────────────────────────────────────────

/// Decode one plane from its stored form to a standard bitboard
/// (bit N = square N, a1=0, h1=7, a8=56, h8=63 in the side-to-move frame).
fn decode_plane(raw: u64) -> u64 {
    reverse_bits_in_bytes(raw)
}

// ── En passant helpers ────────────────────────────────────────────────────────

const RANK_1_MASK: u64 = 0x0000_0000_0000_00FF; // squares 0..7

/// Extract the classical-format ep square from the raw (pre-flip) pawn planes.
///
/// Plane 6 ("their pawns" in side-to-move frame) may contain a phantom pawn
/// on rank 1 (bits 0..7) indicating which file the ep target is on.
/// Returns the FEN ep square string (e.g. "h6") or "-".
fn classical_ep(rec: &V6Record, black_to_move: bool) -> String {
    // Decode plane 6 BEFORE flipping so phantom bits are still on rank 1.
    let their_pawns_decoded = decode_plane(rec.planes[6]);
    let ep_bits = their_pawns_decoded & RANK_1_MASK;

    if ep_bits == 0 {
        return "-".to_string();
    }

    let file = ep_bits.trailing_zeros() as usize;
    if file >= 8 {
        return "-".to_string();
    }

    // In side-to-move frame, rank-1 phantom → ep target square:
    //   White to move: opponent's pawn just pushed to rank 5 (their rank 4),
    //                  ep capture lands on rank 6 in standard coords.
    //   Black to move: board is flipped; opponent's pawn is on rank 4 (std),
    //                  ep capture lands on rank 3 in standard coords.
    let ep_rank = if black_to_move { '3' } else { '6' };
    format!("{}{}", (b'a' + file as u8) as char, ep_rank)
}

// ── Piece representation ──────────────────────────────────────────────────────

#[derive(Copy, Clone, PartialEq)]
enum Piece {
    WP, WN, WB, WR, WQ, WK,
    BP, BN, BB, BR, BQ, BK,
}

impl Piece {
    fn to_char(self) -> char {
        match self {
            Piece::WP => 'P', Piece::WN => 'N', Piece::WB => 'B',
            Piece::WR => 'R', Piece::WQ => 'Q', Piece::WK => 'K',
            Piece::BP => 'p', Piece::BN => 'n', Piece::BB => 'b',
            Piece::BR => 'r', Piece::BQ => 'q', Piece::BK => 'k',
        }
    }
}

// ── FEN builder ───────────────────────────────────────────────────────────────

/// Generate a FEN string from a V6Record.
///
/// Returns `Err` with a description if the planes look invalid
/// (e.g. no kings found).
pub fn record_to_fen(rec: &V6Record) -> Result<String, String> {
    let is_canonical = rec.input_format != INPUT_CLASSICAL;

    // ── Side to move ─────────────────────────────────────────────────────────
    let black_to_move = if is_canonical {
        rec.canonical_side_to_move()
    } else {
        rec.side_to_move_or_enpassant != 0
    };

    // ── En passant (classical only — must happen BEFORE flip) ─────────────────
    // Canonical ep is derived from side_to_move_or_enpassant below.
    let ep_classical = if !is_canonical {
        classical_ep(rec, black_to_move)
    } else {
        String::new() // unused for canonical
    };

    // ── Decode piece planes (slot 0 = most recent position) ───────────────────
    // Apply inverse of ReverseBitsInBytes.
    let mut our   = [0u64; 6]; // P N B R Q K
    let mut their = [0u64; 6];
    for i in 0..6 {
        our[i]   = decode_plane(rec.planes[i]);
        their[i] = decode_plane(rec.planes[i + 6]);
    }

    // ── Strip phantom ep pawns from their[0] before use ──────────────────────
    // Rank-1 bits in their_pawns are ep markers, not real pawns.
    // Remove them so they don't appear on the board.
    their[0] &= !RANK_1_MASK;

    // ── Flip to White-perspective if Black is to move ─────────────────────────
    if black_to_move {
        for bb in our.iter_mut().chain(their.iter_mut()) {
            *bb = flip_vertical(*bb);
        }
    }

    // ── Build 8×8 board (square 0=a1, 63=h8) ─────────────────────────────────
    let mut board = [None::<Piece>; 64];

    let mut place = |bb: u64, piece: Piece| {
        let mut b = bb;
        while b != 0 {
            let sq = b.trailing_zeros() as usize;
            board[sq] = Some(piece);
            b &= b - 1;
        }
    };

    if black_to_move {
        // our[] = Black's pieces (flipped to white-perspective squares)
        // their[] = White's pieces
        place(our[0],   Piece::BP);
        place(our[1],   Piece::BN);
        place(our[2],   Piece::BB);
        place(our[3],   Piece::BR);
        place(our[4],   Piece::BQ);
        place(our[5],   Piece::BK);
        place(their[0], Piece::WP);
        place(their[1], Piece::WN);
        place(their[2], Piece::WB);
        place(their[3], Piece::WR);
        place(their[4], Piece::WQ);
        place(their[5], Piece::WK);
    } else {
        // our[] = White's pieces, their[] = Black's pieces
        place(our[0],   Piece::WP);
        place(our[1],   Piece::WN);
        place(our[2],   Piece::WB);
        place(our[3],   Piece::WR);
        place(our[4],   Piece::WQ);
        place(our[5],   Piece::WK);
        place(their[0], Piece::BP);
        place(their[1], Piece::BN);
        place(their[2], Piece::BB);
        place(their[3], Piece::BR);
        place(their[4], Piece::BQ);
        place(their[5], Piece::BK);
    }

    // ── Sanity check: exactly one king per side ───────────────────────────────
    let wk = board.iter().filter(|&&p| p == Some(Piece::WK)).count();
    let bk = board.iter().filter(|&&p| p == Some(Piece::BK)).count();
    if wk != 1 || bk != 1 {
        return Err(format!("Invalid position: {wk} white kings, {bk} black kings"));
    }

    // ── Piece placement string (ranks 8 → 1) ──────────────────────────────────
    let mut placement = String::new();
    for rank in (0..8).rev() {
        let mut empty = 0u8;
        for file in 0..8 {
            let sq = rank * 8 + file;
            match board[sq] {
                None => empty += 1,
                Some(p) => {
                    if empty > 0 {
                        placement.push(char::from_digit(empty as u32, 10).unwrap());
                        empty = 0;
                    }
                    placement.push(p.to_char());
                }
            }
        }
        if empty > 0 {
            placement.push(char::from_digit(empty as u32, 10).unwrap());
        }
        if rank > 0 {
            placement.push('/');
        }
    }

    // ── Side to move ─────────────────────────────────────────────────────────
    let stm = if black_to_move { 'b' } else { 'w' };

    // ── Castling rights ───────────────────────────────────────────────────────
    // Always emitted in standard FEN order: K Q k q.
    // When Black is to move, "us" = Black and "them" = White.
    //
    // Geometry check: classical-format records occasionally contain FRC
    // positions that slipped through the input_format filter (the king is not
    // on e1/e8 or the rook is not in the corner). Emitting castling rights for
    // such positions produces an invalid standard FEN. We silently drop any
    // right whose piece geometry is inconsistent; the record is then exported
    // without those rights rather than being skipped entirely.
    let (white_oo, white_ooo, black_oo, black_ooo) = if black_to_move {
        (rec.castling_them_oo,  rec.castling_them_ooo,
         rec.castling_us_oo,    rec.castling_us_ooo)
    } else {
        (rec.castling_us_oo,    rec.castling_us_ooo,
         rec.castling_them_oo,  rec.castling_them_ooo)
    };

    // Square indices (White-perspective): e1=4, a1=0, h1=7, e8=60, a8=56, h8=63
    let white_king_on_e1 = board[4]  == Some(Piece::WK);
    let black_king_on_e8 = board[60] == Some(Piece::BK);
    let white_rook_on_h1 = board[7]  == Some(Piece::WR);
    let white_rook_on_a1 = board[0]  == Some(Piece::WR);
    let black_rook_on_h8 = board[63] == Some(Piece::BR);
    let black_rook_on_a8 = board[56] == Some(Piece::BR);

    let mut castling = String::new();
    if white_oo  != 0 && white_king_on_e1 && white_rook_on_h1 { castling.push('K'); }
    if white_ooo != 0 && white_king_on_e1 && white_rook_on_a1 { castling.push('Q'); }
    if black_oo  != 0 && black_king_on_e8 && black_rook_on_h8 { castling.push('k'); }
    if black_ooo != 0 && black_king_on_e8 && black_rook_on_a8 { castling.push('q'); }
    if castling.is_empty() { castling.push('-'); }

    // ── En passant ────────────────────────────────────────────────────────────
    let ep = if is_canonical {
        // Canonical: ep column encoded as bitmask in side_to_move_or_enpassant.
        if rec.side_to_move_or_enpassant != 0 {
            let ep_byte = if rec.transform_flip() {
                rec.side_to_move_or_enpassant.reverse_bits()
            } else {
                rec.side_to_move_or_enpassant
            };
            let file = ep_byte.trailing_zeros() as usize;
            if file < 8 {
                let ep_rank = if black_to_move { '3' } else { '6' };
                format!("{}{}", (b'a' + file as u8) as char, ep_rank)
            } else {
                "-".to_string()
            }
        } else {
            "-".to_string()
        }
    } else {
        // Classical: recovered from phantom pawns in plane 6 (see module docs).
        ep_classical
    };

    // ── Halfmove clock & fullmove number ─────────────────────────────────────
    let halfmove = rec.rule50_count;
    let fullmove = 1u32;

    Ok(format!("{placement} {stm} {castling} {ep} {halfmove} {fullmove}"))
}