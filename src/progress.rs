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

//! Terminal progress display.

use std::io::{self, Write};
use std::time::Instant;

pub struct Progress {
    start:        Instant,
    processed:    usize,
    written:      usize,
    skipped:      usize,
    current_file: String,
}

impl Progress {
    pub fn new() -> Self {
        Self {
            start:        Instant::now(),
            processed:    0,
            written:      0,
            skipped:      0,
            current_file: String::new(),
        }
    }

    pub fn set_file(&mut self, name: &str) {
        self.current_file = name.to_string();
    }

    pub fn update(&mut self, processed: usize, written: usize, skipped_frc: usize, skipped_err: usize) {
        self.processed = processed;
        self.written   = written;
        self.skipped   = skipped_frc + skipped_err;
        self.render();
    }

    fn render(&self) {
        let elapsed = self.start.elapsed().as_secs_f64();
        let rate = if elapsed > 0.0 { self.processed as f64 / elapsed } else { 0.0 };

        let spinner = ["⠋","⠙","⠹","⠸","⠼","⠴","⠦","⠧","⠇","⠏"];
        let frame = spinner[(elapsed * 10.0) as usize % spinner.len()];

        let file = if self.current_file.len() > 32 {
            &self.current_file[self.current_file.len() - 32..]
        } else {
            &self.current_file
        };

        eprint!(
            "\r{frame} {file:<32}  ✅ {:>8}  🚫 {:>5}  ⚡ {:.0}/s   ",
            self.written, self.skipped, rate,
        );
        let _ = io::stderr().flush();
    }

    pub fn finish(&self, total_written: usize, total_skipped_frc: usize, total_skipped_err: usize) {
        let elapsed = self.start.elapsed().as_secs_f64();
        let rate = if elapsed > 0.0 { self.processed as f64 / elapsed } else { 0.0 };
        eprintln!();
        eprintln!("─────────────────────────────────────────────────────────");
        eprintln!("✨ Done in {:.1}s  ({:.0} records/s)", elapsed, rate);
        eprintln!("   ✅  Written to binpack : {total_written}");
        eprintln!("   🚫  Skipped            : {}", total_skipped_frc + total_skipped_err);
        eprintln!("─────────────────────────────────────────────────────────");
    }
}