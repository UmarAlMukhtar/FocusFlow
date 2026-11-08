fn main() {
    // Ordinary capture retains its real Job/current-Init gate, independently of GitHub.
    if let Some(code)=focusflow_windows_audio_worker::worker::dispatch(std::env::args_os()) {std::process::exit(code);}
    std::process::exit(focusflow_windows_audio_worker::proof::dispatch(std::env::args_os()));
}
