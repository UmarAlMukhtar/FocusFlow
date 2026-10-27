use serde::{Deserialize, Serialize};
use crate::{CoreResult, render::RenderPlan};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum PreviewRequest { Video { start_us: u64, end_us: u64 }, Still { at_us: u64 } }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreviewSelection {
    pub first_frame: u64, pub frame_count: u64, pub fps: u32,
    pub start_us: u64, pub duration_us: u64,
}
pub const FPS: u32 = 30;
pub const MAX_INPUTS: usize = 32;

pub fn select(plan: &RenderPlan, request: &PreviewRequest) -> CoreResult<PreviewSelection> {
    use crate::project::require;
    require(plan.preset.fps() == FPS, "preview", "Preview requires a 30fps render plan")?;
    let total = crate::render::grid_boundary(plan.duration_us, FPS)?;
    let (first, end) = match *request {
        PreviewRequest::Video { start_us, end_us } => {
            require(start_us < end_us && end_us <= plan.duration_us, "preview", "Preview range must be nonempty and inside the edited output")?;
            require(end_us - start_us <= 30_000_000, "preview", "Video previews are limited to 30 seconds")?;
            (crate::render::grid_boundary(start_us, FPS)?.min(total), crate::render::grid_boundary(end_us, FPS)?.min(total))
        }
        PreviewRequest::Still { at_us } => {
            require(at_us < plan.duration_us, "preview", "Still time must be inside the edited output")?;
            let first = at_us * FPS as u64 / 1_000_000;
            (first, (first + 1).min(total))
        }
    };
    require(end > first, "preview", "Preview range contains no 30fps sample; request a still or a longer range")?;
    Ok(PreviewSelection { first_frame: first, frame_count: end - first, fps: FPS,
        start_us: first * 1_000_000 / FPS as u64, duration_us: (end - first) * 1_000_000 / FPS as u64 })
}

pub(crate) fn validate(plan: &RenderPlan, selection: &PreviewSelection) -> CoreResult<()> {
    crate::project::require(plan.preset.fps() == FPS && selection.fps == FPS
        && selection.frame_count > 0 && selection.frame_count <= 900
        && selection.first_frame.checked_add(selection.frame_count).is_some_and(|end| end <= (plan.duration_us * FPS as u64).div_ceil(1_000_000))
        && selection.start_us == selection.first_frame * 1_000_000 / FPS as u64
        && selection.duration_us == selection.frame_count * 1_000_000 / FPS as u64,
        "preview", "Invalid preview sampling metadata")
}

pub fn dimensions(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= 960 { return (width, height); }
    ((width as u64 * 960 / longest as u64 / 2 * 2).max(2) as u32,
     (height as u64 * 960 / longest as u64 / 2 * 2).max(2) as u32)
}

pub(crate) struct VideoSpan { pub clip: usize, pub first: u64, pub count: u64, pub source_first: u64 }
pub(crate) struct AudioSpan { pub count: u64, pub low: u64, pub high: u64, pub delay: u64, pub input: Option<usize> }
pub(crate) struct Inputs { pub video: Vec<VideoSpan>, pub audio: Option<crate::audio::RenderAudio>, pub audio_spans: Vec<AudioSpan> }

pub(crate) fn inputs(snapshot: &crate::store::RenderSnapshot, plan: &RenderPlan, selection: &PreviewSelection, use_audio: bool) -> CoreResult<Inputs> {
    validate(plan, selection)?;
    let p = selection.first_frame; let q = p + selection.frame_count;
    let mut video = Vec::new();
    for (index, clip) in plan.clips.iter().enumerate() {
        let (first, count, source_first) = crate::render::clip_grid(clip, FPS)?;
        let a = first.max(p); let b = (first + count).min(q);
        if a < b { video.push(VideoSpan { clip: index, first: a, count: b - a, source_first: source_first + a - first }); }
    }
    let mut input_count = video.len();
    let mut audio = crate::audio::render_audio(snapshot, use_audio)?;
    let mut audio_spans = Vec::new();
    if let Some(wav) = &audio {
        let rate = wav.sample_rate;
        let p = (p * rate as u64).div_ceil(FPS as u64);
        let q = (q * rate as u64).div_ceil(FPS as u64).min(crate::render::grid_boundary(plan.duration_us, rate)?);
        let offset = snapshot.document.timing.audio_offset_us.unwrap_or(0);
        let shift = crate::render::grid_boundary(offset.unsigned_abs(), rate)? as i128 * if offset < 0 { -1 } else { 1 };
        for clip in &plan.clips {
            let (first, count, source_first) = crate::render::clip_grid(clip, rate)?;
            let a = first.max(p); let b = (first + count).min(q);
            if a >= b { continue; }
            let count = b - a;
            let raw = (source_first + a - first) as i128 - shift;
            let low = raw.max(0).min(wav.sample_count as i128) as u64;
            let high = (raw + count as i128).max(0).min(wav.sample_count as i128) as u64;
            let input = if low < high { let i = input_count; input_count += 1; Some(i) } else { None };
            audio_spans.push(AudioSpan { count, low, high, delay: (-raw).max(0).min(count as i128) as u64, input });
        }
        if audio_spans.is_empty() { audio = None; }
    }
    crate::project::require(input_count <= MAX_INPUTS, "preview.inputs", "Preview exceeds 32 seeked file inputs; request a shorter or simpler range")?;
    Ok(Inputs { video, audio, audio_spans })
}
