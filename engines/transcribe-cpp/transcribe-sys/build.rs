//! Placeholder build script.
//!
//! Does nothing unless `TRANSCRIBE_CPP_ROOT` is set. No upstream code is ever
//! compiled, copied, or downloaded by this script; it only emits link flags
//! against an ALREADY-BUILT transcribe.cpp installation that the operator
//! provides. License review must pass first — see ../BUILD-NOTES.md.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=TRANSCRIBE_CPP_ROOT");
    println!("cargo:rerun-if-env-changed=TRANSCRIBE_CPP_LINK_SHARED");

    let Some(root) = std::env::var_os("TRANSCRIBE_CPP_ROOT") else {
        // Placeholder mode: default workspace builds stay toolchain-free.
        // Enabling the `runtime` feature without TRANSCRIBE_CPP_ROOT will
        // fail at final link with undefined `tcpp_*` symbols — intentional,
        // documented in ../BUILD-NOTES.md ("verification without the real
        // library" uses a locally authored ABI shim, never upstream code).
        return;
    };
    let root = PathBuf::from(root);
    let include_dir = root.join("include");
    if !include_dir.is_dir() {
        println!(
            "cargo:warning=transcribe-sys: TRANSCRIBE_CPP_ROOT has no include/ directory at {}",
            include_dir.display()
        );
    }
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("lib").display()
    );
    if std::env::var_os("TRANSCRIBE_CPP_LINK_SHARED").is_some() {
        println!("cargo:rustc-link-lib=dylib=transcribecpp");
    } else {
        println!("cargo:rustc-link-lib=static=transcribecpp");
    }
}
