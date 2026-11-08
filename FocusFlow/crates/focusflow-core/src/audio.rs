pub mod normalize;
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

pub use crate::project::AudioTrackKind;
#[derive(Debug, Clone)]
pub struct PreparedAudioTrack {
    pub kind: AudioTrackKind,
    pub path: std::path::PathBuf,
    pub track: crate::project::AudioTrack,
}

pub fn prepare_audio(snapshot: &crate::store::RenderSnapshot, enabled: bool) -> crate::CoreResult<Vec<PreparedAudioTrack>> {
    let Some(tracks)=&snapshot.document.audio_tracks else {return Ok(Vec::new());};
    let mut prepared=Vec::new();
    for (kind,asset) in [(AudioTrackKind::Microphone,&tracks.microphone),(AudioTrackKind::System,&tracks.system)] {
        let Some(asset)=asset else {continue;};
        let path=match crate::paths::asset_path(&snapshot.project_dir,&asset.path) {
            Ok(path)=>path,
            Err(error) if error.code==crate::ErrorCode::NotFound=> {crate::store::validate_missing_asset(&snapshot.project_dir,&asset.path)?;continue;},
            Err(error)=>return Err(error),
        };
        if !enabled || !is_usable_wav(&path) {continue;}
        let Ok(reader)=hound::WavReader::open(&path) else {continue;};let spec=reader.spec();
        let encoding=matches!((spec.sample_format,spec.bits_per_sample),(hound::SampleFormat::Int,16|24|32)|(hound::SampleFormat::Float,32));
        if !encoding || spec.sample_rate!=asset.sample_rate || spec.channels!=asset.channels || reader.len()%u32::from(spec.channels)!=0 || u64::from(reader.duration())!=asset.frame_count {continue;}
        prepared.push(PreparedAudioTrack {kind,path,track:asset.clone()});
    }
    Ok(prepared)
}

pub const OUTPUT_RATE: u32 = 48_000;
pub const MAX_AUDIO_SPANS: usize = 16_384;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSourceSpan {
    pub domain: usize,
    pub domain_first_sample: u64,
    pub domain_end_sample: u64,
    pub source_start_frame: u64,
    pub source_end_frame: u64,
    pub position_numerator: i128,
    pub position_denominator: i128,
    pub step_numerator: i128,
    pub step_denominator: i128,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSpan {
    pub kind: AudioTrackKind,
    pub output_first_sample: u64,
    pub sample_count: u64,
    pub source: Option<AudioSourceSpan>,
    pub input: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct AudioInput {
    pub path: std::path::PathBuf,
    pub kind: AudioTrackKind,
    pub index: usize,
}
#[derive(Debug, Clone, Default)]
pub struct AudioRenderPlan {
    pub tracks: Vec<PreparedAudioTrack>,
    pub inputs: Vec<AudioInput>,
    pub spans: Vec<AudioSpan>,
    pub first_sample: u64,
    pub total_samples: u64,
    pub input_base: usize,
    pub(crate) windows: Vec<normalize::Window>,
    pub(crate) scratch_upper: u64,
}
struct Interval { generation:u32, start_frame:u64, end_frame:u64, start:i128, end:i128, denominator:i128 }
fn intervals(track:&crate::project::AudioTrack)->crate::CoreResult<Vec<Interval>> {
    use crate::project::AudioTrackTiming;
    let values=match &track.timing {
        AudioTrackTiming::Mapped{segments,..}=>Ok(segments.iter().map(|s|Interval{generation:s.generation,start_frame:s.source_start_frame,end_frame:s.source_end_frame,start:i128::from(s.video_start_us),end:i128::from(s.video_end_us),denominator:1}).collect()),
        timing=> {let offset=match timing{AudioTrackTiming::Estimated{offset_us,..}=>*offset_us,_=>0};let denominator=i128::from(track.sample_rate);let start=checked(i128::from(offset).checked_mul(denominator))?;Ok(vec![Interval{generation:0,start_frame:0,end_frame:track.frame_count,start,end:checked(start.checked_add(checked(i128::from(track.frame_count).checked_mul(1_000_000))?))?,denominator}])}
    }?;
    let mut merged:Vec<Interval>=Vec::new();
    for interval in values {
        if let Some(previous)=merged.last_mut() {
            let same_slope=checked((previous.end-previous.start).checked_mul(i128::from(interval.end_frame-interval.start_frame)))?==checked((interval.end-interval.start).checked_mul(i128::from(previous.end_frame-previous.start_frame)))?;
            if previous.generation==interval.generation && previous.denominator==interval.denominator && previous.end_frame==interval.start_frame && previous.end==interval.start && same_slope {
                previous.end_frame=interval.end_frame;previous.end=interval.end;continue;
            }
        }
        merged.push(interval);
    }
    Ok(merged)
}
fn checked(value:Option<i128>)->crate::CoreResult<i128> {value.ok_or_else(||crate::project::invalid("audio.spans","Audio timing arithmetic exceeds the supported range"))}
fn ceil_ratio(n:i128,d:i128)->i128 {n.div_euclid(d)+i128::from(n.rem_euclid(d)!=0)}
/// Compile one absolute 48kHz output grid; preview crops never reset clip or mapping phase.
pub fn compile_audio(snapshot:&crate::store::RenderSnapshot,plan:&crate::render::RenderPlan,selection:Option<&crate::preview::PreviewSelection>,enabled:bool)->crate::CoreResult<AudioRenderPlan> {
    crate::render::validate_plan(snapshot,plan)?;
    if let Some(selection)=selection {crate::preview::validate(plan,selection)?;}
    let tracks=prepare_audio(snapshot,enabled)?;
    let mut clip_domains=vec![(0_usize,0_usize);plan.clips.len()];let mut run_first=0;
    for next in 1..=plan.clips.len() {
        if next==plan.clips.len() || plan.clips[next-1].source_end_us!=plan.clips[next].source_start_us {
            for domain in &mut clip_domains[run_first..next]{*domain=(run_first,next-1);}run_first=next;
        }
    }
    let end=crate::render::grid_boundary(plan.duration_us,OUTPUT_RATE)?;
    let (first,last)=if let Some(s)=selection {((s.first_frame*u64::from(OUTPUT_RATE)).div_ceil(u64::from(s.fps)),((s.first_frame+s.frame_count)*u64::from(OUTPUT_RATE)).div_ceil(u64::from(s.fps)).min(end))}else{(0,end)};
    let input_base=if let Some(s)=selection {let mut count=0;for clip in &plan.clips {let (a,n,_)=crate::render::clip_grid(clip,s.fps)?;if a< s.first_frame+s.frame_count && a+n>s.first_frame {count+=1;}}count}else{1};
    let mut audio=AudioRenderPlan {tracks:tracks.clone(),first_sample:first,total_samples:last-first,input_base,..Default::default()};
    crate::project::require(input_base+tracks.len()<=if selection.is_some(){crate::preview::MAX_INPUTS}else{3},"audio.inputs","Audio plan exceeds file input limits; request a shorter or simpler preview")?;
    // Count first: an over-budget edit allocates no intersection vector.
    for collect in [false,true] {
        let mut span_count=0_usize;
        let mut append=|span:AudioSpan|->crate::CoreResult<()> {
            span_count=span_count.checked_add(1).filter(|n|*n<=MAX_AUDIO_SPANS).ok_or_else(||crate::project::invalid("audio.spans","Audio plan exceeds 16384 intersections; retain fewer clips or timing segments"))?;
            if collect {audio.spans.push(span);} Ok(())
        };
    for (track_index,track) in tracks.iter().enumerate() {
        let input=input_base+track_index;
        if collect {audio.inputs.push(AudioInput{path:track.path.clone(),kind:track.kind,index:input});}
        let measured=intervals(&track.track)?;
        for (clip_index,clip) in plan.clips.iter().enumerate() {
            let (clip_first,clip_count,_)=crate::render::clip_grid(clip,OUTPUT_RATE)?;
            let low=clip_first.max(first);let high=(clip_first+clip_count).min(last);if low>=high {continue;}
            let (domain_clip,domain_last)=clip_domains[clip_index];let run_start=&plan.clips[domain_clip];let run_end=&plan.clips[domain_last];
            let run_first=crate::render::grid_boundary(run_start.output_start_us,OUTPUT_RATE)?;
            let run_end_sample=crate::render::grid_boundary(run_end.output_start_us+run_end.source_end_us-run_end.source_start_us,OUTPUT_RATE)?;
            let mut cursor=low;
            for (segment_index,interval) in measured.iter().enumerate() {
                let shift=checked(i128::from(clip.output_start_us).checked_sub(i128::from(clip.source_start_us)))?;
                let boundary=|time:i128|->crate::CoreResult<i128>{Ok(ceil_ratio(checked(checked(time.checked_add(checked(shift.checked_mul(interval.denominator))?))?.checked_mul(i128::from(OUTPUT_RATE)))?,checked(1_000_000_i128.checked_mul(interval.denominator))?))};
                let domain_first=boundary(interval.start)?.max(i128::from(run_first));
                let domain_end=boundary(interval.end)?.min(i128::from(run_end_sample));
                let a=domain_first.max(i128::from(low));let b=domain_end.min(i128::from(high));if a>=b {continue;}
                let a=u64::try_from(a).map_err(|_|crate::project::invalid("audio.spans","Audio sample boundary overflow"))?;
                let b=u64::try_from(b).map_err(|_|crate::project::invalid("audio.spans","Audio sample boundary overflow"))?;
                if cursor<a {append(AudioSpan{kind:track.kind,output_first_sample:cursor-first,sample_count:a-cursor,source:None,input:None})?;}
                let frames=i128::from(interval.end_frame-interval.start_frame);
                let duration=interval.end-interval.start;
                let denominator=checked(i128::from(OUTPUT_RATE).checked_mul(duration))?;
                let elapsed=checked(checked(checked(checked(i128::from(a).checked_mul(1_000_000))?.checked_sub(checked(shift.checked_mul(i128::from(OUTPUT_RATE)))?))?.checked_mul(interval.denominator))?.checked_sub(checked(interval.start.checked_mul(i128::from(OUTPUT_RATE)))?))?;
                let position=checked(checked(i128::from(interval.start_frame).checked_mul(denominator))?.checked_add(checked(elapsed.checked_mul(frames))?))?;
                let step=checked(checked(frames.checked_mul(1_000_000))?.checked_mul(interval.denominator))?;
                let source_bound=|time:u64|->crate::CoreResult<u64>{
                    let time=checked(i128::from(time).checked_mul(interval.denominator))?.clamp(interval.start,interval.end);
                    let position=checked(i128::from(interval.start_frame).checked_add(ceil_ratio(checked(checked(time.checked_sub(interval.start))?.checked_mul(frames))?,duration)))?;
                    u64::try_from(position).map_err(|_|crate::project::invalid("audio.spans","Source domain overflow"))
                };
                let domain_first=u64::try_from(domain_first).map_err(|_|crate::project::invalid("audio.spans","Output domain overflow"))?;
                let domain_end=u64::try_from(domain_end).map_err(|_|crate::project::invalid("audio.spans","Output domain overflow"))?;
                append(AudioSpan{kind:track.kind,output_first_sample:a-first,sample_count:b-a,source:Some(AudioSourceSpan{domain:domain_clip*1024+segment_index,domain_first_sample:domain_first,domain_end_sample:domain_end,source_start_frame:source_bound(run_start.source_start_us)?,source_end_frame:source_bound(run_end.source_end_us)?,position_numerator:position,position_denominator:denominator,step_numerator:step,step_denominator:denominator}),input:Some(input)})?;
                cursor=b;
            }
            if cursor<high {append(AudioSpan{kind:track.kind,output_first_sample:cursor-first,sample_count:high-cursor,source:None,input:None})?;}
        }
    }
        if !collect {audio.spans.try_reserve_exact(span_count).map_err(|_|crate::project::invalid("audio.spans","Could not allocate the bounded audio plan; request a shorter range"))?;}
    }
    normalize::compile_windows(&mut audio,selection.is_some())?;
    Ok(audio)
}
