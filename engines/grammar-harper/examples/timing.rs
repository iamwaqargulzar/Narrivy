//! How long one check takes: `cargo run -p narrivy-grammar-harper --example timing`.
use std::time::Instant;

fn main() {
    let mut checker = narrivy_grammar_harper::Checker::new();
    let text = "Their going to the the store tomorow, and she dont know were it is. ".repeat(20);
    let started = Instant::now();
    let _ = checker.check(&text, &Default::default());
    println!("first check (builds rules): {:?}", started.elapsed());
    let started = Instant::now();
    let found = checker
        .check(&text, &Default::default())
        .unwrap_or_default();
    println!(
        "next check: {:?}, {} findings, {} bytes",
        started.elapsed(),
        found.len(),
        text.len()
    );
}
