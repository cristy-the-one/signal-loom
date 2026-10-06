#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    signal_loom_lib::run();
}
