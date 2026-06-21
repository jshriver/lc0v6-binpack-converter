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

use record::{V6Record, RECORD_SIZE};
use printer::{Verbosity, print_record};
use binpack::BinpackWriter;
use progress::Progress;
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
    files:     Vec<PathBuf>,
    verbosity: Option<Verbosity>,
    limit:     Option<usize>,
    skip:      usize,
    summary:   bool,
    output:    Option<PathBuf>,
}

fn usage(prog: &str) -> ! {
    eprintln!(
        "Usage: {prog} [OPTIONS] <file> [<file> ...]\n\
         \n\
         OPTIONS:\n\
           -b, --brief            One-line summary per record\n\
           -n, --normal           Key fields per record\n\
           -f, --full             All fields including policy and planes\n\
           -l, --limit N          Only process the first N records\n\
           -s, --skip N           Skip the first N records\n\
           --summary              Print aggregate statistics at the end\n\
           -o, --output <file>    Export to .binpack file\n\
           -h, --help             Show this help\n\
         \n\
         Without -b/-n/-f, shows a live progress bar instead of per-record output.\n\
         Both raw binary and gzip-compressed (.gz) files are supported.\n\
         Non-classical (FRC/variant) records are always skipped.\n\
         \n\
         EXAMPLES:\n\
           # Silent export with progress bar\n\
           {prog} -o out.binpack data/*.gz\n\
           # Inspect first 10 records\n\
           {prog} --normal --limit 10 game.gz\n\
           # Inspect while exporting\n\
           {prog} --brief -o out.binpack game.gz"
    );
    process::exit(1);
}

fn parse_args() -> Args {
    let raw: Vec<String> = env::args().collect();
    let prog = raw.first().map(String::as_str).unwrap_or("lc0_parser");
    let mut files     = Vec::new();
    let mut verbosity = None;
    let mut limit     = None;
    let mut skip      = 0usize;
    let mut summary   = false;
    let mut output    = None;
    let mut i         = 1;

    while i < raw.len() {
        match raw[i].as_str() {
            "-h" | "--help"   => usage(prog),
            "-b" | "--brief"  => verbosity = Some(Verbosity::Brief),
            "-n" | "--normal" => verbosity = Some(Verbosity::Normal),
            "-f" | "--full"   => verbosity = Some(Verbosity::Full),
            "--summary"       => summary = true,
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
            other if other.starts_with('-') => {
                eprintln!("Unknown option: {other}");
                usage(prog);
            }
            path => files.push(PathBuf::from(path)),
        }
        i += 1;
    }

    if files.is_empty() {
        eprintln!("Error: no input files specified.");
        usage(prog);
    }

    Args { files, verbosity, limit, skip, summary, output }
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
    path:         &PathBuf,
    args:         &Args,
    global_index: &mut usize,
    shown:        &mut usize,
    stats:        &mut Stats,
    writer:       &mut Option<BinpackWriter>,
    prog:         &mut Option<Progress>,
) -> io::Result<()> {
    let mut reader           = open_reader(path)?;
    let mut buf              = vec![0u8; RECORD_SIZE];
    let mut file_skipped_err = 0usize;
    let mut ply: u16         = 0;

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
            match w.write_record(&rec, rec_ply) {
                Ok(_)  => {}
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
            &mut writer, &mut prog,
        ) {
            eprintln!("❌ Error reading {}: {e}", path.display());
        }
    }

    if let Some(w) = writer.as_mut() {
        w.flush();
        if let Some(p) = prog.as_ref() {
            p.finish(w.written(), stats.skipped_frc, w.skipped());
        } else {
            eprintln!("💾 Binpack: {} written, {} skipped", w.written(), w.skipped());
        }
    } else if let Some(p) = prog.as_ref() {
        p.finish(0, stats.skipped_frc, 0);
    }

    if args.summary { stats.print(); }
}