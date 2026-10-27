#[cfg(windows)]
mod native;
fn main() {
    #[cfg(windows)]
    if let Err(e) = native::run() { eprintln!("probe unavailable or failed: {e}"); std::process::exit(2); }
    #[cfg(not(windows))]
    { eprintln!("Windows native probe unavailable on this OS"); std::process::exit(2); }
}
