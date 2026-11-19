//! Narrivy desktop shell binary entrypoint (thin; logic lives in the lib).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    narrivy_desktop_lib::run()
}
