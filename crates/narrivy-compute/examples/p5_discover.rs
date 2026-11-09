//! Native discovery probe CLI. Prints a DiscoveryReport as JSON.
//!
//! Usage: p5_discover [path-to-transcribe-cli]
//!
//! The crate itself owns no host access, so this example injects the demo
//! probe from `examples/support/host_probe.rs`. The product composition
//! injects the reviewed Linux probe instead.
mod support;

use narrivy_compute::discovery::discover;
use std::path::PathBuf;
use support::host_probe::SystemHostProbe;

fn main() {
    let cli = std::env::args().nth(1).map(PathBuf::from);
    let report = discover(&SystemHostProbe, cli.as_deref());
    match serde_json::to_string_pretty(&report) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("serialize failed: {e}");
            std::process::exit(1);
        }
    }
}
