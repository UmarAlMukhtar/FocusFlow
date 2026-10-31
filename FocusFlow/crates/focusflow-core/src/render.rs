pub mod cursor_window;
pub mod follow;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt::{self, Write}};
use crate::{edits::{self, MappedClip}, project::{invalid, ProjectDocument, ZoomElement}, CoreResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportPreset { SmallFile, Balanced, HighQuality }

impl ExportPreset {
    pub fn crf(self) -> u8 {
        match self { Self::SmallFile => 28, Self::Balanced => 25, Self::HighQuality => 22 }
    }
    pub fn fps(self) -> u32 {
        match self { Self::SmallFile | Self::Balanced => 30, Self::HighQuality => 60 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenderPlan {
    pub project_id: String,
    pub revision: u64,
    pub duration_us: u64,
    pub clips: Vec<MappedClip>,
    pub camera: Vec<CameraInterval>,
    pub clicks: Vec<OutputClick>,
    pub preset: ExportPreset,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub follow: Option<follow::FollowPlan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraInterval {
    pub start_us: u64,
    pub end_us: u64,
    pub from: CameraPose,
    pub to: CameraPose,
}

/// Capture-relative normalized center and magnification, shared by preview and export.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CameraPose { pub x: f64, pub y: f64, pub scale: f64 }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputClick { pub at_us: u64, pub x: f64, pub y: f64 }

const MAX_OUTPUT_CLICKS: usize = 8192;
const MAX_CAMERA_INTERVALS: usize = 65_536;
const MAX_GRAPH_BYTES: usize = 32 * 1024 * 1024;
// A strict serialized Click needs more than 20 bytes, so every stored 8 MiB document fits.
const MAX_SOURCE_CLICKS: usize = crate::project::MAX_JSON_BYTES as usize / 20;

fn budget(count: Option<usize>, limit: usize, field: &str) -> CoreResult<usize> {
    count.filter(|count| *count <= limit).ok_or_else(|| invalid(field,
        format!("Render complexity exceeds the {limit} limit; reduce retained clips or interactions")))
}

/// Check each formatted piece before growing either a graph or a camera expression.
struct GraphText { text: String, limit: usize }
impl GraphText {
    fn new(limit: usize) -> Self { Self { text: String::new(), limit } }
    fn append(&mut self, args: fmt::Arguments<'_>) -> CoreResult<()> {
        self.write_fmt(args).map_err(|_| invalid("render.graphBytes",
            format!("Render graph exceeds {} bytes; reduce retained clips or interactions", self.limit)))
    }
    fn chain(&mut self, args: fmt::Arguments<'_>) -> CoreResult<()> {
        if !self.text.is_empty() { self.append(format_args!(";\n"))?; }
        self.append(args)
    }
}
impl Write for GraphText {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.text.len().checked_add(value.len()).is_none_or(|size| size > self.limit) { return Err(fmt::Error); }
        self.text.push_str(value);
        Ok(())
    }
}

const CENTER: CameraPose = CameraPose { x: 0.5, y: 0.5, scale: 1.0 };

impl CameraInterval {
    /// The desktop renderer's cosine easing, with exact endpoint values.
    pub fn pose_at(&self, at_us: u64) -> CameraPose {
        if at_us <= self.start_us { return self.from; }
        if at_us >= self.end_us { return self.to; }
        interpolate(self.from, self.to, at_us - self.start_us, self.end_us - self.start_us)
    }
}

fn interpolate(from: CameraPose, to: CameraPose, elapsed: u64, duration: u64) -> CameraPose {
    let amount = 0.5 - 0.5 * (std::f64::consts::PI * elapsed as f64 / duration as f64).cos();
    CameraPose {
        x: from.x + (to.x - from.x) * amount,
        y: from.y + (to.y - from.y) * amount,
        scale: from.scale + (to.scale - from.scale) * amount,
    }
}

/// Compile one camera schedule, retaining symbolic focus only when follow is requested.
pub fn compile(doc: &ProjectDocument, preset: ExportPreset) -> CoreResult<RenderPlan> {
    budget(Some(doc.clicks.len()), MAX_SOURCE_CLICKS, "render.sourceClicks")?;
    let clips = edits::map_clips(doc)?;
    let camera_bound = budget(doc.zooms.len().checked_mul(7).and_then(|n| n.checked_add(clips.len())),
        MAX_CAMERA_INTERVALS, "render.camera")?;
    let mut follow=doc.zooms.iter().any(|z|z.mode==crate::project::ZoomMode::Follow).then(||follow::FollowPlan{
        phases:Vec::with_capacity(camera_bound),targets:doc.zooms.iter().map(|z|follow::Target{entry_us:z.start_us,fallback:[z.x,z.y]}).collect()});
    let mut source_clicks: Vec<_> = doc.clicks.iter().collect();
    source_clicks.sort_by_key(|click| click.at_us);
    let mut click_ranges = Vec::with_capacity(clips.len());
    let mut click_count = 0usize;
    for clip in &clips {
        let start = source_clicks.partition_point(|click| click.at_us < clip.source_start_us);
        let end = source_clicks.partition_point(|click| click.at_us < clip.source_end_us);
        click_count = budget(click_count.checked_add(end - start), MAX_OUTPUT_CLICKS, "render.clicks")?;
        click_ranges.push(start..end);
    }
    // Conservative upper bounds cover numeric text, repeated camera expressions and audio chains.
    // shortcut: conservative estimates can reject a smaller actual graph; refine when real projects need it.
    let graph_bound = clips.len().checked_mul(2048)
        .and_then(|n| n.checked_add(click_count.checked_mul(2048)?))
        .and_then(|n| n.checked_add(camera_bound.checked_mul(4096)?));
    budget(graph_bound, MAX_GRAPH_BYTES, "render.graphBytes")?;
    let zoom_indices:BTreeMap<_,_>=doc.zooms.iter().enumerate().map(|(i,z)|(z.id.as_str(),i)).collect();
    let mut zooms: BTreeMap<&str, Vec<&ZoomElement>> = BTreeMap::new();
    for zoom in &doc.zooms { zooms.entry(&zoom.clip_id).or_default().push(zoom); }
    for targets in zooms.values_mut() { targets.sort_by_key(|zoom| zoom.start_us); }
    let mut camera = Vec::new();
    let mut clicks = Vec::with_capacity(click_count);
    let mut duration_us = 0;
    for (clip_index,(clip, click_range)) in clips.iter().zip(click_ranges).enumerate() {
        let mut focus=follow::Endpoint::fixed(CENTER);
        // map_clips validates these differences and their cumulative sum against 24 hours.
        let output_time = |source_us: u64| clip.output_start_us + (source_us - clip.source_start_us);
        let clip_end = output_time(clip.source_end_us);
        for click in &source_clicks[click_range] {
            clicks.push(OutputClick { at_us: output_time(click.at_us), x: click.x, y: click.y });
        }
        let targets = zooms.get(clip.clip_id.as_str()).map(Vec::as_slice).unwrap_or(&[]);
        // A cut starts a new camera, even when the source ranges are adjacent or repeated.
        let mut cursor = clip.output_start_us;
        let mut pose = CENTER;
        let mut previous_complete = None;
        for (index, zoom) in targets.iter().enumerate() {
            let start = output_time(zoom.start_us);
            let end = output_time(zoom.end_us);
            let limit = targets.get(index + 1).map(|next| output_time(next.start_us)).unwrap_or(clip_end);
            let current = pose;
            push_interval(&mut camera, &mut cursor, &mut pose, start, current, start, &mut follow, &mut focus, None, clip, clip_index);
            let chained = previous_complete.is_some_and(|complete| start <= complete);
            // Both persisted settings affect chained motion; zero means an instantaneous change.
            let intro = if chained { zoom.intro_us.min(doc.camera.pan_transition_us) } else { zoom.intro_us };
            // Very short active spans still reach their own target before their hold/outro.
            let arrival = start.checked_add(intro).ok_or_else(|| invalid("zooms", "Intro time overflow"))?.min(end);
            let target = CameraPose { x: zoom.x, y: zoom.y, scale: zoom.scale };
            let target_focus=if zoom.mode==crate::project::ZoomMode::Follow {
                follow::Endpoint::Follow{zoom_index:zoom_indices[zoom.id.as_str()]}
            } else {follow::Endpoint::fixed(target)};
            push_interval(&mut camera, &mut cursor, &mut pose, arrival, target, limit, &mut follow, &mut focus, Some(target_focus), clip, clip_index);
            if zoom.mode==crate::project::ZoomMode::Follow {focus=target_focus;}

            if chained {
                let settle_end = arrival.checked_add(doc.camera.settle_us)
                    .ok_or_else(|| invalid("camera.settleUs", "Settle time overflow"))?.min(end);
                push_interval(&mut camera, &mut cursor, &mut pose, settle_end, target, limit, &mut follow, &mut focus, Some(target_focus), clip, clip_index);
                if zoom.mode==crate::project::ZoomMode::Follow {focus=target_focus;}
            }
            push_interval(&mut camera, &mut cursor, &mut pose, end, target, limit, &mut follow, &mut focus, Some(target_focus), clip, clip_index);
            if zoom.mode==crate::project::ZoomMode::Follow {focus=target_focus;}
            let hold_end = end.checked_add(zoom.hold_us).ok_or_else(|| invalid("zooms", "Hold time overflow"))?;
            let complete = hold_end.checked_add(zoom.outro_us).ok_or_else(|| invalid("zooms", "Outro time overflow"))?;
            push_interval(&mut camera, &mut cursor, &mut pose, hold_end, target, limit, &mut follow, &mut focus, Some(target_focus), clip, clip_index);
            focus=follow::Endpoint::PreviousBoundary;
            push_interval(&mut camera, &mut cursor, &mut pose, complete, CENTER, limit, &mut follow, &mut focus, Some(follow::Endpoint::fixed(CENTER)), clip, clip_index);
            previous_complete = Some(complete);
        }
        let current = pose;
        push_interval(&mut camera, &mut cursor, &mut pose, clip_end, current, clip_end, &mut follow, &mut focus, None, clip, clip_index);
        duration_us = clip_end;
    }
    if let Some(f)=&follow {f.check()?;}
    Ok(RenderPlan { project_id: doc.project_id.clone(), revision: doc.revision, duration_us,
        clips, camera, clicks, preset, follow })
}

fn push_interval(camera: &mut Vec<CameraInterval>, cursor: &mut u64, pose: &mut CameraPose,
    end_us: u64, to: CameraPose, limit_us: u64, follow:&mut Option<follow::FollowPlan>,
    focus:&mut follow::Endpoint,target:Option<follow::Endpoint>,clip:&MappedClip,clip_index:usize) {
    if *cursor >= limit_us { return; }
    let destination=target.unwrap_or(*focus);
    if end_us <= *cursor {
        if let Some(f)=follow {f.phases.push(follow::Phase{clip_index,start_us:*cursor,end_us:*cursor,source_start_us:clip.source_start_us,output_start_us:clip.output_start_us,from:*focus,to:destination,weight:1.0,scale_from:pose.scale,scale_to:to.scale});}
        *focus=destination;
        *pose = to;
        return;
    }
    let end = end_us.min(limit_us);
    // A later target retimes this shortened cosine interval, preserving the interrupted pose.
    let endpoint = if end == end_us { to } else { interpolate(*pose, to, end - *cursor, end_us - *cursor) };
    if let Some(f)=follow {f.phases.push(follow::Phase{clip_index,start_us:*cursor,end_us:end,source_start_us:clip.source_start_us,output_start_us:clip.output_start_us,from:*focus,to:destination,
        weight:if end==end_us {1.0}else{follow::ease((end-*cursor) as f64/(end_us-*cursor) as f64)},scale_from:pose.scale,scale_to:endpoint.scale});}
    *focus=follow::Endpoint::PreviousBoundary;
    camera.push(CameraInterval { start_us: *cursor, end_us: end, from: *pose, to: endpoint });
    *cursor = end;
    *pose = endpoint;
}

/// Produce a graph from the same mapped clips, camera intervals and clicks used by preview.
pub fn filter_graph(snapshot: &crate::store::RenderSnapshot, plan: &RenderPlan, use_audio: bool) -> CoreResult<String> {
    validate_plan(snapshot, plan)?;
    graph(snapshot, plan, use_audio, None, None)
}

pub fn preview_filter_graph(snapshot: &crate::store::RenderSnapshot, plan: &RenderPlan,
    selection: &crate::preview::PreviewSelection, use_audio: bool) -> CoreResult<String> {
    validate_plan(snapshot, plan)?;
    graph(snapshot, plan, use_audio, Some(selection), None)
}

fn graph(snapshot: &crate::store::RenderSnapshot, plan: &RenderPlan, use_audio: bool,
    selection: Option<&crate::preview::PreviewSelection>, window: Option<&cursor_window::CursorWindow>) -> CoreResult<String> {
    crate::project::require((!snapshot.document.cursor_settings.enabled && plan.follow.is_none()) || window.is_some(), "cursor", "Cursor overlay or follow requires the prepared window renderer")?;
    let selected = selection.filter(|_|window.is_none()).map(|s| crate::preview::inputs(snapshot, plan, s, use_audio)).transpose()?;
    let audio = if selected.is_none() && window.is_none() { crate::audio::render_audio(snapshot, use_audio)? } else { None };
    let width = (snapshot.document.video.width / 2 * 2).max(2);
    let height = (snapshot.document.video.height / 2 * 2).max(2);
    let fps = plan.preset.fps();
    let mut graph = GraphText::new(if window.is_some(){2*1024*1024}else{MAX_GRAPH_BYTES});
    let mut video_labels = String::new();
    let mut video_count = 0;
    for (index, clip) in plan.clips.iter().enumerate() {
        if window.is_some_and(|w|w.clip!=index) { continue; }
        let duration = clip.source_end_us - clip.source_start_us;
        let end = clip.output_start_us + duration;
        // Allocate on one cumulative grid: independently rounded clips accumulate frame drift.
        let selected_span = selected.as_ref().and_then(|s| s.video.iter().enumerate().find(|(_, span)| span.clip == index));
        let (first, count, source_first) = if let Some(w)=window { (w.first,w.count,w.source_first) } else if selected.is_some() {
            let Some((_, span)) = selected_span else { continue; };
            (span.first, span.count, span.source_first)
        } else { clip_grid(clip, fps)? };
        if count == 0 { continue; }
        let source_end = source_first.checked_add(count).ok_or_else(|| invalid("plan", "Source frame overflow"))?;
        let clock_start = first as f64 / fps as f64;
        let branch_duration = format!("{:.9}", count as f64 / fps as f64);
        let mut input = format!("vc{index}");
        let initial_scale=if window.is_some(){String::new()}else{format!(",scale={width}:{height}")};
        // Normalize before scaling so drawing ON/OFF use identical source pixels.
        // fps before indexed trim samples the preceding source-grid frame even for subframe EOF cuts.
        if let Some((input_index, _)) = selected_span.map(|(i,_)|(i,())).or_else(||window.map(|_|(cursor_window::VIDEO_INPUT,()))) {
            let source_time = source_first as f64 / fps as f64;
            graph.chain(format_args!("[{input_index}:v]fps={fps}:round=up:eof_action=pass:start_time={source_time:.9},tpad=stop_mode=clone:stop_duration=1,trim=start_pts={source_first}:end_pts={source_end},setpts=PTS-STARTPTS,format=rgba{initial_scale}[{input}]"))?;
        } else {
            graph.chain(format_args!("[0:v]fps={fps}:round=up:eof_action=pass,tpad=stop_mode=clone:stop_duration=1,trim=start_frame={source_first}:end_frame={source_end},setpts=PTS-STARTPTS,format=rgba{initial_scale}[{input}]"))?;
        }
        if let Some(w)=window {
            if w.atlas.is_some() {graph.chain(format_args!("{}",w.graph))?;input="cursor_composed".into();}
            graph.chain(format_args!("[{input}]scale={width}:{height}[cursor_scaled]"))?;input="cursor_scaled".into();
        }
        video_labels.push_str(&format!("[v{index}]"));
        video_count += 1;
        let window_start=if window.is_some(){(first*1_000_000/fps as u64).saturating_sub(300_000).max(clip.output_start_us)}else{clip.output_start_us};
        let window_end=if window.is_some(){((first+count)*1_000_000).div_ceil(fps as u64).min(end)}else{end};
        let click_start = plan.clicks.partition_point(|click| click.at_us < window_start);
        let click_end = plan.clicks.partition_point(|click| click.at_us < window_end);
        for (click_index, click) in plan.clicks[click_start..click_end].iter().enumerate() {
            let start = format!("{:.9}", click.at_us as f64 / 1_000_000.0 - clock_start);
            let stop = format!("{:.9}", (click.at_us + 300_000).min(end) as f64 / 1_000_000.0 - clock_start);
            let ring = format!("ring{index}_{click_index}");
            let next = format!("click{index}_{click_index}");
            // Draw in capture space so the same zoom/pan transforms both the ring and video.
            graph.chain(format_args!("color=c=white@0:s=32x32:r={fps}:d={branch_duration},format=rgba,geq=r=255:g=255:b=255:a='if(between(hypot(X-15.5,Y-15.5),11,14),229*(0.5+0.5*cos(PI*clip((T-({start}))/0.3,0,1))),0)'[{ring}]"))?;
            graph.chain(format_args!("[{input}][{ring}]overlay=x={}:y={}:enable='gte(t,{start})*lt(t,{stop})':eof_action=pass:format=rgb:alpha=straight[{next}]",
                click.x * width as f64 - 16.0, click.y * height as f64 - 16.0))?;
            input = next;
        }
        let camera_clock=if window.is_some(){first*1_000_000/fps as u64}else{clip.output_start_us};
        let camera_start = plan.camera.partition_point(|iv| iv.end_us <= camera_clock);
        let camera_end = plan.camera.partition_point(|iv| iv.start_us < window_end);
        let intervals = &plan.camera[camera_start..camera_end];
        if window.is_some() && plan.follow.is_none() {
            // Each axis contains its position tree and two zoom trees, including their fallback leaves.
            budget(intervals.len().checked_add(1).and_then(|n| n.checked_mul(3)),900,"render.graphBytes")?;
        }
        let remaining = if window.is_some(){(128*1024).min(graph.limit-graph.text.len())}else{MAX_GRAPH_BYTES - graph.text.len()};
        let (zoom,x,y)=if let Some(expressions)=window.and_then(|w|w.camera.as_ref()) {
            (expressions[0].clone(),expressions[1].clone(),expressions[2].clone())
        } else {
            (camera_expression(intervals,clock_start,|p|p.scale,remaining)?,camera_expression(intervals,clock_start,|p|p.x,remaining)?,camera_expression(intervals,clock_start,|p|p.y,remaining)?)
        };
        // d=1 is essential: larger values multiply video frames and duration.
        if window.is_some() {
            let x = camera_axis_expression(&x, &zoom, "iw", remaining)?;
            let y = camera_axis_expression(&y, &zoom, "ih", remaining)?;
            graph.chain(format_args!("[{input}]format=gbrap,zoompan=z='{zoom}':x='{x}':y='{y}':d=1:s={width}x{height}:fps={fps},setsar=1[v{index}]"))?;
        } else {
        graph.chain(format_args!("[{input}]format=gbrap,zoompan=z='{zoom}':x='clip(({x})*iw-iw/(2*({zoom})),0,iw-iw/({zoom}))':y='clip(({y})*ih-ih/(2*({zoom})),0,ih-ih/({zoom}))':d=1:s={width}x{height}:fps={fps},setsar=1[v{index}]"))?;
        }
    }
    // concat gives duplicate PTS for single-frame segments; the allocated frame order owns time.
    let styled = snapshot.document.frame_style != Default::default();
    let output = if styled { "frame_input" } else if selection.is_some() { "preview_composed" } else { "v" };
    graph.chain(format_args!("{video_labels}concat=n={video_count}:v=1:a=0,settb=1/{fps},setpts=N[{output}]"))?;
    if styled {
        let mut composition = crate::frame::composition(&snapshot.document.frame_style,
            snapshot.document.video.width, snapshot.document.video.height, fps, selection.map_or(plan.duration_us, |s| s.duration_us))?;
        if selection.is_some() { composition.truncate(composition.len() - 3); composition.push_str("[preview_composed]"); }
        graph.chain(format_args!("{composition}"))?;
    }
    if selection.is_some() {
        let g = crate::frame::geometry(&snapshot.document.frame_style, snapshot.document.video.width, snapshot.document.video.height)?;
        let (w,h) = crate::preview::dimensions(g.width,g.height);
        graph.chain(format_args!("[preview_composed]scale={w}:{h},setsar=1[v]"))?;
    }
    let audio_text=audio_graph(snapshot,plan,selected.as_ref(),audio)?;
    if !audio_text.is_empty(){graph.chain(format_args!("{audio_text}"))?;}
    Ok(graph.text)
}

pub(crate) fn validate_plan(snapshot: &crate::store::RenderSnapshot, plan: &RenderPlan) -> CoreResult<()> {
    if compile(&snapshot.document, plan.preset)? != *plan {
        return Err(invalid("plan", "Render plan does not match the immutable project snapshot"));
    }
    crate::paths::asset_path(&snapshot.project_dir, &snapshot.document.video.path)?;
    Ok(())
}

fn seconds(us: u64) -> String { format!("{}.{:06}", us / 1_000_000, us % 1_000_000) }

pub(crate) fn grid_boundary(us: u64, rate: u32) -> CoreResult<u64> {
    if rate == 0 { return Err(invalid("plan", "Media grid rate must be positive")); }
    us.checked_mul(rate as u64).and_then(|n| n.checked_add(999_999)).map(|n| n / 1_000_000)
        .ok_or_else(|| invalid("plan", "Media grid overflow"))
}

/// First output unit, allocated count and first source unit on a cumulative frame/sample grid.
pub(crate) fn clip_grid(clip: &MappedClip, rate: u32) -> CoreResult<(u64, u64, u64)> {
    let end = clip.output_start_us.checked_add(clip.source_end_us - clip.source_start_us)
        .ok_or_else(|| invalid("plan", "Clip duration overflow"))?;
    let first = grid_boundary(clip.output_start_us, rate)?;
    let count = grid_boundary(end, rate)? - first;
    let phase = first.checked_mul(1_000_000)
        .and_then(|n| n.checked_sub(clip.output_start_us.checked_mul(rate as u64)?))
        .ok_or_else(|| invalid("plan", "Media grid phase overflow"))?;
    let source_first = clip.source_start_us.checked_mul(rate as u64).and_then(|n| n.checked_add(phase))
        .ok_or_else(|| invalid("plan", "Source grid overflow"))? / 1_000_000;
    Ok((first, count, source_first))
}

fn camera_expression(intervals: &[CameraInterval], clock_start: f64, value: impl Fn(CameraPose) -> f64,
    limit: usize) -> CoreResult<String> {
    let last = intervals.last().ok_or_else(|| invalid("plan", "Retained clip has no camera interval"))?;
    let mut expression = GraphText::new(limit);
    let clock = format!("(out_time+{clock_start:.9})");
    expression.append(format_args!("if(lt({clock},{}),", seconds(last.end_us)))?;
    write_camera_decision(&mut expression, intervals, &clock, &value)?;
    expression.append(format_args!(",{})", value(last.to)))?;
    Ok(expression.text)
}

fn camera_axis_expression(position: &str, zoom: &str, dimension: &str, limit: usize) -> CoreResult<String> {
    let mut expression = GraphText::new(limit);
    expression.append(format_args!("clip(({position})*{dimension}-{dimension}/(2*({zoom})),0,{dimension}-{dimension}/({zoom}))"))?;
    Ok(expression.text)
}

fn write_camera_decision(out: &mut GraphText, intervals: &[CameraInterval], clock: &str,
    value: &impl Fn(CameraPose) -> f64) -> CoreResult<()> {
    if intervals.len() == 1 {
        let interval = &intervals[0];
        let from = value(interval.from);
        let to = value(interval.to);
        if from == to { return out.append(format_args!("{from}")); }
        return out.append(format_args!("({from})+(({to})-({from}))*(0.5-0.5*cos(PI*clip(({clock}-({}))/({}),0,1)))",
            seconds(interval.start_us), seconds(interval.end_us - interval.start_us)));
    }
    // A balanced tree keeps FFmpeg parser depth logarithmic; lt preserves half-open boundaries.
    let mid = intervals.len() / 2;
    out.append(format_args!("if(lt({clock},{}),", seconds(intervals[mid].start_us)))?;
    write_camera_decision(out, &intervals[..mid], clock, value)?;
    out.append(format_args!(","))?;
    write_camera_decision(out, &intervals[mid..], clock, value)?;
    out.append(format_args!(")"))
}

#[cfg(test)]
mod tests {
    use super::*;


    mod common { use crate as focusflow_core; include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/common/mod.rs")); }
    fn dense_camera_fixture(moving: bool) -> (common::TestDir, crate::store::RenderSnapshot, RenderPlan) {
        use crate::project::{CursorAsset, CursorMode, CursorStyle};
        let temp = common::TestDir::new(); let mut doc = common::fixture_project();
        doc.schema_version = 3; doc.video.cursor = CursorMode::Separate;
        doc.cursor = Some(CursorAsset { index_path: "index.json".into() });
        doc.cursor_settings.enabled = true; doc.cursor_settings.style = CursorStyle::Arrow;
        std::fs::write(temp.path().join("index.json"), serde_json::json!({"schemaVersion":1,"width":100,"height":50,"durationUs":5000000,"samplesPath":"samples.ndjson","shapes":[]}).to_string()).unwrap();
        std::fs::write(temp.path().join("samples.ndjson"), serde_json::json!({"atUs":0,"x":0.5,"y":0.5,"visible":true,"shapeId":null,"discontinuity":false}).to_string()).unwrap();
        let mut snapshot = crate::store::RenderSnapshot { project_dir: temp.path().into(), document: doc, prepared_focus: None, prepared_cursor: None };
        snapshot.prepare_cursor(&std::sync::atomic::AtomicBool::new(false)).unwrap();
        let mut plan = compile(&snapshot.document, ExportPreset::Balanced).unwrap();
        let count = if moving { 250 } else { 300 };
        let from = if moving { CameraPose { x: 0.12345678901234567, y: 0.23456789012345678, scale: 1.2345678901234567 } } else { CENTER };
        let to = if moving { CameraPose { x: 0.8765432109876543, y: 0.7654321098765432, scale: 3.7654321098765432 } } else { CENTER };
        plan.camera = (0..count).map(|i|CameraInterval { start_us: i*5_000_000/count, end_us: (i+1)*5_000_000/count, from, to }).collect();
        (temp, snapshot, plan)
    }
    #[test]
    fn complete_camera_expressions_reject_byte_overflow_even_when_components_fit() {
        final_camera_limit_regression(true);
    }
    #[test]
    fn complete_camera_expressions_reject_leaf_only_overflow() {
        final_camera_limit_regression(false);
    }
    fn final_camera_limit_regression(moving: bool) {
        let (_temp, snapshot, plan) = dense_camera_fixture(moving);
        let z = camera_expression(&plan.camera, 0.0, |p|p.scale, 128*1024).unwrap();
        let x = camera_expression(&plan.camera, 0.0, |p|p.x, 128*1024).unwrap();
        let composed = format!("clip(({x})*iw-iw/(2*({z})),0,iw-iw/({z}))");
        let leaves = 3*(plan.camera.len()+1);
        if moving { assert!(composed.len()>128*1024); assert!(leaves<=900); }
        else { assert!(composed.len()<128*1024); assert!(leaves>900); }
        println!("moving={moving}: component bytes={}, final bytes={}, final decision leaves={leaves}", x.len(), composed.len());
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let window = cursor_window::next(&snapshot, &plan, 0, 150, &cancel).unwrap();
        let failure = window_graph(&snapshot, &plan, None, &window).err().expect("unbounded final expression was accepted");
        assert_eq!(failure.field.as_deref(), Some("render.graphBytes"));
        assert!(failure.message.contains(if moving {"131072"} else {"900"}));
        let mut first = 0;
        while first < 150 {
            let (bounded, graph) = cursor_window::next_with_graph(&snapshot, &plan, first, 150, None, &cancel).unwrap();
            assert!(bounded.count < 150);
            assert_eq!(bounded.first, first);
            assert_eq!(bounded.source_first, first);
            let mut lengths = Vec::new();
            let mut leaf_counts = Vec::new();
            for key in ["zoompan=z='", ":x='", ":y='"] {
                let scalar = graph.split("zoompan=").last().unwrap();
                let scalar = if key == "zoompan=z='" { scalar.strip_prefix("z='").unwrap() } else { scalar.split(key).nth(1).unwrap() };
                let scalar = scalar.split("'").next().unwrap();
                assert!(scalar.len() <= 128*1024);
                // Each emitted component is a binary if tree: leaves = decisions + 1.
                let trees = if key == "zoompan=z='" { 1 } else { 3 };
                let leaves = scalar.matches("if(").count()+trees;
                assert!(leaves <= 900);
                leaf_counts.push(leaves);
                lengths.push(scalar.len());
            }
            println!("moving={moving}: bounded first={first}, frames={}, z/x/y bytes={lengths:?}, decision leaves={leaf_counts:?}", bounded.count);
            first += bounded.count;
        }
        assert_eq!(first, 150);
    }
    #[test]
    fn graph_text_checks_each_formatted_piece_before_growth() {
        let mut out = GraphText::new(5);
        out.append(format_args!("{}{}", "ab", "cde")).unwrap();
        assert_eq!(out.text, "abcde");
        assert_eq!(out.append(format_args!("f")).unwrap_err().field.as_deref(), Some("render.graphBytes"));
        assert_eq!(out.text, "abcde");
        assert_eq!(budget(None, MAX_GRAPH_BYTES, "render.graphBytes").unwrap_err().field.as_deref(), Some("render.graphBytes"));
        let intervals = [CameraInterval { start_us: 0, end_us: 1_000_000, from: CENTER,
            to: CameraPose { scale: 2.0, ..CENTER } }];
        assert_eq!(camera_expression(&intervals, 0.0, |p| p.scale, 5).unwrap_err().field.as_deref(), Some("render.graphBytes"));
    }
}

pub(crate) fn window_graph(snapshot:&crate::store::RenderSnapshot,plan:&RenderPlan,selection:Option<&crate::preview::PreviewSelection>,window:&cursor_window::CursorWindow)->CoreResult<String>{
    if plan.follow.is_some() {snapshot.prepared_focus.as_ref().ok_or_else(||invalid("render.follow","Focus was not prepared"))?.validate(plan)?;}
    graph(snapshot,plan,false,selection,Some(window))
}

fn audio_graph(snapshot:&crate::store::RenderSnapshot,plan:&RenderPlan,selected:Option<&crate::preview::Inputs>,audio:Option<crate::audio::RenderAudio>)->CoreResult<String>{
 let mut graph=GraphText::new(MAX_GRAPH_BYTES);
    if let Some(selected) = &selected {
        if let Some(wav) = &selected.audio {
            let rate = wav.sample_rate;
            let mut labels = String::new();
            for (i, span) in selected.audio_spans.iter().enumerate() {
                let count = span.count;
                if let Some(input) = span.input {
                    graph.chain(format_args!("[{input}:a]asettb=1/{rate},atrim=start_pts={}:end_pts={},asetpts=PTS-STARTPTS,adelay={}S:all=1,apad=whole_len={count},atrim=end_sample={count},asetpts=PTS-STARTPTS[a{i}]", span.low, span.high, span.delay))?;
                } else {
                    graph.chain(format_args!("anullsrc=r={rate}:cl={}c,atrim=end_sample={count},asetpts=PTS-STARTPTS[a{i}]", wav.channels))?;
                }
                labels.push_str(&format!("[a{i}]"));
            }
            graph.chain(format_args!("{labels}concat=n={}:v=0:a=1,asettb=1/{rate},asetpts=N[a]", selected.audio_spans.len()))?;
        }
    }
    if let Some(audio) = audio {
        let rate = audio.sample_rate;
        let offset = snapshot.document.timing.audio_offset_us.unwrap_or(0);
        let shift = grid_boundary(offset.unsigned_abs(), rate)?;
        let align = if offset < 0 { format!("atrim=start_sample={shift},asetpts=PTS-STARTPTS,") }
            else if offset > 0 { format!("adelay={shift}S:all=1,") } else { String::new() };
        // Align once, then pad on the source timeline before selecting retained native samples.
        graph.chain(format_args!("[1:a]asetpts=PTS-STARTPTS,{align}apad,atrim=end_sample={}[aligned]",
            grid_boundary(snapshot.document.video.duration_us, rate)?))?;
        let mut spans = Vec::new();
        for (i, clip) in plan.clips.iter().enumerate() {
            let (_, count, source_first) = clip_grid(clip, rate)?;
            if count > 0 { spans.push((i, count, source_first)); }
        }
        let labels: String = spans.iter().map(|(i, _, _)| format!("[as{i}]")).collect();
        graph.chain(format_args!("[aligned]asplit={}{labels}", spans.len()))?;
        for (i, count, source_first) in &spans {
            let source_end = source_first.checked_add(*count).ok_or_else(|| invalid("plan", "Source sample overflow"))?;
            graph.chain(format_args!("[as{i}]atrim=start_sample={source_first}:end_sample={source_end},asetpts=PTS-STARTPTS[a{i}]"))?;
        }
        let labels: String = spans.iter().map(|(i, _, _)| format!("[a{i}]")).collect();
        graph.chain(format_args!("{labels}concat=n={}:v=0:a=1,asettb=1/{rate},asetpts=N[a]", spans.len()))?;
    }
 Ok(graph.text)
}

pub(crate) fn mux_audio_graph(snapshot:&crate::store::RenderSnapshot,plan:&RenderPlan,selection:Option<&crate::preview::PreviewSelection>)->CoreResult<String>{
 let selected=selection.map(|s|crate::preview::inputs(snapshot,plan,s,true)).transpose()?;
 let audio=if selected.is_none(){crate::audio::render_audio(snapshot,true)?}else{None};
 audio_graph(snapshot,plan,selected.as_ref(),audio)
}
