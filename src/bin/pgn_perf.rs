use std::env;
use std::fs;
use std::io::Cursor;
use std::time::Instant;

use chess_library_rs::pgn::{StreamParser, StreamParserError, Visitor};

struct CountVisitor {
    games: usize,
    headers: usize,
    moves: usize,
}

impl CountVisitor {
    fn new() -> Self {
        CountVisitor {
            games: 0,
            headers: 0,
            moves: 0,
        }
    }
}

impl Visitor for CountVisitor {
    fn start_pgn(&mut self) {
        self.games += 1;
    }

    fn header(&mut self, _key: &str, _value: &str) {
        self.headers += 1;
    }

    fn start_moves(&mut self) {}

    fn move_token(&mut self, mv: &str, _comment: &str) {
        self.moves += 1;

        debug_assert!(
            is_pgn_move_format(mv) || mv.trim().is_empty(),
            "Move '{}' does not match expected PGN format",
            mv
        );
    }

    fn end_pgn(&mut self) {}
}

fn is_pgn_move_format(mv: &str) -> bool {
    let mv = mv.trim();

    // remove annotations like !, ?, !!, ?!, etc
    let mv = mv.trim_end_matches(|c| c == '!' || c == '?');

    if mv.is_empty() {
        return false;
    }

    // Castling
    if matches!(mv, "O-O" | "O-O+" | "O-O#" | "O-O-O" | "O-O-O+" | "O-O-O#") {
        return true;
    }

    if matches!(mv, "0-0" | "0-0+" | "0-0#" | "0-0-0" | "0-0-0+" | "0-0-0#") {
        return true;
    }

    let bytes = mv.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    let is_file = |b: u8| (b'a'..=b'h').contains(&b);
    let is_rank = |b: u8| (b'1'..=b'8').contains(&b);
    let is_piece = |b: u8| matches!(b as char, 'K' | 'Q' | 'R' | 'B' | 'N');

    // Piece move: Nf3, Rae1, R1e2, Nxd5
    if i < len && is_piece(bytes[i]) {
        i += 1;

        // Optional disambiguation
        if i < len && is_file(bytes[i]) {
            // Full square disambiguation: e.g. Qa8b7 → consume file+rank
            if i + 2 < len && is_rank(bytes[i + 1]) && is_file(bytes[i + 2]) {
                i += 2;
            } else if i + 1 >= len || !is_rank(bytes[i + 1]) {
                // File-only disambiguation (next is not a rank)
                i += 1;
            } else if i + 2 < len && bytes[i + 2] == b'x' {
                // e.g. Nfxe5
                i += 1;
            }
            // else: file is part of target square, don't consume
        }
        if i < len && is_rank(bytes[i]) {
            // same idea for rank disambiguation
            if i + 1 >= len || !is_file(bytes[i + 1]) {
                i += 1;
            } else if i + 2 < len && is_rank(bytes[i + 2]) {
                // handles R1e2
                i += 1;
            }
        }

        // Optional capture
        if i < len && bytes[i] == b'x' {
            i += 1;
        }

        // Target square
        if i + 1 >= len || !is_file(bytes[i]) || !is_rank(bytes[i + 1]) {
            return false;
        }
        i += 2;
    } else {
        // Pawn move: e4, exd5, e8=Q
        if i >= len || !is_file(bytes[i]) {
            return false;
        }

        if i + 1 < len && is_rank(bytes[i + 1]) {
            // simple pawn move: e4
            i += 2;
        } else if i + 2 < len && bytes[i + 1] == b'x' && is_file(bytes[i + 2]) {
            // pawn capture: exd5
            i += 3;
            if i >= len || !is_rank(bytes[i]) {
                return false;
            }
            i += 1;
        } else {
            return false;
        }
    }

    // Optional promotion: =Q, =R, =B, =N
    if i < len && bytes[i] == b'=' {
        i += 1;
        if i >= len || !matches!(bytes[i] as char, 'Q' | 'R' | 'B' | 'N') {
            return false;
        }
        i += 1;
    }

    // Optional check/checkmate
    if i < len && matches!(bytes[i] as char, '+' | '#') {
        i += 1;
    }

    i == len
}

fn usage(prog: &str) {
    eprintln!("Usage: {} <pgn-file> [iterations]", prog);
    eprintln!("Example: cargo run --release --bin pgn_perf -- large.pgn 10");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage(&args[0]);
        std::process::exit(1);
    }

    let path = &args[1];
    let iters: usize = if args.len() >= 3 {
        args[2].parse().unwrap_or(5)
    } else {
        5
    };

    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to read {}: {}", path, e);
            std::process::exit(2);
        }
    };

    let size_bytes = data.len();
    println!(
        "PGN performance test\n  file: {}\n  size: {} bytes\n  iterations: {}",
        path, size_bytes, iters
    );

    let mut total_secs = 0f64;
    let mut last_games = 0usize;
    let mut last_moves = 0usize;

    for i in 0..iters {
        let mut cursor = Cursor::new(&data);
        let mut vis = CountVisitor::new();
        let t0 = Instant::now();
        let res: Result<(), StreamParserError> =
            StreamParser::new(&mut cursor).read_games(&mut vis);
        let dur = t0.elapsed();
        if let Err(e) = res {
            eprintln!("Parser returned error on iteration {}: {}", i + 1, e);
            std::process::exit(3);
        }
        let secs = dur.as_secs_f64();
        total_secs += secs;
        last_games = vis.games;
        last_moves = vis.moves;
        println!(
            "iter {:>2}: {:.6} s — games: {:>3}, moves: {:>6}",
            i + 1,
            secs,
            vis.games,
            vis.moves
        );
    }

    let avg = total_secs / (iters as f64);
    let mb = (size_bytes as f64) / (1024.0 * 1024.0);
    let mbps = mb / avg;

    println!("\nResult (avg over {} runs):", iters);
    println!("  avg time: {:.6} s", avg);
    println!("  throughput: {:.2} MB/s", mbps);
    println!("  last run games: {}, moves: {}", last_games, last_moves);
}
