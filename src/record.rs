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


//! V6 training record — parsed from raw bytes at exact offsets.
//!
//! Source of truth: lc0/src/trainingdata/trainingdata.h
//! static_assert(sizeof(V6TrainingData) == 8356)
//!
//! Offset map:
//!      0  version                   u32
//!      4  input_format              u32
//!      8  probabilities             f32 × 1858  (7432 bytes)
//!   7440  planes                    u64 × 104   ( 832 bytes)
//!   8272  castling_us_ooo           u8
//!   8273  castling_us_oo            u8
//!   8274  castling_them_ooo         u8
//!   8275  castling_them_oo          u8
//!   8276  side_to_move_or_enpassant u8
//!   8277  rule50_count              u8
//!   8278  invariance_info           u8   (bitfield — see below)
//!   8279  dummy                     u8   (was result i8 in v5, zero in v6)
//!   8280  root_q                    f32
//!   8284  best_q                    f32
//!   8288  root_d                    f32
//!   8292  best_d                    f32
//!   8296  root_m                    f32
//!   8300  best_m                    f32
//!   8304  plies_left                f32  (MLH target)
//!   8308  result_q                  f32
//!   8312  result_d                  f32
//!   8316  played_q                  f32
//!   8320  played_d                  f32
//!   8324  played_m                  f32
//!   8328  orig_q                    f32  (may be NaN)
//!   8332  orig_d                    f32  (may be NaN)
//!   8336  orig_m                    f32  (may be NaN)
//!   8340  visits                    u32
//!   8344  played_idx                u16
//!   8346  best_idx                  u16
//!   8348  policy_kld                f32
//!   8352  reserved                  u32
//!   8356  <end>
//!
//! invariance_info bitfield:
//!   bit 7: side to move (canonical input formats)
//!   bit 6: position marked for deletion by rescorer
//!   bit 5: game adjudicated
//!   bit 4: max game length exceeded
//!   bit 3: best_q is for proven best move
//!   bit 2: transpose transform
//!   bit 1: mirror transform
//!   bit 0: flip transform

pub const RECORD_SIZE: usize = 8356;

// ── Byte offsets ──────────────────────────────────────────────────────────────
const OFF_VERSION:          usize = 0;
const OFF_INPUT_FORMAT:     usize = 4;
const OFF_PROBABILITIES:    usize = 8;
const OFF_PLANES:           usize = 7440;
const OFF_CAST_US_OOO:      usize = 8272;
const OFF_CAST_US_OO:       usize = 8273;
const OFF_CAST_THEM_OOO:    usize = 8274;
const OFF_CAST_THEM_OO:     usize = 8275;
const OFF_SIDE_OR_EP:       usize = 8276;
const OFF_RULE50:           usize = 8277;
const OFF_INVARIANCE:       usize = 8278;
const OFF_DUMMY:            usize = 8279;
const OFF_ROOT_Q:           usize = 8280;
const OFF_BEST_Q:           usize = 8284;
const OFF_ROOT_D:           usize = 8288;
const OFF_BEST_D:           usize = 8292;
const OFF_ROOT_M:           usize = 8296;
const OFF_BEST_M:           usize = 8300;
const OFF_PLIES_LEFT:       usize = 8304;
const OFF_RESULT_Q:         usize = 8308;
const OFF_RESULT_D:         usize = 8312;
const OFF_PLAYED_Q:         usize = 8316;
const OFF_PLAYED_D:         usize = 8320;
const OFF_PLAYED_M:         usize = 8324;
const OFF_ORIG_Q:           usize = 8328;
const OFF_ORIG_D:           usize = 8332;
const OFF_ORIG_M:           usize = 8336;
const OFF_VISITS:           usize = 8340;
const OFF_PLAYED_IDX:       usize = 8344;
const OFF_BEST_IDX:         usize = 8346;
const OFF_POLICY_KLD:       usize = 8348;
const OFF_RESERVED:         usize = 8352;

// ── Little-endian helpers ─────────────────────────────────────────────────────

#[inline] fn ru8 (b: &[u8], o: usize) -> u8  { b[o] }
#[inline] fn ru16(b: &[u8], o: usize) -> u16 { u16::from_le_bytes(b[o..o+2].try_into().unwrap()) }
#[inline] fn ru32(b: &[u8], o: usize) -> u32 { u32::from_le_bytes(b[o..o+4].try_into().unwrap()) }
#[inline] fn ru64(b: &[u8], o: usize) -> u64 { u64::from_le_bytes(b[o..o+8].try_into().unwrap()) }
#[inline] fn rf32(b: &[u8], o: usize) -> f32 { f32::from_le_bytes(b[o..o+4].try_into().unwrap()) }

// ── Record struct ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct V6Record {
    pub version:                   u32,
    pub input_format:              u32,

    /// Policy probabilities for all 1858 lc0 moves. -1.0 = unvisited.
    pub probabilities:             Box<[f32; 1858]>,
    /// Input board planes as 64-bit bitboards (bit-reversed bytes vs standard).
    pub planes:                    Box<[u64; 104]>,

    pub castling_us_ooo:           u8,
    pub castling_us_oo:            u8,
    pub castling_them_ooo:         u8,
    pub castling_them_oo:          u8,
    /// Canonical formats: en-passant column mask. Classical: 0=white, 1=black.
    pub side_to_move_or_enpassant: u8,
    pub rule50_count:              u8,
    /// Bitfield — see module docs.
    pub invariance_info:           u8,
    /// Zero in V6 (was signed result byte in V5).
    pub dummy:                     u8,

    pub root_q:    f32,
    pub best_q:    f32,
    pub root_d:    f32,
    pub best_d:    f32,
    pub root_m:    f32,
    pub best_m:    f32,
    /// MLH training target (plies remaining).
    pub plies_left: f32,
    pub result_q:  f32,
    pub result_d:  f32,
    pub played_q:  f32,
    pub played_d:  f32,
    pub played_m:  f32,
    /// May be NaN if not found in cache.
    pub orig_q:    f32,
    pub orig_d:    f32,
    pub orig_m:    f32,

    pub visits:     u32,
    pub played_idx: u16,
    pub best_idx:   u16,
    /// KL-divergence between visit distribution and policy.
    pub policy_kld: f32,
    pub reserved:   u32,
}

impl V6Record {
    /// Parse one record from a byte slice of at least `RECORD_SIZE` bytes.
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() < RECORD_SIZE { return None; }

        let mut probabilities = Box::new([0f32; 1858]);
        for i in 0..1858 {
            probabilities[i] = rf32(b, OFF_PROBABILITIES + i * 4);
        }

        let mut planes = Box::new([0u64; 104]);
        for i in 0..104 {
            planes[i] = ru64(b, OFF_PLANES + i * 8);
        }

        Some(V6Record {
            version:                   ru32(b, OFF_VERSION),
            input_format:              ru32(b, OFF_INPUT_FORMAT),
            probabilities,
            planes,
            castling_us_ooo:           ru8(b, OFF_CAST_US_OOO),
            castling_us_oo:            ru8(b, OFF_CAST_US_OO),
            castling_them_ooo:         ru8(b, OFF_CAST_THEM_OOO),
            castling_them_oo:          ru8(b, OFF_CAST_THEM_OO),
            side_to_move_or_enpassant: ru8(b, OFF_SIDE_OR_EP),
            rule50_count:              ru8(b, OFF_RULE50),
            invariance_info:           ru8(b, OFF_INVARIANCE),
            dummy:                     ru8(b, OFF_DUMMY),
            root_q:    rf32(b, OFF_ROOT_Q),
            best_q:    rf32(b, OFF_BEST_Q),
            root_d:    rf32(b, OFF_ROOT_D),
            best_d:    rf32(b, OFF_BEST_D),
            root_m:    rf32(b, OFF_ROOT_M),
            best_m:    rf32(b, OFF_BEST_M),
            plies_left: rf32(b, OFF_PLIES_LEFT),
            result_q:  rf32(b, OFF_RESULT_Q),
            result_d:  rf32(b, OFF_RESULT_D),
            played_q:  rf32(b, OFF_PLAYED_Q),
            played_d:  rf32(b, OFF_PLAYED_D),
            played_m:  rf32(b, OFF_PLAYED_M),
            orig_q:    rf32(b, OFF_ORIG_Q),
            orig_d:    rf32(b, OFF_ORIG_D),
            orig_m:    rf32(b, OFF_ORIG_M),
            visits:     ru32(b, OFF_VISITS),
            played_idx: ru16(b, OFF_PLAYED_IDX),
            best_idx:   ru16(b, OFF_BEST_IDX),
            policy_kld: rf32(b, OFF_POLICY_KLD),
            reserved:   ru32(b, OFF_RESERVED),
        })
    }

    // ── invariance_info bit accessors ────────────────────────────────────────

    pub fn canonical_side_to_move(&self) -> bool  { self.invariance_info & (1 << 7) != 0 }
    pub fn marked_for_deletion(&self)    -> bool  { self.invariance_info & (1 << 6) != 0 }
    pub fn game_adjudicated(&self)       -> bool  { self.invariance_info & (1 << 5) != 0 }
    pub fn max_game_length_exceeded(&self)-> bool { self.invariance_info & (1 << 4) != 0 }
    pub fn best_q_is_proven(&self)       -> bool  { self.invariance_info & (1 << 3) != 0 }
    pub fn transform_transpose(&self)    -> bool  { self.invariance_info & (1 << 2) != 0 }
    pub fn transform_mirror(&self)       -> bool  { self.invariance_info & (1 << 1) != 0 }
    pub fn transform_flip(&self)         -> bool  { self.invariance_info & (1 << 0) != 0 }

    // ── Derived helpers ──────────────────────────────────────────────────────

    /// Win probability: W = (1 + Q - D) / 2
    pub fn root_win_prob(&self) -> f32 { (1.0 + self.root_q - self.root_d) / 2.0 }

    /// Loss probability: L = (1 - Q - D) / 2
    pub fn root_loss_prob(&self) -> f32 { (1.0 - self.root_q - self.root_d) / 2.0 }

    /// Number of visited (non-negative) policy moves.
    pub fn num_visited_moves(&self) -> usize {
        self.probabilities.iter().filter(|p| **p >= 0.0).count()
    }

    /// (index, probability) of the highest-probability policy move.
    pub fn best_policy_move(&self) -> (usize, f32) {
        self.probabilities
            .iter()
            .enumerate()
            .filter(|(_, p)| **p >= 0.0)
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(i, &p)| (i, p))
            .unwrap_or((0, 0.0))
    }

    pub fn side_to_move_str(&self) -> &'static str {
        if self.side_to_move_or_enpassant == 0 { "White" } else { "Black" }
    }

    pub fn castling_str(&self) -> String {
        let mut s = String::new();
        if self.castling_us_oo    != 0 { s.push('K'); }
        if self.castling_us_ooo   != 0 { s.push('Q'); }
        if self.castling_them_oo  != 0 { s.push('k'); }
        if self.castling_them_ooo != 0 { s.push('q'); }
        if s.is_empty() { s.push('-'); }
        s
    }
}

// ── Q ↔ centipawn conversion ──────────────────────────────────────────────────

/// Convert a Q value in [-1.0, 1.0] to centipawns using the lc0 formula:
///   cp = 660.6 * q / (1.0 - 0.9751875 * q^10), clamped to [-32000, 32000]
///
/// Returns None if Q is NaN.
pub fn q_to_cp(q: f32) -> Option<i16> {
    if q.is_nan() {
        return None;
    }
    let q = q as f64;
    let cp = 660.6 * q / (1.0 - 0.9751875 * q.powi(10));
    let cp = cp.clamp(-32000.0, 32000.0);
    Some(cp.round() as i16)
}

/// Format a Q value as a centipawn string, e.g. "+42cp" or "±M" for proven mates.
pub fn q_to_cp_str(q: f32, is_proven: bool) -> String {
    if q.is_nan() {
        return "N/A".to_string();
    }
    if q >= 1.0  { return if is_proven { "+M".to_string() } else { "+32000cp".to_string() }; }
    if q <= -1.0 { return if is_proven { "-M".to_string() } else { "-32000cp".to_string() }; }
    match q_to_cp(q) {
        Some(cp) => format!("{cp:+}cp"),
        None     => "N/A".to_string(),
    }
}