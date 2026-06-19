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

use crate::record::{V6Record, q_to_cp_str};
use crate::fen::record_to_fen;
use crate::moves::idx_to_move;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    Brief,
    Normal,
    Full,
}

pub fn print_record(
    index: usize,
    filename: &str,
    rec: &V6Record,
    verbosity: Verbosity,
) {
    match verbosity {
        Verbosity::Brief => print_brief(index, filename, rec),
        Verbosity::Normal => print_normal(index, filename, rec),
        Verbosity::Full => print_full(index, filename, rec),
    }
}

fn print_brief(index: usize, filename: &str, rec: &V6Record) {
    let (best_idx, best_prob) = rec.best_policy_move();
    let (ver, fmt, r50, vis) = (
        rec.version,
        rec.input_format,
        rec.rule50_count,
        rec.visits,
    );
    let (rq, rd, rm) = (rec.root_q, rec.root_d, rec.root_m);

    let fen = match record_to_fen(rec) {
        Ok(f) => f,
        Err(e) => format!("<FEN error: {e}>"),
    };

    println!(
        "[{index:>6}] {filename}  ver={ver} fmt={fmt} stm={stm} rule50={r50:>3} \
         visits={vis:>7} Q={rq:+.4} ({cp}) D={rd:.4} M={rm:.1} \
         best={best_move} ({bp:.2}%)  {fen}",
        stm = rec.side_to_move_str(),
        cp = q_to_cp_str(rq, false),
        best_move = idx_to_move(best_idx as u16),
        bp = best_prob * 100.0,
    );
}

fn print_normal(index: usize, filename: &str, rec: &V6Record) {
    let sep = "─".repeat(60);

    println!("\n┌{sep}");
    println!("│ Record #{index}");
    println!("│ File             : {filename}");
    println!("├{sep}");

    // FEN
    match record_to_fen(rec) {
        Ok(fen) => println!("│ FEN              : {fen}"),
        Err(e) => println!("│ FEN              : <error: {e}>"),
    }

    // Header
    let (ver, fmt, r50, inv, dum) = (
        rec.version,
        rec.input_format,
        rec.rule50_count,
        rec.invariance_info,
        rec.dummy,
    );

    println!("├{sep}");
    println!("│ Version          : {ver}");
    println!("│ Input format     : {fmt}");
    println!(
        "│ Side/EP          : {} (raw={})",
        rec.side_to_move_str(),
        rec.side_to_move_or_enpassant
    );
    println!("│ Castling         : {}", rec.castling_str());
    println!("│ Rule-50 clock    : {r50}");
    println!("│ Invariance info  : 0x{inv:02X}");
    println!(
        "│   side_to_move   = {}",
        rec.canonical_side_to_move() as u8
    );
    println!(
        "│   marked_del     = {}",
        rec.marked_for_deletion() as u8
    );
    println!(
        "│   adjudicated    = {}",
        rec.game_adjudicated() as u8
    );
    println!(
        "│   max_len_exc    = {}",
        rec.max_game_length_exceeded() as u8
    );
    println!(
        "│   best_proven    = {}",
        rec.best_q_is_proven() as u8
    );
    println!(
        "│   xform T/M/F    = {}/{}/{}",
        rec.transform_transpose() as u8,
        rec.transform_mirror() as u8,
        rec.transform_flip() as u8
    );
    println!("│ Dummy (v5 res)   : {dum}");

    // Value targets
    let proven = rec.best_q_is_proven();

    println!("├{sep}");
    println!("│ ── Value targets ──────────────────────────────────");

    let (rq, rd, bq, bd, oq, od) = (
        rec.root_q,
        rec.root_d,
        rec.best_q,
        rec.best_d,
        rec.orig_q,
        rec.orig_d,
    );
    let (rm, bm, om, pl) = (
        rec.root_m,
        rec.best_m,
        rec.orig_m,
        rec.plies_left,
    );
    let (rsq, rsd) = (rec.result_q, rec.result_d);

    println!(
        "│ Root  W/D/L      : {:.4} / {:.4} / {:.4}",
        rec.root_win_prob(),
        rd,
        rec.root_loss_prob()
    );
    println!(
        "│ Root  Q/D/M      : {rq:+.4} ({rcp}) / {rd:.4} / {rm:.1}",
        rcp = q_to_cp_str(rq, false)
    );
    println!(
        "│ Best  Q/D/M      : {bq:+.4} ({bcp}) / {bd:.4} / {bm:.1}",
        bcp = q_to_cp_str(bq, proven)
    );
    println!(
        "│ Orig  Q/D/M      : {oq:+.4} ({ocp}) / {od:.4} / {om:.1}  (NaN if not cached)",
        ocp = q_to_cp_str(oq, false)
    );
    println!(
        "│ Played Q/D/M     : {pq:+.4} ({pcp}) / {pd:.4} / {pm:.1}",
        pq = rec.played_q,
        pcp = q_to_cp_str(rec.played_q, false),
        pd = rec.played_d,
        pm = rec.played_m
    );
    println!("│ Result Q/D       : {rsq:+.4} / {rsd:.4}");
    println!("│ Plies left (MLH) : {pl:.1}");

    // Move info
    println!("├{sep}");
    println!("│ ── Move info ──────────────────────────────────────");

    let (vis, pidx, bidx) = (rec.visits, rec.played_idx, rec.best_idx);
    let (kld, res) = (rec.policy_kld, rec.reserved);

    println!("│ Visits           : {vis}");
    println!(
        "│ Played move      : {} (idx={pidx})",
        idx_to_move(pidx)
    );
    println!(
        "│ Best move        : {} (idx={bidx}){}",
        idx_to_move(bidx),
        if proven { "  [proven]" } else { "" }
    );
    println!("│ Policy KLD       : {kld:.6}");
    println!("│ Reserved         : {res}");

    // Policy summary
    let visited = rec.num_visited_moves();
    let (top_idx, top_prob) = rec.best_policy_move();

    println!("├{sep}");
    println!("│ ── Policy ({visited} visited moves) ─────────────────");
    println!(
        "│ Top move         : {} (idx={top_idx}) prob={top_prob:.4} ({:.2}%)",
        idx_to_move(top_idx as u16),
        top_prob * 100.0
    );

    let mut entries: Vec<(usize, f32)> = rec
        .probabilities
        .iter()
        .enumerate()
        .filter(|(_, p)| **p >= 0.0)
        .map(|(i, &p)| (i, p))
        .collect();

    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    println!("│ Top-5 moves:");
    for (rank, (idx, prob)) in entries.iter().take(5).enumerate() {
        println!(
            "│   #{}: {} (idx={idx:>4}) prob={prob:.4} ({:.2}%)",
            rank + 1,
            idx_to_move(*idx as u16),
            prob * 100.0
        );
    }

    println!("└{sep}");
}

fn print_full(index: usize, filename: &str, rec: &V6Record) {
    print_normal(index, filename, rec);

    println!(
        "\n  ── Full policy ({} entries) ──",
        rec.probabilities.len()
    );

    for (i, &p) in rec.probabilities.iter().enumerate() {
        if p >= 0.0 {
            println!(
                "    policy[{i:>4}] = {p:.6}  ({})",
                idx_to_move(i as u16)
            );
        }
    }

    println!("\n  ── Input planes (104 × u64 bitboards, non-zero only) ──");

    for (i, &plane) in rec.planes.iter().enumerate() {
        if plane != 0 {
            println!("    plane[{i:>3}] = {plane:#018X}");
            print_bitboard(plane);
        }
    }

    println!("  └{}", "─".repeat(60));
}

fn print_bitboard(bb: u64) {
    for rank in (0..8).rev() {
        print!("    ");
        for file in 0..8 {
            let sq = rank * 8 + file;
            if bb & (1u64 << sq) != 0 {
                print!("1 ");
            } else {
                print!(". ");
            }
        }
        println!("  (rank {})", rank + 1);
    }
}