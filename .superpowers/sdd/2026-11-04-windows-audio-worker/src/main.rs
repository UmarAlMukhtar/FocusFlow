fn main() {
    std::process::exit(focusflow_windows_audio_worker::worker::dispatch(std::env::args_os()).unwrap_or(64));
}
