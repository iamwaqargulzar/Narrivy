fn main() {
    // The frontend bundle is embedded at compile time. Rebuild whenever it
    // changes, so `cargo run` after `npm run build` never serves a stale UI.
    println!("cargo:rerun-if-changed=../dist");
    tauri_build::build()
}
