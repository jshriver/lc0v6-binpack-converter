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
mod binpack;
mod fen;
mod moves;
mod printer;
mod progress;
mod record;
mod syzygy;

use record::{V6Record, RECORD_SIZE};
use printer::{Verbosity, print_record};
use binpack::{BackpropMode, BinpackWriter};
use progress::Progress;
use syzygy::{SyzygyProber, TableInventory};

use std::{
    collections::HashMap,
    env,
    fs::File,
    io::{self, BufReader, Read},
    path::PathBuf,
    process,
};

use flate2::read::GzDecoder;

const INPUT_CLASSICAL: u32 = 1;
const PROGRESS_INTERVAL: usize = 1000;

// ── Args ──────────────────────────────────────────────────────────────────────

struct Args {
    files:         Vec<PathBuf>,
    verbosity:     Option<Verbosity>,
    limit:         Option<usize>,
    skip:          usize,
    summary:       bool,
    output:        Option<PathBuf>,
    syzygy_path:   Option<PathBuf>,
    /// Resolved once during arg parsing from `--backpropagate` and
    /// `--backpropagate-limit` — see `resolve_backprop_mode` for the
    /// precedence rule between the two flags.
    backprop_mode: BackpropMode,
    _input_dir:    Option<PathBuf>,
}

fn usage(prog: &str) -> ! {
    eprintln!(
        "Usage: {prog} [OPTIONS] [<file> ...]\n\
         \n\
         OPTIONS:\n\
           -b, --brief                One-line summary per record\n\
           -n, --normal               Key fields per record\n\
           -f, --full                 All fields including policy and planes\n\
           -l, --limit N              Only process the first N records\n\
           -s, --skip N               Skip the first N records\n\
           --summary                  Print aggregate statistics at the end\n\
           -o, --output <file>        Export to .binpack file\n\
           -d, --input-dir <dir>      Process all .gz files in a directory.\n\
                                      Avoids shell glob limits on Windows/Linux.\n\
                                      Can be combined with explicit file args.\n\
           --syzygy-path <paths>      Colon-separated (semicolon on Windows) list\n\
                                      of Syzygy tablebase directories.\n\
                                      Positions with ≤7 pieces are probed; the\n\
                                      first hit overrides result_q and is\n\
                                      propagated forward through the game.\n\
           --backpropagate            Also propagate the first Syzygy hit\n\
                                      backward to move 1 (requires --syzygy-path).\n\
                                      Without this flag only forward propagation\n\
                                      (hit ply → end of game) is applied.\n\
           --backpropagate-limit N    Like --backpropagate, but only patches the\n\
                                      N plies immediately before the hit instead\n\
                                      of going all the way back to move 1. Implies\n\
                                      --backpropagate is enabled on its own — you\n\
                                      don't need both. If both are given, the\n\
                                      limit wins (backprop is capped at N plies).\n\
                                      Bounds the assumption that the game's result\n\
                                      was already decided arbitrarily far before\n\
                                      the TB hit, which becomes less reliable the\n\
                                      further back you go.\n\
           -h, --help                 Show this help\n\
         \n\
         Without -b/-n/-f, shows a live progress bar instead of per-record output.\n\
         Both raw binary and gzip-compressed (.gz) files are supported.\n\
         Non-classical (FRC/variant) records are always skipped.\n\
         \n\
         EXAMPLES:\n\
           # Process a whole directory (no glob needed)\n\
           {prog} -d /data/training -o out.binpack\n\
           # Windows - no more PowerShell glob workarounds\n\
           {prog} -d C:\\training\\run1 -o out.binpack\n\
           # Export with Syzygy result correction\n\
           {prog} -d /data/training -o out.binpack --syzygy-path /tb/syzygy\n\
           # Export with full back+forward propagation\n\
           {prog} -d /data/training -o out.binpack --syzygy-path /tb/syzygy --backpropagate\n\
           # Export with backprop capped at 30 plies before each TB hit\n\
           {prog} -d /data/training -o out.binpack --syzygy-path /tb/syzygy --backpropagate-limit 30\n\
           # Inspect first 10 records of a single file\n\
           {prog} --normal --limit 10 game.gz\n\
           # Mix directory and explicit files\n\
           {prog} -d /data/training extra1.gz extra2.gz -o out.binpack"
    );
    process::exit(1);
}

fn parse_args() -> Args {
    let raw: Vec<String> = env::args().collect();
    let prog = raw.first().map(String::as_str).unwrap_or("lc0_parser");

    let mut files             = Vec::new();
    let mut verbosity         = None;
    let mut limit             = None;
    let mut skip              = 0usize;
    let mut summary           = false;
    let mut output            = None;
    let mut syzygy_path       = None;
    let mut backpropagate     = false;
    let mut backpropagate_lim: Option<u16> = None;
    let mut input_dir         = None;

    let mut i = 1;
    while i < raw.len() {
        match raw[i].as_str() {
            "-h" | "--help"   => usage(prog),
            "-b" | "--brief"  => verbosity = Some(Verbosity::Brief),
            "-n" | "--normal" => verbosity = Some(Verbosity::Normal),
            "-f" | "--full"   => verbosity = Some(Verbosity::Full),
            "--summary"       => summary = true,
            "--backpropagate" => backpropagate = true,
            "--backpropagate-limit" => {
                i += 1;
                backpropagate_lim = Some(raw.get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| { eprintln!("--backpropagate-limit needs an integer (plies)"); process::exit(1); }));
            }
            "-l" | "--limit"  => {
                i += 1;
                limit = Some(raw.get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| { eprintln!("--limit needs an integer"); process::exit(1); }));
            }
            "-s" | "--skip"   => {
                i += 1;
                skip = raw.get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| { eprintln!("--skip needs an integer"); process::exit(1); });
            }
            "-o" | "--output" => {
                i += 1;
                output = Some(PathBuf::from(raw.get(i).unwrap_or_else(|| {
                    eprintln!("--output needs a filename"); process::exit(1);
                })));
            }
            "--syzygy-path" => {
                i += 1;
                syzygy_path = Some(PathBuf::from(raw.get(i).unwrap_or_else(|| {
                    eprintln!("--syzygy-path needs a directory"); process::exit(1);
                })));
            }
            "-d" | "--input-dir" => {
                i += 1;
                input_dir = Some(PathBuf::from(raw.get(i).unwrap_or_else(|| {
                    eprintln!("--input-dir needs a directory"); process::exit(1);
                })));
            }
            other if other.starts_with('-') => {
                eprintln!("Unknown option: {other}");
                usage(prog);
            }
            path => files.push(PathBuf::from(path)),
        }
        i += 1;
    }

    // Expand --input-dir: collect all .gz files, sorted for determinism.
    if let Some(ref dir) = input_dir {
        match std::fs::read_dir(dir) {
            Ok(entries) => {
                let mut dir_files: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        p.extension()
                            .and_then(|ext| ext.to_str())
                            .map(|ext| ext.eq_ignore_ascii_case("gz"))
                            .unwrap_or(false)
                    })
                    .collect();
                dir_files.sort();
                if dir_files.is_empty() {
                    eprintln!("Warning: no .gz files found in {}", dir.display());
                } else {
                    eprintln!("📂 Found {} .gz files in {}", dir_files.len(), dir.display());
                }
                files.extend(dir_files);
            }
            Err(e) => {
                eprintln!("❌ Cannot read --input-dir {}: {e}", dir.display());
                process::exit(1);
            }
        }
    }

    if files.is_empty() {
        eprintln!("Error: no input files specified. Use -d <dir> or pass files directly.");
        usage(prog);
    }

    let backprop_mode = resolve_backprop_mode(backpropagate, backpropagate_lim);

    if backprop_mode.is_enabled() && syzygy_path.is_none() {
        eprintln!("Warning: --backpropagate/--backpropagate-limit has no effect without --syzygy-path.");
    }

    Args { files, verbosity, limit, skip, summary, output, syzygy_path, backprop_mode, _input_dir: input_dir }
}

/// Resolve `--backpropagate` and `--backpropagate-limit` into a single
/// `BackpropMode`, per this precedence:
///   - limit given (regardless of `--backpropagate`) → `Limited(n)`.
///     `--backpropagate-limit` alone is sufficient to enable backprop;
///     you don't need `--backpropagate` too. If both are given, the
///     limit wins — it's strictly more specific than "unlimited".
///   - `--backpropagate` alone (no limit)            → `Unlimited`.
///   - neither flag                                  → `Off`.
fn resolve_backprop_mode(backpropagate: bool, limit: Option<u16>) -> BackpropMode {
    match limit {
        Some(n) => BackpropMode::Limited(n),
        None if backpropagate => BackpropMode::Unlimited,
        None => BackpropMode::Off,
    }
}

// ── Stats ─────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Stats {
    total_records:  usize,
    skipped_frc:    usize,
    total_visits:   u64,
    sum_root_q:     f64,
    sum_root_d:     f64,
    sum_plies_left: f64,
    sum_policy_kld: f64,
    adjudicated:    usize,
    proven_best:    usize,
    version_counts: HashMap<u32, usize>,
    format_counts:  HashMap<u32, usize>,
}

impl Stats {
    fn update(&mut self, rec: &V6Record) {
        self.total_records  += 1;
        self.total_visits   += rec.visits as u64;
        self.sum_root_q     += rec.root_q as f64;
        self.sum_root_d     += rec.root_d as f64;
        self.sum_plies_left += rec.plies_left as f64;
        self.sum_policy_kld += rec.policy_kld as f64;
        if rec.game_adjudicated() { self.adjudicated += 1; }
        if rec.best_q_is_proven() { self.proven_best += 1; }
        *self.version_counts.entry(rec.version).or_insert(0) += 1;
        *self.format_counts.entry(rec.input_format).or_insert(0) += 1;
    }

    fn print(&self) {
        let n = self.total_records as f64;
        let sep = "═".repeat(60);
        println!("\n╔{sep}");
        println!("║ Summary");
        println!("╠{sep}");
        println!("║ Total records     : {}", self.total_records);
        if self.skipped_frc > 0 {
            println!("║ Skipped (FRC/var) : {}", self.skipped_frc);
        }
        println!("║ Total visits      : {}", self.total_visits);
        if self.total_records > 0 {
            println!("║ Avg visits/record : {:.1}", self.total_visits as f64 / n);
            println!("║ Avg root Q        : {:+.4}", self.sum_root_q / n);
            println!("║ Avg root D        : {:.4}", self.sum_root_d / n);
            println!("║ Avg plies left    : {:.1}", self.sum_plies_left / n);
            println!("║ Avg policy KLD    : {:.6}", self.sum_policy_kld / n);
            println!("║ Adjudicated       : {} ({:.1}%)", self.adjudicated,
                     self.adjudicated as f64 / n * 100.0);
            println!("║ Proven best move  : {} ({:.1}%)", self.proven_best,
                     self.proven_best as f64 / n * 100.0);
        }
        println!("║ Versions seen     : {:?}", self.version_counts);
        println!("║ Input formats     : {:?}", self.format_counts);
        println!("╚{sep}");
    }
}

// ── Reader ────────────────────────────────────────────────────────────────────

fn open_reader(path: &PathBuf) -> io::Result<Box<dyn Read>> {
    let file = File::open(path)?;
    let is_gz = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("gz"))
        .unwrap_or(false);
    if is_gz {
        Ok(Box::new(BufReader::new(GzDecoder::new(file))))
    } else {
        Ok(Box::new(BufReader::new(file)))
    }
}

// ── Per-file processing ───────────────────────────────────────────────────────

fn process_file(
    path:          &PathBuf,
    args:          &Args,
    global_index:  &mut usize,
    shown:         &mut usize,
    stats:         &mut Stats,
    writer:        &mut Option<BinpackWriter>,
    prober:        &SyzygyProber,
    prog:          &mut Option<Progress>,
) -> io::Result<()> {
    let mut reader           = open_reader(path)?;
    let mut buf              = vec![0u8; RECORD_SIZE];
    let mut file_skipped_err = 0usize;
    let mut ply: u16         = 0;
    // Track whether we have already found a TB hit in this game so we can
    // skip further probes (which cost disk I/O) for the rest of the file.
    let mut tb_hit_found     = false;

    let filename = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?");

    if let Some(p) = prog.as_mut() {
        p.set_file(filename);
    } else {
        eprintln!("📂 {filename}");
    }

    loop {
        if let Some(lim) = args.limit {
            if *shown >= lim { break; }
        }

        let mut total_read = 0;
        loop {
            match reader.read(&mut buf[total_read..])? {
                0 => break,
                n => total_read += n,
            }
            if total_read == RECORD_SIZE { break; }
        }
        if total_read == 0 { break; }
        if total_read < RECORD_SIZE {
            eprintln!("\n  ⚠️  Trailing {total_read} bytes (expected {RECORD_SIZE}), skipping.");
            break;
        }

        let rec = match V6Record::from_bytes(&buf) {
            Some(r) => r,
            None    => {
                eprintln!("\n  ⚠️  Could not parse record #{}", *global_index);
                *global_index += 1;
                ply = ply.saturating_add(1);
                continue;
            }
        };

        // Always skip non-classical (FRC/variant) records.
        if rec.input_format != INPUT_CLASSICAL {
            stats.skipped_frc += 1;
            *global_index += 1;
            ply = ply.saturating_add(1);
            continue;
        }

        let rec_idx = *global_index;
        *global_index += 1;
        let rec_ply = ply;
        ply = ply.saturating_add(1);

        if rec_idx < args.skip { continue; }

        if rec.version != 6 {
            eprintln!("\n  ⚠️  Record #{rec_idx} has version {} (expected 6)", rec.version);
        }

        if args.summary { stats.update(&rec); }

        if let Some(v) = args.verbosity {
            print_record(rec_idx, filename, &rec, v);
        }

        if let Some(w) = writer.as_mut() {
            match w.buffer_record(&rec, rec_ply, prober, tb_hit_found) {
                Ok((_, new_hit)) => {
                    // Stop probing for the rest of this game once we have a hit.
                    if new_hit {
                        tb_hit_found = true;
                    }
                }
                Err(e) => {
                    if args.verbosity.is_some() {
                        eprintln!("  ⚠️  Binpack error at #{rec_idx}: {e}");
                    }
                    file_skipped_err += 1;
                }
            }
        }

        *shown += 1;

        if let Some(p) = prog.as_mut() {
            if *shown % PROGRESS_INTERVAL == 0 {
                let (written, skipped_err) = writer.as_ref()
                    .map(|w| (w.written(), w.skipped()))
                    .unwrap_or((0, 0));
                p.update(*shown, written, stats.skipped_frc, skipped_err + file_skipped_err);
            }
        }
    }

    // End of game (file) — flush the game buffer with TB propagation.
    if let Some(w) = writer.as_mut() {
        if let Err(e) = w.flush_game(args.backprop_mode) {
            eprintln!("  ⚠️  Error flushing game for {filename}: {e}");
        }
    }

    // Final progress update for this file.
    if let Some(p) = prog.as_mut() {
        let (written, skipped_err) = writer.as_ref()
            .map(|w| (w.written(), w.skipped()))
            .unwrap_or((0, 0));
        p.update(*shown, written, stats.skipped_frc, skipped_err + file_skipped_err);
    }

    Ok(())
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() {
    const _: () = assert!(RECORD_SIZE == 8356,
        "RECORD_SIZE must be 8356 — update offsets to match V6TrainingData");

    let args             = parse_args();
    let mut global_index = 0usize;
    let mut shown        = 0usize;
    let mut stats        = Stats::default();

    // ── Syzygy prober ─────────────────────────────────────────────────────────
    let prober = if let Some(ref sz_path) = args.syzygy_path {
        match SyzygyProber::load(sz_path) {
            Ok(p) => {
                eprintln!("♟  Syzygy: tables loaded from {}", sz_path.display());
                let inv: TableInventory = p.inventory();
                if inv.file_count > 0 {
                    eprintln!(
                        "♟  Syzygy: found {} table files (up to {}-men)",
                        inv.file_count, inv.max_pieces
                    );
                } else {
                    eprintln!("♟  Syzygy: warning — 0 table files found under the given path(s)");
                }
                match args.backprop_mode {
                    BackpropMode::Off => {
                        eprintln!("♟  Syzygy: forward propagation only (use --backpropagate or --backpropagate-limit N to also patch earlier plies)");
                    }
                    BackpropMode::Unlimited => {
                        eprintln!("♟  Syzygy: backward propagation enabled (unlimited, back to move 1)");
                    }
                    BackpropMode::Limited(n) => {
                        eprintln!("♟  Syzygy: backward propagation enabled (limited to {n} plies before each hit)");
                    }
                }
                p
            }
            Err(e) => {
                eprintln!("❌ {e}");
                process::exit(1);
            }
        }
    } else {
        SyzygyProber::disabled()
    };

    // ── Binpack writer ────────────────────────────────────────────────────────
    let mut writer: Option<BinpackWriter> = if let Some(ref out_path) = args.output {
        match BinpackWriter::create(out_path) {
            Ok(w)  => {
                eprintln!("💾 Output: {}", out_path.display());
                Some(w)
            }
            Err(e) => { eprintln!("❌ Failed to create output file: {e}"); process::exit(1); }
        }
    } else {
        None
    };

    let mut prog: Option<Progress> = if args.verbosity.is_none() {
        Some(Progress::new())
    } else {
        None
    };

    for path in &args.files {
        if let Err(e) = process_file(
            path, &args, &mut global_index, &mut shown, &mut stats,
            &mut writer, &prober, &mut prog,
        ) {
            eprintln!("❌ Error reading {}: {e}", path.display());
        }
    }

    if let Some(w) = writer.as_mut() {
        w.flush();
        let tb_hits             = w.tb_hits();
        let positions_corrected = w.positions_corrected();
        let wdl_orig             = w.wdl_original;
        let wdl_corr             = w.wdl_corrected;

        if let Some(p) = prog.as_ref() {
            p.finish(w.written(), stats.skipped_frc, w.skipped());
        } else {
            eprintln!("💾 Binpack: {} written, {} skipped", w.written(), w.skipped());
        }

        eprintln!(
            "📊 Original  WDL  — wins: {:>8}  draws: {:>8}  losses: {:>8}",
            wdl_orig.wins, wdl_orig.draws, wdl_orig.losses
        );

        if prober.is_loaded() {
            eprintln!("♟  Syzygy: {tb_hits} games had a TB hit");
            eprintln!("♟  Syzygy: {positions_corrected} positions corrected by propagation");
            eprintln!(
                "📊 Corrected WDL  — wins: {:>8}  draws: {:>8}  losses: {:>8}",
                wdl_corr.wins, wdl_corr.draws, wdl_corr.losses
            );
        }
    } else if let Some(p) = prog.as_ref() {
        p.finish(0, stats.skipped_frc, 0);
    }

    if args.summary { stats.print(); }
}