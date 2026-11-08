//! Current Windows queries are sampled reconstructions, never compositor history.
use crate::{
    capture::{
        windows::{NullOnly, WindowsOwnedClockV1},
        UniqueJson,
    },
    cursor::{self, CursorShape},
    interaction_timeline::{self as timeline, *},
    legacy::{self, LegacyDefaults},
    project::{self, invalid, require},
    CoreResult,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
};

pub const MAX_POSE_RECORD_BYTES: usize = 2048;
pub const MAX_INPUT_RECORD_BYTES: usize = 1024;
pub const MAX_INTERACTION_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RETAINED_BYTES: usize = 128 * 1024 * 1024;
pub const OBSERVATIONS_PATH: &str = "cursor/observations.ndjson";
pub const INPUT_OBSERVATIONS_PATH: &str = "cursor/input-observations.ndjson";

macro_rules! literals {
    ($name:ident { $($v:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        pub enum $name { $(#[serde(rename=$s)] $v),+ }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D:serde::Deserializer<'de>>(de:D)->Result<Self,D::Error> {
                let value=String::deserialize(de)?;
                match value.as_str() {$($s=>Ok(Self::$v),)+ _=>Err(serde::de::Error::unknown_variant(&value,&[$($s),+]))}
            }
        }
    };
}
literals!(GeometryModel { DwmExtendedFrameTranslationV1 => "dwmExtendedFrameTranslationV1" });
literals!(ModelProfile { Popup => "pmv2_96_single_monitor_popup", Decorated => "pmv2_96_single_monitor_decorated" });
literals!(ObservationPolicy { WindowsCurrentQueryBracket => "windowsCurrentQueryBracket" });
literals!(ReconstructionPolicy { CausalAfterBracket => "causalAfterBracket" });
literals!(AnimationPhase { Unknown => "unknown" });
literals!(QualityObservation { SampledCurrent => "sampledCurrent" });
literals!(QualityGeometry { EmpiricalDwmTranslation => "empiricalDwmTranslation" });
literals!(ButtonCoverage { SampledEdges => "sampledEdges" });
literals!(DragCoverage { SampledPath => "sampledPath" });
literals!(StopKind { Stop => "stop", SourceEnd => "sourceEnd", SamplerFailure => "samplerFailure", Budget => "budget" });
literals!(DragEndKind { ObservedRelease => "observedRelease", Interrupted => "interrupted" });
literals!(DragEndReason { Release => "release", Barrier => "barrier", SourceCutoff => "sourceCutoff", Stop => "stop", Budget => "budget" });
fn required<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    de: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(de)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CursorObservationIndexV2 {
    pub schema_version: u32,
    pub width: u32,
    pub height: u32,
    pub duration_us: u64,
    pub observations_path: String,
    pub input_observations_path: String,
    pub shapes: Vec<CursorShape>,
    pub clock: WindowsOwnedClockV1,
    pub model: GeometryModel,
    pub profile: ModelProfile,
    pub observation: ObservationPolicy,
    pub reconstruction: ReconstructionPolicy,
    pub max_hold_us: u64,
    pub spatial_error_px: NullOnly,
    pub frame_alignment_error_us: NullOnly,
    pub animation_phase: AnimationPhase,
    pub button_coverage: ButtonCoverage,
    pub drag_coverage: DragCoverage,
    pub terminal: ObservationTerminal,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowObservation {
    pub hwnd: String,
    pub pid: u32,
    pub tid: u32,
    pub client_origin: [i32; 2],
    pub client_size: [u32; 2],
    pub window_rect: [i32; 4],
    pub dwm_rect: [i32; 4],
    pub dpi: u32,
    pub observer_pmv2: bool,
    pub target_pmv2: bool,
    pub style: u32,
    pub ex_style: u32,
    pub visible: bool,
    pub minimized: bool,
    pub monitor_rect: [i32; 4],
    pub monitor_count: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PointerObservation {
    pub physical_position: [i32; 2],
    pub cursor_position: [i32; 2],
    pub flags: u32,
    pub handle: String,
    pub hit_hwnd: String,
    pub root_hwnd: String,
    pub hit_pid: u32,
    pub hit_tid: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum WindowsCursorRowV2 {
    #[serde(rename_all = "camelCase")]
    Observed {
        q0: String,
        q1: String,
        before: WindowObservation,
        after: WindowObservation,
        pointer_before: PointerObservation,
        pointer_after: PointerObservation,
        #[serde(deserialize_with = "required")]
        shape_id: Option<String>,
        barrier_revision: u64,
    },
    #[serde(rename_all = "camelCase")]
    Barrier {
        q0: String,
        q1: String,
        reason: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum WindowsInputRowV1 {
    #[serde(rename_all = "camelCase")]
    Observed {
        q0: String,
        q1: String,
        physical_position: [i32; 2],
        hit_hwnd: String,
        root_hwnd: String,
        hit_pid: u32,
        hit_tid: u32,
        // Physical high-bit states below map to logical buttons through this measured setting.
        buttons_swapped: bool,
        #[serde(deserialize_with = "required")]
        left: Option<bool>,
        #[serde(deserialize_with = "required")]
        right: Option<bool>,
        #[serde(deserialize_with = "required")]
        basis_pose_row: Option<u32>,
        barrier_revision_before: u64,
        barrier_revision_after: u64,
    },
    #[serde(rename_all = "camelCase")]
    Barrier {
        q0: String,
        q1: String,
        reason: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QpcBarrier {
    pub q0: String,
    pub q1: String,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservationTerminal {
    #[serde(deserialize_with = "required")]
    pub pose_barrier: Option<QpcBarrier>,
    #[serde(deserialize_with = "required")]
    pub input_barrier: Option<QpcBarrier>,
    pub source_cutoff_qpc: i64,
    pub stop_kind: StopKind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CursorObservationQuality {
    pub observation: QualityObservation,
    pub geometry: QualityGeometry,
    pub spatial_error_px: NullOnly,
    pub frame_alignment_error_us: NullOnly,
    pub animation_phase: AnimationPhase,
    pub button_coverage: ButtonCoverage,
    pub drag_coverage: DragCoverage,
    pub interaction_truncated: bool,
    pub message: String,
}
impl CursorObservationQuality {
    pub fn sampled(truncated: bool) -> Self {
        Self {observation:QualityObservation::SampledCurrent,geometry:QualityGeometry::EmpiricalDwmTranslation,spatial_error_px:NullOnly,frame_alignment_error_us:NullOnly,animation_phase:AnimationPhase::Unknown,button_coverage:ButtonCoverage::SampledEdges,drag_coverage:DragCoverage::SampledPath,interaction_truncated:truncated,message:"Cursor uses sampled Windows observations and an empirical window mapping. Frame alignment and animation phase are unmeasured; short presses and fast drag details can be missed.".into()}
    }
}

// Arrays are admitted only for declared collections and scalar tuples, never named structs.
pub(crate) fn decode_strict<T: DeserializeOwned>(bytes: &[u8]) -> CoreResult<T> {
    let UniqueJson(value) = serde_json::from_slice(bytes)
        .map_err(|e| invalid("observations", format!("Invalid strict JSON: {e}")))?;
    fn named(v: &serde_json::Value, key: &str) -> CoreResult<()> {
        match v {
            serde_json::Value::Array(a) => {
                require(
                    matches!(
                        key,
                        "shapes"
                            | "physicalPosition"
                            | "cursorPosition"
                            | "clientOrigin"
                            | "clientSize"
                            | "windowRect"
                            | "dwmRect"
                            | "monitorRect"
                            | "clips"
                            | "clicks"
                            | "zooms"
                            | "warnings"
                    ),
                    "observations",
                    "Named objects require JSON objects",
                )?;
                for v in a {
                    named(v, "")?;
                }
            }
            serde_json::Value::Object(o) => {
                for (k, v) in o {
                    named(v, k)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    named(&value, "")?;
    serde_json::from_value(value)
        .map_err(|e| invalid("observations", format!("Invalid observation fields: {e}")))
}
pub fn qpc(text: &str) -> CoreResult<i64> {
    require(
        !text.is_empty()
            && text.len() <= 19
            && text.bytes().all(|b| b.is_ascii_digit())
            && (text == "0" || !text.starts_with('0')),
        "qpc",
        "Expected canonical nonnegative signed-64-bit decimal QPC",
    )?;
    text.parse()
        .map_err(|_| invalid("qpc", "QPC exceeds signed-64-bit range"))
}
fn handle(text: &str, zero: bool) -> CoreResult<()> {
    require(
        !text.is_empty()
            && text.len() <= 20
            && text.bytes().all(|b| b.is_ascii_digit())
            && (text == "0" || !text.starts_with('0')),
        "hwnd",
        "Expected canonical unsigned decimal handle",
    )?;
    let v = text
        .parse::<u64>()
        .map_err(|_| invalid("hwnd", "Handle exceeds unsigned-64-bit range"))?;
    require(zero || v > 0, "hwnd", "Expected nonzero HWND")
}
fn bracket(a: &str, b: &str) -> CoreResult<[i64; 2]> {
    let a = qpc(a)?;
    let b = qpc(b)?;
    require(a <= b, "qpc", "Query bracket regresses")?;
    Ok([a, b])
}
impl WindowsCursorRowV2 {
    pub fn bracket(&self) -> CoreResult<[i64; 2]> {
        match self {
            Self::Observed { q0, q1, .. } | Self::Barrier { q0, q1, .. } => bracket(q0, q1),
        }
    }
}
impl WindowsInputRowV1 {
    pub fn bracket(&self) -> CoreResult<[i64; 2]> {
        match self {
            Self::Observed { q0, q1, .. } | Self::Barrier { q0, q1, .. } => bracket(q0, q1),
        }
    }
}
fn reason(s: &str) -> CoreResult<()> {
    require(
        !s.is_empty() && s.len() <= 256,
        "reason",
        "Barrier reason requires 1..256 UTF-8 bytes",
    )
}
fn terminal_valid(t: &ObservationTerminal, index: &CursorObservationIndexV2) -> CoreResult<()> {
    require(
        t.source_cutoff_qpc == qpc(&index.clock.cutoff_qpc)?,
        "terminal",
        "Terminal cutoff must equal the owned video clock",
    )?;
    for b in [&t.pose_barrier, &t.input_barrier].into_iter().flatten() {
        bracket(&b.q0, &b.q1)?;
        reason(&b.reason)?;
    }
    Ok(())
}
pub fn decode_observation_index(bytes: &[u8]) -> CoreResult<CursorObservationIndexV2> {
    require(
        bytes.len() as u64 <= cursor::MAX_INDEX_BYTES,
        "cursor.index",
        "Index exceeds 64 KiB",
    )?;
    let index: CursorObservationIndexV2 = decode_strict(bytes)?;
    index.validate()?;
    Ok(index)
}
impl CursorObservationIndexV2 {
    pub fn validate(&self) -> CoreResult<()> {
        #[cfg(test)]
        prepared_tests::INDEX_VALIDATIONS.with(|count| count.set(count.get() + 1));
        require(
            self.schema_version == 2,
            "cursor.index.schemaVersion",
            "Expected cursor observation index schema 2",
        )?;
        project::validate_video(self.width, self.height, self.duration_us)?;
        require(
            self.observations_path == OBSERVATIONS_PATH
                && self.input_observations_path == INPUT_OBSERVATIONS_PATH,
            "cursor.index",
            "Observation references must use the two fixed contained paths",
        )?;
        require(
            self.max_hold_us == 250_000,
            "cursor.index.maxHoldUs",
            "Expected causal hold policy 250000us",
        )?;
        let endpoint = crate::capture::windows::WindowsSavedEndpointV2 {
            provenance: crate::capture::windows::WindowsEndpointProvenance::SavedPacketFullGrid,
            end_pts: self.clock.completed_frames,
            time_base: crate::capture::TimeBase {
                numerator: 1,
                denominator: 30,
            },
            interval: crate::capture::Interval::HalfOpen,
        };
        self.clock.validate(&endpoint)?;
        require(
            self.duration_us == endpoint.duration_us()?,
            "cursor.index.durationUs",
            "Duration must equal the saved owned grid",
        )?;
        require(
            self.shapes.len() <= cursor::MAX_SHAPES,
            "cursor.shapes",
            "At most 256 shapes",
        )?;
        let mut decoded = 0usize;
        for (i, s) in self.shapes.iter().enumerate() {
            require(
                project::valid_id(&s.id)
                    && self.shapes[..i]
                        .iter()
                        .all(|p| p.id != s.id && p.path != s.path),
                "cursor.shapes",
                "Shape identities and paths must be unique",
            )?;
            require(
                s.path == format!("cursor/shapes/{}.pam", s.id),
                "cursor.shapes.path",
                "Expected fixed contained shape PAM reference",
            )?;
            require(
                (1..=512).contains(&s.width)
                    && (1..=512).contains(&s.height)
                    && s.hotspot_x < s.width
                    && s.hotspot_y < s.height,
                "cursor.shapes",
                "Invalid shape size or hotspot",
            )?;
            decoded += s.width as usize * s.height as usize * 4;
            require(
                decoded <= cursor::MAX_NATIVE_BYTES,
                "cursor.shapes",
                "Decoded image budget exceeds 32 MiB",
            )?;
        }
        terminal_valid(&self.terminal, self)
    }
    pub fn frequency(&self) -> CoreResult<i128> {
        Ok(i128::from(qpc(&self.clock.qpc_frequency)?))
    }
    /// Exact numerator over 10*F; never round or use floating point before admission.
    pub fn relative_numerator(&self, q: i64) -> CoreResult<i128> {
        i128::from(q)
            .checked_mul(10_000_000)
            .and_then(|v| {
                self.frequency().ok().and_then(|f| {
                    f.checked_mul(qpc(&self.clock.epoch_100ns).ok()?.into())
                        .and_then(|e| v.checked_sub(e))
                })
            })
            .ok_or_else(|| invalid("clock", "QPC conversion overflow"))
    }
    pub fn ceil_us(&self, q: i64) -> CoreResult<u64> {
        let n = self.relative_numerator(q)?;
        let d = 10 * self.frequency()?;
        require(
            n >= 0,
            "qpc",
            "Pre-epoch observation has no compatibility time",
        )?;
        u64::try_from((n + d - 1) / d).map_err(|_| invalid("qpc", "Time overflow"))
    }
    pub fn eligible(&self, b: [i64; 2]) -> CoreResult<bool> {
        let f = self.frequency()?;
        let lo = self.relative_numerator(b[0])?;
        let hi = self.relative_numerator(b[1])?;
        Ok(b[0] >= qpc(&self.clock.admission_qpc)?
            && lo >= 0
            && 30 * hi < i128::from(self.clock.completed_frames) * 10_000_000 * f
            && 30 * i128::from(b[1] - b[0]) <= f)
    }
}
fn validate_window(w: &WindowObservation) -> CoreResult<()> {
    handle(&w.hwnd, false)?;
    require(
        w.client_size
            .into_iter()
            .all(|n| (1..=i32::MAX as u32).contains(&n)),
        "window.clientSize",
        "Measured client dimensions must be positive signed-32-bit sizes",
    )?;
    for rect in [w.window_rect, w.dwm_rect, w.monitor_rect] {
        require(
            i64::from(rect[2]) - i64::from(rect[0]) > 0
                && i64::from(rect[3]) - i64::from(rect[1]) > 0,
            "window.rect",
            "Measured rectangles must be nonempty",
        )?;
    }
    Ok(())
}
fn validate_pointer(p: &PointerObservation) -> CoreResult<()> {
    handle(&p.handle, true)?;
    handle(&p.hit_hwnd, true)?;
    handle(&p.root_hwnd, true)
}
pub fn validate_observation_row(
    row: &WindowsCursorRowV2,
    index: &CursorObservationIndexV2,
) -> CoreResult<()> {
    row.bracket()?;
    match row {
        WindowsCursorRowV2::Barrier { reason: s, .. } => reason(s),
        WindowsCursorRowV2::Observed {
            before,
            after,
            pointer_before,
            pointer_after,
            shape_id,
            ..
        } => {
            validate_window(before)?;
            validate_window(after)?;
            validate_pointer(pointer_before)?;
            validate_pointer(pointer_after)?;
            require(
                shape_id
                    .as_ref()
                    .is_none_or(|id| index.shapes.iter().any(|s| s.id == *id)),
                "shapeId",
                "Unknown copied shape reference",
            )
        }
    }
}
fn family(w: &WindowObservation, index: &CursorObservationIndexV2) -> bool {
    let style = match index.profile {
        ModelProfile::Popup => 0x9000_0000,
        ModelProfile::Decorated => 0x10cf_0000,
    };
    w.pid != 0
        && w.tid != 0
        && w.observer_pmv2
        && w.target_pmv2
        && w.dpi == 96
        && w.monitor_count == 1
        && w.monitor_rect[0] == 0
        && w.monitor_rect[1] == 0
        && w.visible
        && !w.minimized
        && w.ex_style == 0
        && w.style == style
        && i64::from(w.dwm_rect[2]) - i64::from(w.dwm_rect[0]) >= i64::from(index.width) - 1
        && i64::from(w.dwm_rect[2]) - i64::from(w.dwm_rect[0]) <= i64::from(index.width)
        && i64::from(w.dwm_rect[3]) - i64::from(w.dwm_rect[1]) >= i64::from(index.height) - 1
        && i64::from(w.dwm_rect[3]) - i64::from(w.dwm_rect[1]) <= i64::from(index.height)
}
fn pose_basis<'a>(
    row: &'a WindowsCursorRowV2,
    index: &CursorObservationIndexV2,
) -> CoreResult<Option<(&'a WindowObservation, &'a PointerObservation)>> {
    validate_observation_row(row, index)?;
    if !index.eligible(row.bracket()?)? {
        return Ok(None);
    }
    let WindowsCursorRowV2::Observed {
        before,
        after,
        pointer_before: a,
        pointer_after: b,
        ..
    } = row
    else {
        return Ok(None);
    };
    if before != after
        || !family(before, index)
        || a != b
        || a.flags != 1
        || a.handle == "0"
        || a.physical_position != a.cursor_position
        || a.root_hwnd != before.hwnd
        || a.hit_pid != before.pid
        || a.hit_tid != before.tid
        || a.hit_hwnd == "0"
        || source_point(a.physical_position, before).is_none()
    {
        return Ok(None);
    }
    Ok(Some((before, a)))
}
fn source_point(p: [i32; 2], w: &WindowObservation) -> Option<[i32; 2]> {
    let x = i64::from(p[0]) - i64::from(w.dwm_rect[0]);
    let y = i64::from(p[1]) - i64::from(w.dwm_rect[1]);
    if x < 0
        || y < 0
        || x >= i64::from(w.dwm_rect[2]) - i64::from(w.dwm_rect[0])
        || y >= i64::from(w.dwm_rect[3]) - i64::from(w.dwm_rect[1])
    {
        None
    } else {
        Some([x as i32, y as i32])
    }
}
pub fn project_observation(
    row: &WindowsCursorRowV2,
    index: &CursorObservationIndexV2,
) -> CoreResult<Option<[f64; 2]>> {
    index.validate()?;
    project_with_validated_index(row, index)
}
// Internal callers validate the immutable index once; pose_basis still checks each row.
fn project_with_validated_index(
    row: &WindowsCursorRowV2,
    index: &CursorObservationIndexV2,
) -> CoreResult<Option<[f64; 2]>> {
    let Some((w, p)) = pose_basis(row, index)? else {
        return Ok(None);
    };
    Ok(source_point(p.physical_position, w).map(|[x, y]| {
        [
            f64::from(x) / f64::from(index.width),
            f64::from(y) / f64::from(index.height),
        ]
    }))
}

/// Admission only: retain rows with a positive causal interval after stream and terminal cuts.
/// Task 2 must use this subset before preparing drawable positions or image/follow runs.
pub(crate) fn eligible_pose_segments(
    poses: &[WindowsCursorRowV2],
    index: &CursorObservationIndexV2,
) -> CoreResult<Vec<Option<u32>>> {
    index.validate()?;
    require(
        poses.len() <= cursor::MAX_SAMPLES,
        "poses",
        "Pose record limit exceeded",
    )?;
    let terminal_end = match &index.terminal.pose_barrier {
        Some(b) => bracket(&b.q0, &b.q1)?[0].min(index.terminal.source_cutoff_qpc),
        None => index.terminal.source_cutoff_qpc,
    };
    let mut segments = Vec::with_capacity(poses.len());
    // An invalid row wins its whole lower-endpoint tick, including later equal-tick rows.
    let mut veto_ticks = Vec::new();
    let mut veto_ends = Vec::new();
    for row in poses {
        if pose_basis(row, index)?.is_none() {
            veto_ticks.push(row.bracket()?[0]);
            veto_ends.push(row.bracket()?[1]);
        }
    }
    veto_ticks.sort_unstable();
    veto_ticks.dedup();
    veto_ends.sort_unstable();
    veto_ends.dedup();
    let mut segment = 0u32;
    let mut previous_basis = None;
    let mut previous_bracket = None;
    for row in poses {
        validate_observation_row(row, index)?;
        let b = row.bracket()?;
        require(
            previous_bracket.is_none_or(|p: [i64; 2]| p[0] <= b[0] && p[1] <= b[1]),
            "poses.qpc",
            "Pose stream regresses",
        )?;
        previous_bracket = Some(b);
        let basis = if veto_ticks.binary_search(&b[0]).is_ok()
            || veto_ends.binary_search(&b[1]).is_ok()
            || b[1] >= terminal_end
        {
            None
        } else {
            pose_basis(row, index)?
        };
        if let Some((w, _)) = basis {
            let identity = (
                w.clone(),
                match row {
                    WindowsCursorRowV2::Observed {
                        barrier_revision, ..
                    } => *barrier_revision,
                    _ => 0,
                },
            );
            if previous_basis.as_ref() != Some(&identity) {
                segment += 1;
            }
            previous_basis = Some(identity);
            segments.push(Some(segment));
        } else {
            segment += 1;
            previous_basis = None;
            segments.push(None);
        }
    }
    let mut next_end = terminal_end;
    for i in (0..poses.len()).rev() {
        let b = poses[i].bracket()?;
        let boundary = if segments[i].is_none() || (i > 0 && segments[i - 1] != segments[i]) {
            b[0]
        } else {
            b[1]
        };
        if segments[i].is_some() && b[1] >= next_end {
            segments[i] = None;
        }
        next_end = next_end.min(boundary);
    }
    Ok(segments)
}

fn retained_row_cost<T>(line_cap: usize) -> usize {
    std::mem::size_of::<T>() + 2 * line_cap
}
fn terminal_retained_costs() -> [usize; 2] {
    [
        retained_row_cost::<WindowsCursorRowV2>(MAX_POSE_RECORD_BYTES),
        retained_row_cost::<WindowsInputRowV1>(MAX_INPUT_RECORD_BYTES),
    ]
}

/// Shared telemetry admission; two stream barriers and final index publication are reserved.
#[derive(Debug, Clone)]
pub struct ObservationBudget {
    pub max_bytes: usize,
    pub max_records: usize,
    pub max_retained_bytes: usize,
    bytes: usize,
    records: usize,
    retained: usize,
    closed: bool,
    terminals: usize,
}
impl Default for ObservationBudget {
    fn default() -> Self {
        Self::with_limits(
            cursor::MAX_SAMPLES_BYTES as usize,
            cursor::MAX_SAMPLES,
            MAX_RETAINED_BYTES,
        )
    }
}
impl ObservationBudget {
    pub fn with_limits(max_bytes: usize, max_records: usize, max_retained_bytes: usize) -> Self {
        Self {
            max_bytes,
            max_records,
            max_retained_bytes,
            bytes: 0,
            records: 0,
            retained: 0,
            closed: false,
            terminals: 0,
        }
    }
    pub fn admit(&mut self, bytes: usize, retained: usize) -> bool {
        if self.closed
            || self.records >= self.max_records.saturating_sub(2)
            || bytes
                > self
                    .max_bytes
                    .saturating_sub(MAX_POSE_RECORD_BYTES + MAX_INPUT_RECORD_BYTES)
                    .saturating_sub(self.bytes)
            || retained
                > self
                    .max_retained_bytes
                    .saturating_sub(cursor::MAX_INDEX_BYTES as usize + MAX_INTERACTION_BYTES + 8192)
                    .saturating_sub(terminal_retained_costs().into_iter().sum::<usize>())
                    .saturating_sub(self.retained)
        {
            self.closed = true;
            return false;
        }
        self.records += 1;
        self.bytes += bytes;
        self.retained += retained;
        true
    }
    /// Terminal rows bypass the closed data lane, still within the original combined limits.
    fn consume(&mut self, bytes: usize, retained: usize) -> bool {
        if self.records >= self.max_records
            || bytes > self.max_bytes.saturating_sub(self.bytes)
            || retained
                > self
                    .max_retained_bytes
                    .saturating_sub(cursor::MAX_INDEX_BYTES as usize + MAX_INTERACTION_BYTES + 8192)
                    .saturating_sub(self.retained)
        {
            return false;
        }
        self.records += 1;
        self.bytes += bytes;
        self.retained += retained;
        true
    }
    pub fn admit_terminal(&mut self, bytes: usize) -> bool {
        // Reader order is pose then input; the first charge is conservative if only input remains.
        let Some(retained) = terminal_retained_costs().get(self.terminals).copied() else {
            return false;
        };
        if !self.consume(bytes, retained) {
            return false;
        }
        self.terminals += 1;
        true
    }
}
#[derive(Debug)]
pub struct ObservationData {
    pub index: CursorObservationIndexV2,
    pub poses: Vec<WindowsCursorRowV2>,
    pub inputs: Vec<WindowsInputRowV1>,
}
fn read_rows<T: DeserializeOwned>(
    path: &Path,
    line_cap: usize,
    budget: &mut ObservationBudget,
    check: impl Fn(&T) -> CoreResult<[i64; 2]>,
) -> CoreResult<Vec<T>> {
    cursor::read_stable(path, cursor::MAX_SAMPLES_BYTES, |file| {
        let mut reader = BufReader::new(file);
        let mut line = Vec::with_capacity(line_cap + 1);
        let mut rows = Vec::new();
        let mut previous = None;
        loop {
            line.clear();
            let n = (&mut reader)
                .take((line_cap + 1) as u64)
                .read_until(b'\n', &mut line)
                .map_err(|e| invalid("observations", e.to_string()))?;
            if n == 0 {
                break;
            }
            require(
                n <= line_cap,
                "observations",
                "Observation line exceeds its bounded record size",
            )?;
            // Account conservatively for all string heaps and Vec growth before retaining a row.
            let allocation = retained_row_cost::<T>(line_cap);
            require(
                budget.consume(n, allocation),
                "observations",
                "Combined stream byte, record or retained-memory budget exceeded",
            )?;
            let row: T = decode_strict(&line)?;
            let b = check(&row)?;
            require(
                previous.is_none_or(|p: [i64; 2]| p[0] <= b[0] && p[1] <= b[1]),
                "observations.qpc",
                "Stream brackets must be independently nondecreasing",
            )?;
            previous = Some(b);
            rows.reserve_exact(1);
            rows.push(row);
        }
        Ok(rows)
    })
}
pub fn read_observations(root: &Path) -> CoreResult<ObservationData> {
    let index_path = crate::capture::asset(root, "cursor/index.json")?
        .ok_or_else(|| invalid("cursor.index", "Missing observation index"))?;
    let index = cursor::read_stable(&index_path, cursor::MAX_INDEX_BYTES, |file| {
        let mut bytes = Vec::new();
        file.take(cursor::MAX_INDEX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| invalid("cursor.index", e.to_string()))?;
        decode_observation_index(&bytes)
    })?;
    let pose_path = crate::capture::asset(root, &index.observations_path)?
        .ok_or_else(|| invalid("observations", "Missing pose observations"))?;
    let input_path = crate::capture::asset(root, &index.input_observations_path)?
        .ok_or_else(|| invalid("observations", "Missing input observations"))?;
    let mut budget = ObservationBudget::default();
    let poses = read_rows(&pose_path, MAX_POSE_RECORD_BYTES, &mut budget, |row| {
        validate_observation_row(row, &index)?;
        row.bracket()
    })?;
    let inputs = read_rows(
        &input_path,
        MAX_INPUT_RECORD_BYTES,
        &mut budget,
        validate_input,
    )?;
    Ok(ObservationData {
        index,
        poses,
        inputs,
    })
}
fn validate_input(row: &WindowsInputRowV1) -> CoreResult<[i64; 2]> {
    let b = row.bracket()?;
    match row {
        WindowsInputRowV1::Barrier { reason: s, .. } => reason(s)?,
        WindowsInputRowV1::Observed {
            hit_hwnd,
            root_hwnd,
            ..
        } => {
            handle(hit_hwnd, true)?;
            handle(root_hwnd, true)?;
        }
    }
    Ok(b)
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampledClickEvidence {
    pub button: ClickButton,
    pub previous_input_row: u32,
    pub observed_input_row: u32,
    pub basis_pose_row: u32,
    pub occurrence_qpc: [i64; 2],
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampledDragEvidence {
    pub button: ClickButton,
    pub start_edge: SampledClickEvidence,
    pub input_point_rows: Vec<u32>,
    pub end_kind: DragEndKind,
    pub end_reason: DragEndReason,
    pub end_bracket: Option<[i64; 2]>,
    pub release_bracket: Option<[i64; 2]>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampledInteractions {
    pub click_evidence: Vec<SampledClickEvidence>,
    pub drag_evidence: Vec<SampledDragEvidence>,
    pub compatibility: InteractionEvents,
    pub timeline: Vec<TimelineSegment>,
    pub project: legacy::LegacyInteractions,
    pub truncated: bool,
}
#[derive(Debug, Clone, Copy)]
pub struct InteractionLimits {
    pub clicks: usize,
    pub drags: usize,
    pub points: usize,
    pub bytes: usize,
}
impl Default for InteractionLimits {
    fn default() -> Self {
        Self {
            clicks: 4096,
            drags: 4096,
            points: 8192,
            bytes: MAX_INTERACTION_BYTES,
        }
    }
}
#[derive(Clone)]
struct InputPoint {
    row: u32,
    basis: u32,
    b: [i64; 2],
    p: [i32; 2],
    buttons: [Option<bool>; 2],
    swapped: bool,
    segment: u32,
}
struct Pending {
    edge: SampledClickEvidence,
    points: Vec<InputPoint>,
    last: InputPoint,
    active: bool,
}
fn distance(a: &InputPoint, b: &InputPoint) -> i128 {
    let x = i128::from(a.p[0]) - i128::from(b.p[0]);
    let y = i128::from(a.p[1]) - i128::from(b.p[1]);
    x * x + y * y
}
fn point(p: &InputPoint, index: &CursorObservationIndexV2) -> CoreResult<DragPoint> {
    Ok(DragPoint {
        timestamp: index.ceil_us(p.b[1])? as f64 / 1_000_000.0,
        x: p.p[0],
        y: p.p[1],
    })
}
fn finish_pending(
    mut pending: Pending,
    kind: DragEndKind,
    why: DragEndReason,
    end: Option<[i64; 2]>,
    release: Option<[i64; 2]>,
    out: &mut SampledInteractions,
    index: &CursorObservationIndexV2,
    retained: &mut usize,
) -> CoreResult<()> {
    if pending.last.b[1] > pending.points.last().unwrap().b[1] {
        if pending.last.p == pending.points.last().unwrap().p {
            *pending.points.last_mut().unwrap() = pending.last.clone();
        } else {
            pending.points.push(pending.last.clone());
            *retained += 1;
        }
    }
    if !pending.active
        || pending.points.len() < 2
        || pending.points[0].b[1] >= pending.points.last().unwrap().b[1]
    {
        return Ok(());
    }
    let points = pending
        .points
        .iter()
        .map(|p| point(p, index))
        .collect::<CoreResult<Vec<_>>>()?;
    if points[0].timestamp >= points.last().unwrap().timestamp {
        return Ok(());
    }
    out.compatibility.drags.push(DragInteraction {
        start: points[0].timestamp,
        end: points.last().unwrap().timestamp,
        button: pending.edge.button,
        points,
    });
    out.drag_evidence.push(SampledDragEvidence {
        button: pending.edge.button,
        start_edge: pending.edge,
        input_point_rows: pending.points.iter().map(|p| p.row).collect(),
        end_kind: kind,
        end_reason: why,
        end_bracket: end,
        release_bracket: release,
    });
    Ok(())
}
fn terminal_bound(
    index: &CursorObservationIndexV2,
    terminal: &ObservationTerminal,
) -> CoreResult<i64> {
    let mut bound = terminal.source_cutoff_qpc;
    for b in [&terminal.pose_barrier, &terminal.input_barrier]
        .into_iter()
        .flatten()
    {
        bound = bound.min(bracket(&b.q0, &b.q1)?[0]);
    }
    index.validate()?;
    Ok(bound)
}
fn input_point(
    row: &WindowsInputRowV1,
    i: usize,
    poses: &[WindowsCursorRowV2],
    index: &CursorObservationIndexV2,
    segments: &[Option<u32>],
    latest: Option<usize>,
    end: i64,
) -> CoreResult<Option<InputPoint>> {
    let b = validate_input(row)?;
    let WindowsInputRowV1::Observed {
        physical_position,
        hit_hwnd,
        root_hwnd,
        hit_pid,
        hit_tid,
        buttons_swapped,
        left,
        right,
        basis_pose_row: Some(basis),
        barrier_revision_before,
        barrier_revision_after,
        ..
    } = row
    else {
        return Ok(None);
    };
    require(
        (*basis as usize) < poses.len(),
        "basisPoseRow",
        "Dangling pose basis reference",
    )?;
    let n = *basis as usize;
    let Some(segment) = segments[n] else {
        return Ok(None);
    };
    let Some((w, _)) = pose_basis(&poses[n], index)? else {
        return Ok(None);
    };
    let pb = poses[n].bracket()?;
    let revision = match &poses[n] {
        WindowsCursorRowV2::Observed {
            barrier_revision, ..
        } => *barrier_revision,
        _ => return Ok(None),
    };
    if !index.eligible(b)?
        || b[1] >= end
        || pb[1] > b[0]
        || 30 * i128::from(b[1] - pb[1]) > index.frequency()?
        || barrier_revision_before != barrier_revision_after
        || *barrier_revision_before != revision
        || latest.is_none_or(|p| segments[p] != Some(segment))
        || hit_hwnd == "0"
        || root_hwnd != &w.hwnd
        || *hit_pid != w.pid
        || *hit_tid != w.tid
        || left.is_none()
        || right.is_none()
    {
        return Ok(None);
    }
    let Some(p) = source_point(*physical_position, w) else {
        return Ok(None);
    };
    Ok(Some(InputPoint {
        row: i as u32,
        basis: *basis,
        b,
        p,
        buttons: if *buttons_swapped {
            [*right, *left]
        } else {
            [*left, *right]
        },
        swapped: *buttons_swapped,
        segment,
    }))
}
pub fn sampled_interactions(
    poses: &[WindowsCursorRowV2],
    inputs: &[WindowsInputRowV1],
    terminal: &ObservationTerminal,
    index: &CursorObservationIndexV2,
    defaults: LegacyDefaults,
) -> CoreResult<SampledInteractions> {
    sampled_interactions_with_limits(
        poses,
        inputs,
        terminal,
        index,
        defaults,
        InteractionLimits::default(),
    )
}
pub fn sampled_interactions_with_limits(
    poses: &[WindowsCursorRowV2],
    inputs: &[WindowsInputRowV1],
    terminal: &ObservationTerminal,
    index: &CursorObservationIndexV2,
    defaults: LegacyDefaults,
    limits: InteractionLimits,
) -> CoreResult<SampledInteractions> {
    index.validate()?;
    terminal_valid(terminal, index)?;
    require(
        terminal == &index.terminal,
        "terminal",
        "Conversion terminal must equal index authority",
    )?;
    legacy::validate_defaults(defaults)?;
    require(
        limits.clicks <= 4096
            && limits.drags <= 4096
            && limits.points <= 8192
            && limits.bytes <= MAX_INTERACTION_BYTES,
        "limits",
        "Interaction limits cannot exceed format policy",
    )?;
    require(
        poses
            .len()
            .checked_add(inputs.len())
            .is_some_and(|n| n <= cursor::MAX_SAMPLES),
        "observations",
        "Combined record limit exceeded",
    )?;
    let mut out = SampledInteractions {
        click_evidence: vec![],
        drag_evidence: vec![],
        compatibility: InteractionEvents {
            clicks: vec![],
            drags: vec![],
        },
        timeline: vec![],
        project: legacy::LegacyInteractions {
            zooms: vec![],
            clicks: vec![],
            warnings: vec![],
        },
        truncated: false,
    };
    let segments = eligible_pose_segments(poses, index)?;
    let mut previous: Option<InputPoint> = None;
    let mut pending: [Option<Pending>; 2] = [None, None];
    let mut retained = 0usize;
    let end = terminal_bound(index, terminal)?;
    let mut next_pose = 0;
    let mut latest = None;
    let mut last_input = None;
    let mut points = Vec::with_capacity(inputs.len());
    let mut input_veto_ticks = Vec::new();
    for (i, row) in inputs.iter().enumerate() {
        let b = validate_input(row)?;
        require(
            last_input.is_none_or(|p: [i64; 2]| p[0] <= b[0] && p[1] <= b[1]),
            "inputs.qpc",
            "Input stream regresses",
        )?;
        last_input = Some(b);
        while next_pose < poses.len() && poses[next_pose].bracket()?[0] <= b[1] {
            latest = Some(next_pose);
            next_pose += 1;
        }
        let point = input_point(row, i, poses, index, &segments, latest, end)?;
        if point.is_none() {
            input_veto_ticks.push(b[0]);
        }
        points.push((point, latest));
    }
    input_veto_ticks.sort_unstable();
    input_veto_ticks.dedup();
    for (row, (point, latest)) in inputs.iter().zip(points) {
        let b = row.bracket()?;
        let p = if input_veto_ticks.binary_search(&b[0]).is_ok() {
            None
        } else {
            point
        };
        let continuous = match (&previous, &p) {
            (Some(a), Some(b)) => {
                a.segment == b.segment
                    && a.swapped == b.swapped
                    && i128::from(b.b[1] - a.b[0]) * 1_000_000 <= index.frequency()? * 250_000
            }
            _ => false,
        };
        if p.is_none() || (!continuous && previous.is_some()) {
            let outside_saved = index.relative_numerator(b[1])? * 30
                >= i128::from(index.clock.completed_frames) * 10_000_000 * index.frequency()?;
            let (cause, cause_bracket) = if outside_saved {
                (DragEndReason::SourceCutoff, None)
            } else if b[1] >= end {
                let cause = match terminal.stop_kind {
                    StopKind::SourceEnd => DragEndReason::SourceCutoff,
                    StopKind::Stop => DragEndReason::Stop,
                    StopKind::Budget => DragEndReason::Budget,
                    StopKind::SamplerFailure => DragEndReason::Barrier,
                };
                let measured = [&terminal.pose_barrier, &terminal.input_barrier]
                    .into_iter()
                    .flatten()
                    .map(|b| bracket(&b.q0, &b.q1))
                    .collect::<CoreResult<Vec<_>>>()?
                    .into_iter()
                    .min_by_key(|b| b[0]);
                (
                    if measured.is_some() {
                        DragEndReason::Barrier
                    } else {
                        cause
                    },
                    measured,
                )
            } else if let Some(latest) = latest.filter(|n| {
                previous
                    .as_ref()
                    .is_some_and(|p| segments[*n] != Some(p.segment))
            }) {
                (DragEndReason::Barrier, Some(poses[latest].bracket()?))
            } else {
                (DragEndReason::Barrier, Some(b))
            };
            for p in pending.iter_mut().filter_map(Option::take) {
                finish_pending(
                    p,
                    DragEndKind::Interrupted,
                    cause,
                    cause_bracket,
                    None,
                    &mut out,
                    index,
                    &mut retained,
                )?;
            }
            previous = None;
        }
        let Some(p) = p else { continue };
        // Ordinary points leave two final-point slots and two drag records for active buttons.
        let active = pending.iter().flatten().count();
        if out.drag_evidence.len() + active > limits.drags
            || 8192
                + out.click_evidence.len() * 2048
                + (out.drag_evidence.len() + active) * 1024
                + (retained + 2) * 2048
                > limits.bytes
        {
            out.truncated = true;
            break;
        }
        for (button_index, drag) in pending.iter_mut().enumerate() {
            if p.buttons[button_index] == Some(true) {
                if let Some(drag) = drag {
                    drag.last = p.clone();
                }
            }
        }
        for button_index in 0..2 {
            let button = if button_index == 0 {
                ClickButton::Left
            } else {
                ClickButton::Right
            };
            let state = p.buttons[button_index].unwrap();
            if let Some(prev) = &previous {
                let was = prev.buttons[button_index].unwrap();
                if !was && state {
                    if out.click_evidence.len() >= limits.clicks
                        || out.drag_evidence.len() + pending.iter().flatten().count()
                            >= limits.drags
                        || retained >= limits.points.saturating_sub(2)
                    {
                        out.truncated = true;
                        break;
                    }
                    let at = index.ceil_us(p.b[1])?;
                    if at < index.duration_us {
                        let edge = SampledClickEvidence {
                            button,
                            previous_input_row: prev.row,
                            observed_input_row: p.row,
                            basis_pose_row: p.basis,
                            occurrence_qpc: [prev.b[0], p.b[1]],
                        };
                        out.compatibility.clicks.push(ClickEvent {
                            timestamp: at as f64 / 1_000_000.0,
                            x: p.p[0],
                            y: p.p[1],
                            button,
                        });
                        out.click_evidence.push(edge.clone());
                        pending[button_index] = Some(Pending {
                            edge,
                            points: vec![p.clone()],
                            last: p.clone(),
                            active: false,
                        });
                        retained += 1;
                    }
                } else if was && !state {
                    if let Some(mut drag) = pending[button_index].take() {
                        let release = [prev.b[0], p.b[1]];
                        if drag.points.last().unwrap().p == p.p {
                            *drag.points.last_mut().unwrap() = p.clone();
                        } else {
                            drag.points.push(p.clone());
                            retained += 1;
                        }
                        drag.last = p.clone();
                        drag.active |= distance(&drag.points[0], &p) >= 16;
                        finish_pending(
                            drag,
                            DragEndKind::ObservedRelease,
                            DragEndReason::Release,
                            Some(p.b),
                            Some(release),
                            &mut out,
                            index,
                            &mut retained,
                        )?;
                    }
                }
            }
            if state {
                if let Some(drag) = &mut pending[button_index] {
                    drag.last = p.clone();
                    drag.active |= distance(&drag.points[0], &p) >= 16;
                    let last = drag.points.last().unwrap();
                    let d = distance(last, &p);
                    if drag.active
                        && d >= 4
                        && (i128::from(p.b[1] - last.b[1]) * 1_000_000
                            >= index.frequency()? * 50_000
                            || d >= 144)
                    {
                        if retained >= limits.points.saturating_sub(2) {
                            out.truncated = true;
                            break;
                        }
                        drag.points.push(p.clone());
                        retained += 1;
                    }
                }
            }
        }
        previous = Some(p);
        if out.truncated {
            break;
        }
    }
    let why = if out.truncated || terminal.stop_kind == StopKind::Budget {
        DragEndReason::Budget
    } else if terminal.pose_barrier.is_some()
        || terminal.input_barrier.is_some()
        || terminal.stop_kind == StopKind::SamplerFailure
    {
        DragEndReason::Barrier
    } else if terminal.stop_kind == StopKind::SourceEnd {
        DragEndReason::SourceCutoff
    } else {
        DragEndReason::Stop
    };
    let end_bracket = if let Some(b) = terminal
        .pose_barrier
        .as_ref()
        .or(terminal.input_barrier.as_ref())
    {
        Some(bracket(&b.q0, &b.q1)?)
    } else {
        None
    };
    for mut drag in pending.into_iter().flatten() {
        if drag.last.b[1] > drag.points.last().unwrap().b[1] {
            if drag.last.p == drag.points.last().unwrap().p {
                *drag.points.last_mut().unwrap() = drag.last.clone();
            } else if retained < limits.points {
                drag.points.push(drag.last.clone());
                retained += 1;
            }
        }
        finish_pending(
            drag,
            DragEndKind::Interrupted,
            why,
            end_bracket,
            None,
            &mut out,
            index,
            &mut retained,
        )?;
    }
    out.truncated |= terminal.stop_kind == StopKind::Budget;
    out.timeline = timeline::build_timeline_segments(
        out.compatibility.clicks.clone(),
        out.compatibility.drags.clone(),
    );
    out.project = legacy::convert_interactions(
        crate::ffmpeg::ProbedVideo {
            width: index.width,
            height: index.height,
            duration_us: index.duration_us,
        },
        defaults,
        serde_json::to_value(&out.timeline).map_err(|e| invalid("timeline", e.to_string()))?,
        serde_json::to_value(&out.compatibility.clicks)
            .map_err(|e| invalid("clicks", e.to_string()))?,
        Some(
            serde_json::to_value(&out.compatibility.drags)
                .map_err(|e| invalid("drags", e.to_string()))?,
        ),
    )?;
    require(
        serde_json::to_vec(&out)
            .map_err(|e| invalid("interactions", e.to_string()))?
            .len()
            <= limits.bytes,
        "interactions",
        "Derived interaction serialization exceeds policy",
    )?;
    Ok(out)
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn stream_budget_is_shared_and_checked_before_retaining_rows() {
        let root = std::env::temp_dir().join(format!(
            "focusflow-observation-budget-{}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("rows.ndjson");
        let row = b"{\"type\":\"barrier\",\"q0\":\"0\",\"q1\":\"0\",\"reason\":\"query failed\"}\n";
        std::fs::write(&path, [row.as_slice(), row.as_slice()].concat()).unwrap();
        let mut records = ObservationBudget::with_limits(1024, 3, 16 * 1024 * 1024);
        let first = read_rows::<WindowsInputRowV1>(
            &path,
            MAX_INPUT_RECORD_BYTES,
            &mut records,
            validate_input,
        )
        .unwrap();
        assert_eq!(first.len(), 2);
        assert!(read_rows::<WindowsInputRowV1>(
            &path,
            MAX_INPUT_RECORD_BYTES,
            &mut records,
            validate_input
        )
        .is_err());
        let mut bytes = ObservationBudget::with_limits(row.len() * 3, 100, 16 * 1024 * 1024);
        read_rows::<WindowsInputRowV1>(&path, MAX_INPUT_RECORD_BYTES, &mut bytes, validate_input)
            .unwrap();
        assert!(read_rows::<WindowsInputRowV1>(
            &path,
            MAX_INPUT_RECORD_BYTES,
            &mut bytes,
            validate_input
        )
        .is_err());
        let mut memory = ObservationBudget::with_limits(1024, 100, MAX_INTERACTION_BYTES);
        assert!(read_rows::<WindowsInputRowV1>(
            &path,
            MAX_INPUT_RECORD_BYTES,
            &mut memory,
            validate_input
        )
        .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Compact immutable evidence: original row order/brackets and stream segment survive preparation.
#[derive(Debug, Clone, Copy)]
struct PreparedObservation {
    bracket: [i64; 2],
    until: i128,
    point: [f64; 2],
    endpoint: [f64; 2],
    segment: u32,
    shape: u16,
}
#[derive(Debug, Clone)]
pub(crate) struct PreparedObservations {
    rows: Vec<PreparedObservation>,
    visible: Vec<u32>,
    denominator: i128,
    epoch: i128,
    duration_us: u64,
    tau_us: u64,
}
impl PreparedObservations {
    pub(crate) fn prepare(
        data: &ObservationData,
        tau_us: u64,
        cancel: &impl Fn() -> CoreResult<bool>,
    ) -> CoreResult<Self> {
        Self::with_limit(data, tau_us, MAX_RETAINED_BYTES, cancel)
    }
    fn with_limit(
        data: &ObservationData,
        tau_us: u64,
        limit: usize,
        cancel: &impl Fn() -> CoreResult<bool>,
    ) -> CoreResult<Self> {
        let index = &data.index;
        let segments = eligible_pose_segments(&data.poses, index)?;
        let mut result = Self {
            rows: Vec::new(),
            visible: Vec::new(),
            denominator: 10 * index.frequency()?,
            epoch: index.frequency()? * i128::from(qpc(&index.clock.epoch_100ns)?),
            duration_us: index.duration_us,
            tau_us,
        };
        let count = segments.iter().filter(|s| s.is_some()).count();
        let estimate = data
            .poses
            .len()
            .checked_mul(std::mem::size_of::<PreparedObservation>())
            .and_then(|n| n.checked_add(count * std::mem::size_of::<u32>()))
            .ok_or_else(|| invalid("cursor.observations", "Prepared observation size overflow"))?;
        require(
            estimate <= limit,
            "cursor.observations",
            "Prepared observations exceed retained-memory limit",
        )?;
        result.rows.reserve_exact(data.poses.len());
        result.visible.reserve_exact(count);
        result.check_limit(limit)?;
        let mut cut = index.relative_numerator(index.terminal.source_cutoff_qpc)?;
        if let Some(b) = &index.terminal.pose_barrier {
            cut = cut.min(index.relative_numerator(bracket(&b.q0, &b.q1)?[0])?);
        }
        for (i, row) in data.poses.iter().enumerate() {
            if i % 4096 == 0 && cancel()? {
                return Err(crate::ffmpeg::cancel_error());
            }
            let b = row.bracket()?;
            let point = if segments[i].is_some() {
                project_with_validated_index(row, index)?.ok_or_else(|| {
                    invalid("cursor.observations", "Eligible pose lost its source basis")
                })?
            } else {
                [0.0, 0.0]
            };
            let shape = match row {
                WindowsCursorRowV2::Observed {
                    shape_id: Some(id), ..
                } => index.shapes.iter().position(|s| s.id == *id).unwrap() as u16,
                _ => u16::MAX,
            };
            result.rows.push(PreparedObservation {
                bracket: b,
                until: cut.min(
                    index.relative_numerator(b[0])?
                        + i128::from(index.max_hold_us) * result.denominator,
                ),
                point,
                endpoint: point,
                segment: segments[i].unwrap_or(0),
                shape,
            });
        }
        for i in (0..result.rows.len()).rev() {
            let row = &mut result.rows[i];
            row.until = row.until.min(cut);
            if segments[i].is_none() || (i > 0 && segments[i - 1] != segments[i]) {
                cut = cut.min(index.relative_numerator(row.bracket[0])?);
            }
        }
        for i in 0..result.rows.len() {
            if i % 4096 == 0 && cancel()? {
                return Err(crate::ffmpeg::cancel_error());
            }
            let row = result.rows[i];
            if row.segment == 0 {
                continue;
            }
            let start = result.relative(row.bracket[1]);
            if i > 0
                && result.rows[i - 1].segment == row.segment
                && start < result.rows[i - 1].until
            {
                result.rows[i].endpoint = result.position(i - 1, start);
            }
            result.visible.push(i as u32);
        }
        result.check_limit(limit)?;
        Ok(result)
    }
    fn relative(&self, q: i64) -> i128 {
        i128::from(q) * 10_000_000 - self.epoch
    }
    fn position(&self, i: usize, time: i128) -> [f64; 2] {
        let row = self.rows[i];
        if self.tau_us == 0 {
            return row.point;
        }
        let dt = (time - self.relative(row.bracket[1])) as f64 / self.denominator as f64;
        let decay = (-dt / self.tau_us as f64).exp();
        [
            row.point[0] + (row.endpoint[0] - row.point[0]) * decay,
            row.point[1] + (row.endpoint[1] - row.point[1]) * decay,
        ]
    }
    fn last(&self, source_numerator: u128, rate: u32) -> Option<(usize, i128, i128)> {
        if rate == 0 {
            return None;
        }
        let time = i128::try_from(source_numerator)
            .ok()?
            .checked_mul(self.denominator)?;
        let rate = i128::from(rate);
        let j = self
            .visible
            .partition_point(|i| self.relative(self.rows[*i as usize].bracket[1]) * rate <= time)
            .checked_sub(1)?;
        Some((self.visible[j] as usize, time, rate))
    }
    pub(crate) fn sample_at(&self, source_numerator: u128, rate: u32) -> Option<([f64; 2], u16)> {
        if rate == 0 || source_numerator >= u128::from(self.duration_us) * u128::from(rate) {
            return None;
        }
        let (i, time, rate) = self.last(source_numerator, rate)?;
        let row = self.rows[i];
        if time >= row.until * rate {
            return None;
        }
        // Eligibility is exact; floating point enters only the aesthetic smoothing function.
        let dt =
            (time - self.relative(row.bracket[1]) * rate) as f64 / (self.denominator * rate) as f64;
        if self.tau_us > 0 && dt == 0.0 {
            return Some((row.endpoint, row.shape));
        }
        let decay = if self.tau_us == 0 {
            0.0
        } else {
            (-dt / self.tau_us as f64).exp()
        };
        Some((
            [
                row.point[0] + (row.endpoint[0] - row.point[0]) * decay,
                row.point[1] + (row.endpoint[1] - row.point[1]) * decay,
            ],
            row.shape,
        ))
    }
    pub(crate) fn position_at(&self, source_numerator: u128, rate: u32) -> Option<[f64; 2]> {
        self.sample_at(source_numerator, rate).map(|(p, _)| p)
    }
    pub(crate) fn follow_focus_at(
        &self,
        source_numerator: u128,
        rate: u32,
        entry_us: u64,
        fallback: [f64; 2],
    ) -> [f64; 2] {
        let clamp = |p: [f64; 2]| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)];
        if let Some(p) = self.position_at(source_numerator, rate) {
            return clamp(p);
        }
        if entry_us >= self.duration_us {
            return fallback;
        }
        if let Some((i, time, rate)) = self.last(source_numerator, rate) {
            let row = self.rows[i];
            if row.until > i128::from(entry_us) * self.denominator && row.until * rate <= time {
                return clamp(self.position(i, row.until));
            }
        }
        fallback
    }
    fn check_limit(&self, limit: usize) -> CoreResult<()> {
        require(
            self.buffer_bytes() <= limit,
            "cursor.observations",
            "Prepared observation capacities exceed retained-memory limit",
        )
    }
    pub(crate) fn buffer_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.rows.capacity() * std::mem::size_of::<PreparedObservation>()
            + self.visible.capacity() * std::mem::size_of::<u32>()
    }
    pub(crate) fn sample_count(&self) -> usize {
        self.rows.len()
    }
    pub(crate) fn has_gaps(&self) -> bool {
        let Some(first) = self.visible.first() else {
            return true;
        };
        self.relative(self.rows[*first as usize].bracket[1]) > 0
            || self.visible.windows(2).any(|pair| {
                self.relative(self.rows[pair[1] as usize].bracket[1])
                    > self.rows[pair[0] as usize].until
            })
            || self.rows[*self.visible.last().unwrap() as usize].until
                < i128::from(self.duration_us) * self.denominator
    }
    pub(crate) fn native_complete(&self) -> bool {
        self.visible
            .iter()
            .all(|i| self.rows[*i as usize].shape != u16::MAX)
    }
    pub(crate) fn uses_shape(&self, shape: usize) -> bool {
        self.visible
            .iter()
            .any(|i| self.rows[*i as usize].shape as usize == shape)
    }
}

#[cfg(test)]
mod prepared_tests {
    use super::*;
    std::thread_local! {
        pub(super) static INDEX_VALIDATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    fn large_shape_table(rows: usize) -> ObservationData {
        use serde_json::json;
        let shapes: Vec<_> = (0..256).map(|s| json!({"id":format!("s{s}"),"path":format!("cursor/shapes/s{s}.pam"),"width":1,"height":1,"hotspotX":0,"hotspotY":0})).collect();
        let index = serde_json::from_value(json!({
            "schemaVersion":2,"width":322,"height":272,"durationUs":34000000,
            "observationsPath":"cursor/observations.ndjson","inputObservationsPath":"cursor/input-observations.ndjson","shapes":shapes,
            "clock":{"provenance":"ownedWgcCausalCfrV1","qpcFrequency":"1000000","admissionQpc":"0","epoch100ns":"0","cutoffQpc":"34000000","cutoffReason":"stop","completedFrames":1020,"workerExitCode":0,"encoderExitCode":0,"interactions":"unmeasured","audio":"unmeasured"},
            "model":"dwmExtendedFrameTranslationV1","profile":"pmv2_96_single_monitor_decorated","observation":"windowsCurrentQueryBracket","reconstruction":"causalAfterBracket","maxHoldUs":250000,
            "spatialErrorPx":null,"frameAlignmentErrorUs":null,"animationPhase":"unknown","buttonCoverage":"sampledEdges","dragCoverage":"sampledPath",
            "terminal":{"poseBarrier":null,"inputBarrier":null,"sourceCutoffQpc":34000000,"stopKind":"stop"}
        })).unwrap();
        let window = json!({"hwnd":"123","pid":10,"tid":11,"clientOrigin":[47,40],"clientSize":[322,272],"windowRect":[40,40,376,319],"dwmRect":[47,40,369,312],"dpi":96,"observerPmv2":true,"targetPmv2":true,"style":0x10cf0000,"exStyle":0,"visible":true,"minimized":false,"monitorRect":[0,0,1920,1080],"monitorCount":1});
        let pointer = json!({"physicalPosition":[56,79],"cursorPosition":[56,79],"flags":1,"handle":"77","hitHwnd":"123","rootHwnd":"123","hitPid":10,"hitTid":11});
        let poses=(0..rows).map(|n| serde_json::from_value(json!({"type":"observed","q0":(n*4000).to_string(),"q1":(n*4000).to_string(),"before":window,"after":window,"pointerBefore":pointer,"pointerAfter":pointer,"shapeId":null,"barrierRevision":0})).unwrap()).collect();
        ObservationData {
            index,
            poses,
            inputs: vec![],
        }
    }
    #[test]
    fn preparation_validates_immutable_shape_table_once_and_public_projection_stays_strict() {
        let mut data = large_shape_table(2000);
        INDEX_VALIDATIONS.with(|count| count.set(0));
        let prepared = PreparedObservations::prepare(&data, 0, &|| Ok(false)).unwrap();
        assert_eq!(prepared.sample_count(), 2000);
        assert_eq!(
            prepared.position_at(0, 1),
            Some([9.0 / 322.0, 39.0 / 272.0])
        );
        assert_eq!(
            INDEX_VALIDATIONS.with(|count| count.get()),
            1,
            "one immutable shape-table validation per preparation"
        );
        data.index.shapes[255] = data.index.shapes[0].clone();
        assert!(
            project_observation(&data.poses[0], &data.index).is_err(),
            "public projection must reject an invalid index"
        );
    }
    #[test]
    fn preparation_cancels_before_next_record_without_revalidating_shape_table() {
        let data = large_shape_table(8193);
        INDEX_VALIDATIONS.with(|count| count.set(0));
        let callbacks = std::cell::Cell::new(0);
        let error = PreparedObservations::prepare(&data, 0, &|| {
            callbacks.set(callbacks.get() + 1);
            assert_eq!(
                INDEX_VALIDATIONS.with(|count| count.get()),
                1,
                "no per-row immutable validation between cancellation checkpoints"
            );
            Ok(callbacks.get() == 2)
        })
        .unwrap_err();
        assert_eq!(error.code, crate::ErrorCode::Cancelled);
        assert_eq!(callbacks.get(), 2);
    }
    #[test]
    fn prepared_small_cap_accounts_actual_capacities_and_container() {
        let p = PreparedObservations {
            rows: vec![PreparedObservation {
                bracket: [3000000, 3000006],
                until: 250000,
                point: [0.5, 0.5],
                endpoint: [0.5, 0.5],
                segment: 1,
                shape: 0,
            }],
            visible: vec![0],
            denominator: 30000000,
            epoch: 30000000000000,
            duration_us: 1000000,
            tau_us: 0,
        };
        let arrays = p.rows.capacity() * std::mem::size_of::<PreparedObservation>()
            + p.visible.capacity() * std::mem::size_of::<u32>();
        assert!(
            p.check_limit(arrays).is_err(),
            "container memory must be charged too"
        );
        let cap = arrays + std::mem::size_of::<PreparedObservations>();
        p.check_limit(cap).unwrap();
        assert_eq!(p.buffer_bytes(), cap);
    }
}
