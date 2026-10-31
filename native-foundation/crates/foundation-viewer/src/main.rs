#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod desktop;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    desktop::run()
}
