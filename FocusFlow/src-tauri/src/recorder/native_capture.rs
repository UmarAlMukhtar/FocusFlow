//! Linux recording ownership and immutable initial snapshot assembly.
use super::{cursor, video_clock, CapturePlan, InteractionEvents, RecorderError, RecorderResult,
    RecordingSource, StartReservation, build_timeline_segments};
use focusflow_core::{capture::{self, *}, ffmpeg::ProbedVideoEvidence, legacy::{self, LegacyDefaults}, project::*};
use serde::Serialize;
use std::{io::Write, path::Path, time::Instant};

pub(super) const EMBEDDED_NOTICE: &str = "Separate cursor capture is unavailable with this FFmpeg. The cursor will be recorded in the video.";

pub(super) struct Session {
    sampler: Option<cursor::Sampler>,
    outcome: Option<cursor::Outcome>,
    source: cursor::Source,
    manifest: CaptureManifestV1,
    defaults: LegacyDefaults,
}
impl Session {
    pub(super) fn prepare(dir: &Path, plan: &CapturePlan, defaults: LegacyDefaults) -> RecorderResult<Self> {
        let source = cursor::Source { bounds:plan.bounds, window:plan.window_capture.as_ref().map(|w|w.hwnd as u32) };
        let kind = match plan.source {RecordingSource::Screen=>SourceKind::Screen,RecordingSource::Region{..}=>SourceKind::Region,RecordingSource::Window{..}=>SourceKind::Window};
        let id = super::session_id_from_dir(dir)?;
        let manifest = incomplete(&id, kind, source);
        manifest.validate(&id).map_err(recovery)?;
        let sampler = cursor::Sampler::prepare(dir, source)?;
        Ok(Self {sampler:Some(sampler),outcome:None,source,manifest,defaults})
    }

    pub(super) fn arm(&mut self, dir:&Path, epoch:Instant, clock:&video_clock::Clock,
        reservation:&mut StartReservation<'_>, args:&mut [String]) -> RecorderResult<()> {
        if !clock.enabled() {return Err(RecorderError::new("cursor_clock_unavailable", "The video clock could not start. Retry recording; no video was started.", true));}
        let mouse = args.iter().position(|arg|arg=="-draw_mouse").filter(|i|*i+1<args.len())
            .ok_or_else(||RecorderError::new("cursor_source_invalid", "The capture source has no cursor setting", true))?;
        self.sampler.as_mut().ok_or_else(||RecorderError::new("cursor_unavailable", "Cursor sampling has already finished", true))?.start(epoch)?;
        reservation.publish_incomplete(dir, &self.manifest)?;
        // Cursor-free capture is authorized only by the successfully synced incomplete marker.
        args[mouse+1] = "0".into();
        Ok(())
    }

    pub(super) fn note_source_end(&mut self, reason: &'static str) {
        self.manifest.diagnostic.get_or_insert_with(|| CaptureDiagnosticV1 {stage:DiagnosticStage::Cursor,message:reason.into()});
    }

    pub(super) fn request_stop(&self) {if let Some(sampler)=&self.sampler {sampler.request_stop();}}
    pub(super) fn join(&mut self) {if let Some(sampler)=self.sampler.take() {self.outcome=Some(sampler.finish());}}
    pub(super) fn publish(mut self, dir:&Path, saved:&ProbedVideoEvidence,
        clock:Option<&video_clock::AcceptedClock>, interactions:&InteractionEvents, has_mic:bool) -> RecorderResult<()> {
        self.join();
        let outcome=self.outcome.take().ok_or_else(||RecorderError::new("capture_finalize_failed", "Cursor completion is missing; retain this recording for recovery.", true))?;
        let cursor=cursor::finalize(dir,self.source,outcome,clock).map_err(recovery)?;
        complete_snapshot(&mut self.manifest,self.defaults,saved,clock,cursor,interactions,has_mic)?;
        capture::publish_finalized_assets(dir,&self.manifest,saved).map_err(recovery)?;
        Ok(())
    }
}

pub(super) fn recovery(error:impl std::fmt::Display)->RecorderError {
    RecorderError::new("capture_finalize_failed",format!("Recording files were retained for recovery. Reload before trying recovery: {error}"),true)
}
fn incomplete(id:&str, kind:SourceKind, source:cursor::Source)->CaptureManifestV1 {
    let b=source.bounds;
    CaptureManifestV1 {schema_version:1,capture_id:id.into(),
        source:NativeSourceV1 {backend:Backend::LinuxX11,kind,initial_bounds:Bounds {x:b.x,y:b.y,width:b.width,height:b.height},window_id:source.window.map(|id|id.to_string())},
        diagnostic:None,state:CaptureStateV1::Incomplete {video_path:VideoPath::Screen,video_cursor:SeparateCursor::Separate,raw_samples_path:RawSamplesPath::Raw}}
}

// Stop serialization at the existing input limit before allocating a serde Value tree.
fn bounded_json(value:&impl Serialize)->RecorderResult<serde_json::Value> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self,bytes:&[u8])->std::io::Result<usize> {
            if bytes.len() as u64 > MAX_JSON_BYTES.saturating_sub(self.0.len() as u64) {
                return Err(std::io::Error::other("Native interactions exceed the 8 MiB input limit"));
            }
            self.0.extend_from_slice(bytes);Ok(bytes.len())
        }
        fn flush(&mut self)->std::io::Result<()> {Ok(())}
    }
    let mut bytes=Bounded(Vec::new());
    serde_json::to_writer(&mut bytes,value).map_err(recovery)?;
    serde_json::from_slice(&bytes.0).map_err(recovery)
}

fn complete_snapshot(manifest:&mut CaptureManifestV1,defaults:LegacyDefaults,saved:&ProbedVideoEvidence,
    clock:Option<&video_clock::AcceptedClock>,cursor:cursor::FinalizedCursor,events:&InteractionEvents,has_mic:bool)->RecorderResult<()> {
    let clicks=bounded_json(&events.clicks)?;
    let drags=bounded_json(&events.drags)?;
    let timeline=build_timeline_segments(events.clicks.clone(),events.drags.clone());
    let converted=legacy::convert_interactions(saved.video,defaults,bounded_json(&timeline)?,clicks,Some(drags)).map_err(recovery)?;
    let (mode,diagnostic)=match cursor {
        cursor::FinalizedCursor::Separate {degraded}=>(CursorMode::Separate,degraded),
        cursor::FinalizedCursor::Absent {reason}=>(CursorMode::Absent,Some(reason)),
    };
    let separate=mode==CursorMode::Separate;
    let mut warnings=vec![ProjectWarning::LegacyTimingUnknown];
    warnings.extend(converted.warnings);
    if let Some(message) = diagnostic {
        let source_end = manifest.diagnostic.take();
        manifest.diagnostic = Some(CaptureDiagnosticV1 {
            stage:if clock.is_some(){DiagnosticStage::Cursor}else{DiagnosticStage::Clock},
            message:source_end.map_or_else(||message.into(), |notice|format!("{message}. {}",notice.message)),
        });
    }
    // A source-end notice disables initial drawing as a precaution; the retained cursor quality is unchanged.
    let drawing_disabled = manifest.diagnostic.is_some();
    if drawing_disabled {warnings.push(ProjectWarning::CursorCaptureIncomplete);}
    let endpoint=saved.endpoint;
    manifest.state=CaptureStateV1::Finalized {
        video_timing:VideoTimingV1 {
            endpoint:SavedVideoEndpointV1 {provenance:EndpointProvenance::SavedPacketFirstAndTail,end_pts:endpoint.ticks,time_base:TimeBase {numerator:endpoint.numerator,denominator:endpoint.denominator},interval:Interval::HalfOpen},
            clock:clock.map(|clock|AcceptedX11ClockV1 {source_clock:SourceClock::UnixMicrosecondsPreImageCopyRequest,epoch:Epoch::CaptureLaunchMonotonicNs,
                anchor:Anchor {low_ns:clock.anchor_low_ns.to_string(),high_exclusive_ns:clock.anchor_high_exclusive_ns.to_string()},completed_frames:clock.completed_frames,
                cadence:TimeBase {numerator:1,denominator:30},provenance:ClockProvenance::CompletedEncoderGridAndSavedPacketCadence,interactions:Interactions::DeliveryTimeRebasedOnce,audio:AudioTiming::Unmeasured}),
        },
        cursor_timing:separate.then_some(CursorTimingV1 {clock:CursorClock::VideoFrameZeroRelativeUs,observation:Observation::X11RequestBracketMidpoint,initial_state:InitialState::BoundedPreRollHold,
            visibility:Visibility::SameScreenNativeImageOrArrow1xIntersectsCrop,server_visibility:ServerVisibility::Unknown}),
        initial_project:ProjectDocument {
            schema_version:3,revision:0,project_id:manifest.capture_id.clone(),frame_style:Default::default(),
            video:VideoAsset {path:"screen.mp4".into(),width:saved.video.width,height:saved.video.height,duration_us:saved.video.duration_us,cursor:mode},
            cursor:separate.then_some(CursorAsset {index_path:"cursor/index.json".into()}),cursor_settings:CursorSettings {enabled:separate&&!drawing_disabled,..Default::default()},
            microphone:has_mic.then_some(AudioAsset {path:"mic.wav".into()}),timing:Timing {provenance:TimingProvenance::LegacyUnknown,audio_offset_us:None},
            camera:CameraDefaults {pan_transition_us:defaults.pan_transition_us,settle_us:defaults.settle_us},
            clips:vec![Clip {id:"clip-0".into(),in_us:0,out_us:saved.video.duration_us}],zooms:converted.zooms,clicks:converted.clicks,warnings,
        },
    };
    manifest.validate(&manifest.capture_id).map_err(recovery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::*;
    use focusflow_core::ffmpeg::{PacketTime, ProbedVideo, ProbedVideoEvidence};
    fn source() -> cursor::Source { cursor::Source { bounds: CaptureBounds { x:20,y:40,width:320,height:240 }, window:None } }
    fn saved() -> ProbedVideoEvidence { ProbedVideoEvidence { video:ProbedVideo { width:320,height:240,duration_us:3_000_000 }, endpoint:PacketTime {ticks:90,numerator:1,denominator:30},first_packet_duration:PacketTime {ticks:1,numerator:1,denominator:30} } }
    fn clock() -> video_clock::AcceptedClock { video_clock::AcceptedClock { anchor_low_ns:100_000_000,anchor_high_exclusive_ns:100_000_100,frame_zero_ns:100_000_050,completed_frames:90,saved_endpoint:saved().endpoint } }
    fn events() -> InteractionEvents { InteractionEvents {clicks:vec![ClickEvent {timestamp:1.0,x:80,y:120,button:ClickButton::Left}],drags:vec![]} }
    #[test]
    fn native_snapshot_freezes_defaults_geometry_interactions_and_truthful_modes() {
        let mut defaults=crate::export::ExportSettings::default().legacy_defaults().unwrap();
        defaults.pan_transition_us=180_000;defaults.scale=3.0;
        for result in [cursor::FinalizedCursor::Separate {degraded:None},cursor::FinalizedCursor::Separate {degraded:Some("Positions only")},cursor::FinalizedCursor::Absent {reason:"No accepted video clock"}] {
            let is_separate=matches!(result,cursor::FinalizedCursor::Separate {..});
            let enabled=matches!(result,cursor::FinalizedCursor::Separate {degraded:None});
            let mut manifest=incomplete("fixture",SourceKind::Region,source());
            complete_snapshot(&mut manifest,defaults,&saved(),Some(&clock()),result,&events(),false).unwrap();
            manifest.validate("fixture").unwrap();
            let doc=manifest.finalized_project().unwrap();
            assert_eq!(doc.schema_version,3);assert_eq!(doc.revision,0);assert_eq!(doc.camera.pan_transition_us,180_000);
            assert_eq!(doc.clips[0].out_us,3_000_000);assert_eq!(doc.clicks[0].at_us,1_000_000);assert_eq!(doc.clicks[0].x,0.25);
            assert!(doc.zooms.iter().all(|z|z.mode==focusflow_core::project::ZoomMode::Fixed));
            assert!(doc.zooms.iter().all(|z|z.scale==3.0));assert!(!doc.zooms.is_empty());
            assert_eq!(doc.cursor_settings.style,CursorStyle::Native);assert_eq!(doc.cursor_settings.scale,1.0);assert_eq!(doc.cursor_settings.smoothing_us,0);
            assert_eq!(doc.cursor_settings.enabled,enabled);assert_eq!(doc.cursor.is_some(),is_separate);
            assert_eq!(manifest.diagnostic.is_some(),!enabled);assert_eq!(doc.warnings.contains(&ProjectWarning::CursorCaptureIncomplete),!enabled);
            assert!(!doc.warnings.contains(&ProjectWarning::EmbeddedCursor));assert!(doc.microphone.is_none());assert_eq!(doc.timing.audio_offset_us,None);
        }
    }
    #[test]
    fn rejected_clock_snapshot_is_absent_and_preserves_uncorrected_legacy_events() {
        let mut manifest=incomplete("fixture",SourceKind::Region,source());
        complete_snapshot(&mut manifest,crate::export::ExportSettings::default().legacy_defaults().unwrap(),&saved(),None,cursor::FinalizedCursor::Absent {reason:"No accepted video clock"},&events(),true).unwrap();
        manifest.validate("fixture").unwrap();
        let CaptureStateV1::Finalized {video_timing,cursor_timing,initial_project:doc}=&manifest.state else {panic!()};
        assert!(video_timing.clock.is_none()&&cursor_timing.is_none());assert_eq!(doc.video.cursor,CursorMode::Absent);
        assert_eq!(doc.clicks[0].at_us,1_000_000);assert_eq!(doc.microphone.as_ref().unwrap().path,"mic.wav");
    }
    #[test]
    fn native_json_serialization_is_bounded_before_value_conversion() {
        assert!(bounded_json(&"x".repeat(MAX_JSON_BYTES as usize)).is_err());
        assert_eq!(bounded_json(&vec![1,2]).unwrap(),serde_json::json!([1,2]));
    }
    #[test]
    fn attempted_authority_closes_cleanup_even_on_failed_publication() {
        let dir=std::env::temp_dir().join(format!("focusflow-authority-gate-{}",std::process::id()));
        fs::create_dir_all(dir.join("capture.json")).unwrap();
        let state=RecorderState::new();let mut reservation=StartReservation::acquire(&state).unwrap();reservation.directory=Some(dir.clone());
        let id=dir.file_name().unwrap().to_str().unwrap();let manifest=incomplete(id,SourceKind::Region,source());
        assert!(reservation.publish_incomplete(&dir,&manifest).is_err());drop(reservation);
        assert!(dir.exists());assert!(!state.lock_runtime().unwrap().starting);fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn cleanup_guards_release_waiters_even_after_runtime_poison() {
        let state=RecorderState::new();let reservation=StartReservation::acquire(&state).unwrap();
        let (sender,mut receiver)=tauri::async_runtime::channel(1);
        state.lock_runtime().unwrap().lifecycle_waiters.push(sender);
        let _=std::panic::catch_unwind(||{let _guard=state.runtime.lock().unwrap();panic!("controlled poison")});
        drop(reservation);assert!(receiver.try_recv().is_ok());assert!(!state.cleanup_runtime().starting);
        state.cleanup_runtime().finalizing=true;
        let (sender,mut receiver)=tauri::async_runtime::channel(1);state.cleanup_runtime().lifecycle_waiters.push(sender);
        drop(FinalizingGuard(&state));assert!(receiver.try_recv().is_ok());assert!(!state.cleanup_runtime().finalizing);
        tauri::async_runtime::block_on(shutdown(&state));assert!(state.cleanup_runtime().shutting_down);
    }
    #[test]
    fn unsupported_clock_has_a_bounded_non_sensitive_notice() {
        let capability=video_clock::Capability::probe(Path::new("/usr/bin/false"));
        assert!(!capability.supported());assert!(EMBEDDED_NOTICE.len()<160);assert!(!EMBEDDED_NOTICE.contains('/'));
        let status=RecordingStatus {phase:RecordingPhase::Recording,output_path:String::new(),pid:None,monitor:None,elapsed_ms:0,file_size_bytes:None,notice:Some(EMBEDDED_NOTICE.into())};
        assert_eq!(serde_json::to_value(&status).unwrap()["notice"],EMBEDDED_NOTICE);
        let status=RecordingStatus {notice:None,..status};assert!(serde_json::to_value(status).unwrap().get("notice").is_none());
    }
    #[test]
    fn snapshot_uses_owned_events_despite_stale_optional_sidecars() {
        let dir=std::env::temp_dir().join(format!("focusflow-owned-events-{}",std::process::id()));fs::create_dir_all(&dir).unwrap();
        for name in ["clicks.json","drags.json","timeline.json"] {fs::create_dir(dir.join(name)).unwrap();}
        assert!(write_clicks_json(&dir.join("screen.mp4"),&events().clicks).is_err());
        assert!(write_drags_json(&dir.join("screen.mp4"),&events().drags).is_err());
        let mut manifest=incomplete("fixture",SourceKind::Region,source());
        complete_snapshot(&mut manifest,crate::export::ExportSettings::default().legacy_defaults().unwrap(),&saved(),Some(&clock()),cursor::FinalizedCursor::Absent {reason:"No trustworthy cursor observation prefix"},&events(),false).unwrap();
        assert_eq!(manifest.finalized_project().unwrap().clicks[0].at_us,1_000_000);
        assert!(!manifest.finalized_project().unwrap().zooms.is_empty());fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn missing_required_source_retains_incomplete_authority() {
        let dir=std::env::temp_dir().join(format!("focusflow-final-gate-{}",std::process::id()));fs::create_dir_all(&dir).unwrap();let dir=dir.canonicalize().unwrap();
        let id=dir.file_name().unwrap().to_str().unwrap();let manifest=incomplete(id,SourceKind::Region,source());
        capture::publish(&dir,&manifest).unwrap();let original=fs::read(dir.join("capture.json")).unwrap();
        // Shared validator is supplied exact synthetic evidence here, never claimed as an encoder clock.
        let outcome=cursor::Outcome {raw_path:dir.join("cursor/raw.ndjson"),committed_bytes:0,records:0,shapes:vec![],cutoff_ns:None,trustworthy_prefix:false,degraded:true,reason:None};
        let session=Session {sampler:None,outcome:Some(outcome),source:source(),manifest,defaults:crate::export::ExportSettings::default().legacy_defaults().unwrap()};
        assert!(session.publish(&dir,&saved(),None,&events(),false).is_err());
        assert_eq!(fs::read(dir.join("capture.json")).unwrap(),original);assert!(!dir.join("project.json").exists());fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn linux_output_bounds_are_output_scoped_and_keep_embedded_until_admission() {
        let args=x11::args(source().bounds,None,Path::new("screen.mp4")).unwrap();
        let input=args.iter().position(|x|x=="-i").unwrap();
        for (flag,value) in [("-r","30"),("-t","86400")] {let at=args.iter().position(|x|x==flag).expect("missing output bound");assert!(at>input);assert_eq!(args[at+1],value);}
        let mouse=args.iter().position(|x|x=="-draw_mouse").unwrap();assert_eq!(args[mouse+1],"1");
    }
}

#[cfg(test)]
#[path = "native_capture/tests.rs"]
mod lifecycle_tests;
