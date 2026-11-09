pub use focusflow_windows_audio_worker::{format, Result};
pub mod signal;
pub mod analysis;
pub mod collect;

pub fn selected_index(text: &str, count: usize) -> Result<usize> {
    let n: usize = text.trim().parse().map_err(|_| "Enter one listed device number, or close to cancel")?;
    if n == 0 || n > count { return Err("Choose one of the listed device numbers".into()); }
    Ok(n - 1)
}

pub fn write_reference(path: &std::path::Path) -> Result<()> {
    let format = format::Format { rate: 48000, channels: 2, bits: 16, valid_bits: 16, align: 4, encoding: format::Encoding::Pcm };
    let file = std::fs::OpenOptions::new().create_new(true).write(true).open(path).map_err(|e|e.to_string())?;
    let mut wav = hound::WavWriter::new(file, hound::WavSpec { sample_rate:48000,channels:2,bits_per_sample:16,sample_format:hound::SampleFormat::Int }).map_err(|e|e.to_string())?;
    let mut signal = signal::Signal::new(format, 1)?;
    while signal.remaining() > 0 {
        for sample in signal.block(signal::BLOCK_FRAMES)?.chunks_exact(2) {
            wav.write_sample(i16::from_le_bytes([sample[0],sample[1]])).map_err(|e|e.to_string())?;
        }
    }
    wav.finalize().map_err(|e|e.to_string())
}
