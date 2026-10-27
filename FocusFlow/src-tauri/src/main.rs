// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = focusflow_lib::dispatch_capture_guard(std::env::args_os().skip(1)) {
        std::process::exit(code);
    }
    focusflow_lib::run()
}
