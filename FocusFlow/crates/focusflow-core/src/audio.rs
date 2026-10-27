use std::path::Path;

pub fn is_usable_wav(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else { return false; };
    if !metadata.is_file() || metadata.len() <= 44 { return false; }
    let Ok(mut reader) = hound::WavReader::open(path) else { return false; };
    if reader.duration() == 0 || reader.len() == 0 { return false; }
    // Read all declared samples so a truncated data chunk is not treated as usable audio.
    match reader.spec().sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().all(|sample| sample.is_ok_and(f32::is_finite)),
        hound::SampleFormat::Int => reader.samples::<i32>().all(|sample| sample.is_ok()),
    }
}

pub(crate) struct RenderAudio {
    pub path: std::path::PathBuf,
    pub sample_rate: u32,
    pub sample_count: u64,
    pub channels: u16,
}

/// Missing/unusable optional audio falls back to video only; invalid references still fail.
pub(crate) fn render_audio(snapshot: &crate::store::RenderSnapshot, enabled: bool) -> crate::CoreResult<Option<RenderAudio>> {
    let Some(asset) = &snapshot.document.microphone else { return Ok(None); };
    let path = match crate::paths::asset_path(&snapshot.project_dir, &asset.path) {
        Ok(path) => path,
        Err(error) if error.code == crate::ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !enabled || !is_usable_wav(&path) { return Ok(None); }
    let Ok(reader) = hound::WavReader::open(&path) else { return Ok(None); };
    let sample_rate = reader.spec().sample_rate;
    Ok((sample_rate > 0).then_some(RenderAudio { path, sample_rate, sample_count: reader.duration() as u64, channels: reader.spec().channels }))
}
