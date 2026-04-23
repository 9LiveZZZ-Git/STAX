//! Minimal REPL. M0: arithmetic and stack ops only.
//! Run with: `cargo run --bin stax-repl`
//!
//! `quit` to exit. `.s` to show the stack.

use std::io::{self, BufRead, Write};

use stax_eval::Interp;
use stax_parser::parse;

fn main() -> anyhow::Result<()> {
    let mut interp = Interp::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    println!("stax M0 REPL — try: 2 3 + .s");
    loop {
        print!("> ");
        stdout.flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() { continue; }
        if line == "quit" || line == ":q" { break; }
        if line == ".s" {
            for (i, v) in interp.stack.iter().enumerate() {
                println!("  [{i}] {v:?}");
            }
            continue;
        }

        match parse(line) {
            Err(e) => println!("parse error: {e}"),
            Ok(ops) => match interp.exec(&ops) {
                Err(e) => println!("eval error: {e}"),
                Ok(()) => {
                    if let Some(v) = interp.stack.last() {
                        println!("{v:?}");
                    }
                }
            },
        }
    }
    Ok(())
}
