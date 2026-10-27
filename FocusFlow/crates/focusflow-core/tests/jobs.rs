mod common;
use common::{fixture_project, TestDir};
use focusflow_core::{jobs::{JobManager, JobState}, render::ExportPreset, ErrorCode};
use std::{fs, path::{Path, PathBuf}};
fn setup() -> (TestDir, PathBuf, JobManager) {
    let temp = TestDir::new(); let project = temp.path().join("fixture");
    fs::create_dir(&project).unwrap();
    common::write_legacy(&project);
    fs::create_dir_all(project.join("exports/previous")).unwrap();
    fs::write(project.join("exports/previous/output.mp4"), b"previous good export").unwrap();
    fs::write(project.join("edited.mp4"), b"legacy good export").unwrap();
    fs::write(project.join("project.json"), serde_json::to_vec(&fixture_project()).unwrap()).unwrap();
    let manager = JobManager::new(temp.path(), Path::new("ffmpeg")).unwrap();
    (temp, project, manager)
}
fn originals(project: &Path) -> Vec<(String, Vec<u8>)> {
    ["screen.mp4", "metadata.json", "timeline.json", "clicks.json", "drags.json", "edited.mp4", "exports/previous/output.mp4"]
        .iter().map(|p| (p.to_string(), fs::read(project.join(p)).unwrap())).collect()
}
fn preserved(project: &Path, before: &[(String, Vec<u8>)]) {
    for (path, bytes) in before { assert_eq!(&fs::read(project.join(path)).unwrap(), bytes, "{path}"); }
}
#[test]
fn stale_revision_does_not_start_encoder() {
    let (_temp, project, manager) = setup();
    let before = originals(&project);
    let error = manager.start_export("fixture", 1, ExportPreset::Balanced).unwrap_err();
    assert_eq!(error.code, ErrorCode::RevisionConflict);
    assert_eq!(fs::read_dir(project.join("exports")).unwrap().count(), 1);
    preserved(&project, &before);
}
#[test]
fn failure_preserves_previous_exports() {
    let (_temp, project, manager) = setup(); let before = originals(&project);
    let started = manager.start_export("fixture", 0, ExportPreset::Balanced).unwrap();
    let result = manager.wait("fixture", &started.job_id).unwrap();
    assert_eq!(result.state, JobState::Failed);
    assert_eq!(result.error.unwrap().code, ErrorCode::EncoderFailed);
    assert!(result.output_relative_path.is_none());
    assert!(!project.join("exports").join(&started.job_id).join("output.mp4").exists());
    preserved(&project, &before);
}

#[test]
fn invalid_job_ids_and_unsafe_export_directories_are_rejected() {
    let (_temp, project, manager) = setup();
    for id in ["../outside", "", "C:drive", "a/b"] {
        assert_eq!(manager.status("fixture", id).unwrap_err().code, ErrorCode::InvalidId);
        assert_eq!(manager.cancel("fixture", id).unwrap_err().code, ErrorCode::InvalidId);
    }
    fs::remove_dir_all(project.join("exports")).unwrap();
    fs::write(project.join("exports"), b"not a directory").unwrap();
    assert_eq!(manager.start_export("fixture", 0, ExportPreset::Balanced).unwrap_err().code, ErrorCode::InvalidPath);
}

#[cfg(unix)]
#[test]
fn export_symlinks_cannot_escape_the_project() {
    let (temp, project, manager) = setup(); let outside = TestDir::new();
    fs::remove_dir_all(project.join("exports")).unwrap();
    std::os::unix::fs::symlink(outside.path(), project.join("exports")).unwrap();
    assert_eq!(manager.start_export("fixture", 0, ExportPreset::Balanced).unwrap_err().code, ErrorCode::InvalidPath);
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    fs::remove_file(project.join("exports")).unwrap(); fs::create_dir(project.join("exports")).unwrap();
    std::os::unix::fs::symlink(outside.path(), project.join("exports/escape")).unwrap();
    assert_eq!(manager.status("fixture", "escape").unwrap_err().code, ErrorCode::InvalidPath);
    assert!(temp.path().exists());
}

#[test]
#[ignore = "requires FFmpeg and FFprobe; generated media only"]
fn real_job_uses_production_graph() {
    use focusflow_core::project::*;
    let (temp, project, manager) = setup();
    let output = std::process::Command::new("ffmpeg").args(["-hide_banner", "-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x96:rate=60:duration=3", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"]).arg(project.join("screen.mp4")).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let mut writer = hound::WavWriter::create(project.join("mic.wav"), hound::WavSpec { channels: 1, sample_rate: 48000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }).unwrap();
    for n in 0..48000 { writer.write_sample(((n as f64 * 440.0 * std::f64::consts::TAU / 48000.0).sin() * 2000.0) as i16).unwrap(); } writer.finalize().unwrap();
    let mut doc = fixture_project(); doc.video.width = 160; doc.video.height = 96; doc.video.duration_us = 3_000_000;
    doc.microphone = Some(AudioAsset { path: "mic.wav".into() });
    doc.clips = vec![Clip { id: "first".into(), in_us: 0, out_us: 1_000_000 }, Clip { id: "second".into(), in_us: 2_000_000, out_us: 3_000_000 }];
    doc.zooms = vec![ZoomElement { mode: focusflow_core::project::ZoomMode::Fixed, id: "zoom".into(), clip_id: "first".into(), start_us: 100_000, end_us: 800_000, x: 0.4, y: 0.5, scale: 2.0, intro_us: 100_000, hold_us: 50_000, outro_us: 50_000 }];
    doc.clicks = vec![Click { at_us: 500_000, x: 0.4, y: 0.5 }];
    fs::write(project.join("project.json"), serde_json::to_vec(&doc).unwrap()).unwrap();
    let before = originals(&project); let audio = fs::read(project.join("mic.wav")).unwrap();
    let mut outputs = Vec::new();
    for preset in [ExportPreset::Balanced, ExportPreset::HighQuality] {
        let job = manager.start_export("fixture", 0, preset).unwrap();
        let done = manager.wait("fixture", &job.job_id).unwrap();
        assert_eq!(done.state, JobState::Completed, "{done:?}");
        let path = project.join(done.output_relative_path.as_ref().unwrap());
        let probe = std::process::Command::new("ffprobe").args(["-v", "error", "-show_streams", "-of", "json"]).arg(&path).output().unwrap();
        assert!(probe.status.success()); let info: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let streams = info["streams"].as_array().unwrap();
        let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
        let sound = streams.iter().find(|s| s["codec_type"] == "audio").unwrap();
        assert_eq!(video["codec_name"], "h264"); assert_eq!(sound["codec_name"], "aac");
        assert_eq!(video["r_frame_rate"], format!("{}/1", preset.fps()));
        for stream in [video, sound] { let duration: f64 = stream["duration"].as_str().unwrap().parse().unwrap(); assert!((duration - 2.0).abs() <= 1.0 / preset.fps() as f64 + 0.001, "{stream}"); }
        let graph = fs::read_to_string(path.parent().unwrap().join(focusflow_core::ffmpeg::FILTER_SCRIPT)).unwrap();
        assert!(graph.contains("trim=start_frame=")); assert!(graph.contains(":d=1:")); assert!(graph.contains("atrim"));
        assert_eq!(JobManager::new(temp.path(), Path::new("ffmpeg")).unwrap().status("fixture", &job.job_id).unwrap(), done);
        let record: serde_json::Value = serde_json::from_slice(&fs::read(path.parent().unwrap().join("job.json")).unwrap()).unwrap();
        assert_eq!(record["preset"], serde_json::to_value(preset).unwrap());
        outputs.push((path.clone(), fs::read(path).unwrap()));
    }
    assert_ne!(outputs[0].0, outputs[1].0);
    for (path, bytes) in outputs { assert_eq!(fs::read(path).unwrap(), bytes); }
    preserved(&project, &before); assert_eq!(fs::read(project.join("mic.wav")).unwrap(), audio);
    let summary: serde_json::Value = serde_json::from_slice(&fs::read(project.join("session.json")).unwrap()).unwrap();
    assert_eq!(summary["exported"], true); assert_eq!(summary["session_id"], "old");
}


#[test]
fn render_budget_rejection_reserves_no_job_and_releases_locks() {
    use focusflow_core::project::{Click, Clip};
    let (_temp, project, manager) = setup();
    let before = originals(&project);
    let mut doc = fixture_project();
    doc.clips = (0..64).map(|i| Clip { id: format!("repeat-{i}"), in_us: 0, out_us: 1_000_000 }).collect();
    doc.clicks = (0..129).map(|i| Click { at_us: i, x: 0.5, y: 0.5 }).collect();
    fs::write(project.join("project.json"), serde_json::to_vec(&doc).unwrap()).unwrap();
    let error = manager.start_export("fixture", 0, ExportPreset::Balanced).unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidDocument);
    assert_eq!(error.field.as_deref(), Some("render.clicks"));
    assert_eq!(fs::read_dir(project.join("exports")).unwrap().count(), 1);
    preserved(&project, &before);
    fs::write(project.join("project.json"), serde_json::to_vec(&fixture_project()).unwrap()).unwrap();
    let next = manager.start_export("fixture", 0, ExportPreset::Balanced).unwrap();
    assert_eq!(manager.wait("fixture", &next.job_id).unwrap().state, JobState::Failed);
}

#[test]
fn named_export_rejects_unsafe_basename_before_job_creation() {
    let (_temp, project, manager)=setup();
    for name in ["output.partial.mp4","OUTPUT.PARTIAL.MP4","../x.mp4","a/b.mp4","C:x.mp4","a\\b.mp4","CON.mp4","com1.mp4","LPT9.mp4","bad.mp4 ","bad.mp4.","x.png", ".mp4", "x?.mp4"] {
        assert!(manager.start_named_export("fixture",0,ExportPreset::Balanced,name).is_err(), "accepted {name:?}");
    }
    assert_eq!(fs::read_dir(project.join("exports")).unwrap().count(),1);
}
#[test]
fn old_completed_output_resource_is_readable_without_current_document() {
    let (_temp, project, manager)=setup();
    let dir=project.join("exports/old-job");fs::create_dir(&dir).unwrap();fs::write(dir.join("output.mp4"),b"old output").unwrap();
    fs::write(dir.join("job.json"),r#"{"schemaVersion":1,"preset":"balanced","status":{"jobId":"old-job","projectId":"fixture","revision":0,"state":"completed","percentage":100.0,"outputRelativePath":"exports/old-job/output.mp4","error":null,"warnings":[]}}"#).unwrap();
    fs::remove_file(project.join("project.json")).unwrap();
    let output=manager.output("fixture","old-job").unwrap();
    assert_eq!(output.mime_type,"video/mp4");assert!(output.metadata.is_none());
    assert_eq!(output.path,dir.join("output.mp4"));
    let chunk=manager.read_output_chunk("fixture","old-job",4,100).unwrap();
    assert_eq!(chunk.bytes,b"output");assert_eq!(chunk.total_bytes,10);assert!(chunk.eof);
    assert!(manager.read_output_chunk("fixture","old-job",11,1).is_err());
    assert!(manager.read_output_chunk("fixture","old-job",0,0).is_err());
    assert!(manager.read_output_chunk("fixture","old-job",0,1_048_577).is_err());
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated media only"]
fn real_preview_jobs_publish_bounded_video_png_and_named_exports() {
    use focusflow_core::{preview::PreviewRequest,jobs::JobKind,project::*};
    let (temp,project,manager)=setup();
    let generated=std::process::Command::new("ffmpeg").args(["-v","error","-y","-f","lavfi","-i","testsrc2=size=160x96:rate=60:duration=3","-c:v","libx264","-pix_fmt","yuv420p"]).arg(project.join("screen.mp4")).output().unwrap();assert!(generated.status.success());
    let mut wav=hound::WavWriter::create(project.join("mic.wav"),hound::WavSpec{channels:1,sample_rate:44100,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();for n in 0..44100{wav.write_sample(((n as f64*0.1).sin()*10000.0) as i16).unwrap();}wav.finalize().unwrap();
    let mut doc=fixture_project();doc.video.width=160;doc.video.height=96;doc.video.duration_us=3_000_000;doc.clips[0].out_us=3_000_000;doc.microphone=Some(AudioAsset{path:"mic.wav".into()});doc.timing.audio_offset_us=Some(123_457);doc.timing.provenance=TimingProvenance::Measured;
    fs::write(project.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
    let summary=fs::read(project.join("session.json")).unwrap();let before=originals(&project);
    for request in [PreviewRequest::Video{start_us:33_334,end_us:1_200_001},PreviewRequest::Still{at_us:233_334}] {
        let started=manager.start_preview("fixture",0,request).unwrap();let done=manager.wait("fixture",&started.job_id).unwrap();assert_eq!(done.state,JobState::Completed,"{done:?}");
        let output=manager.output("fixture",&started.job_id).unwrap();let metadata=output.metadata.as_ref().unwrap();
        let probe=std::process::Command::new("ffprobe").args(["-v","error","-show_streams","-of","json"]).arg(&output.path).output().unwrap();assert!(probe.status.success());let value:serde_json::Value=serde_json::from_slice(&probe.stdout).unwrap();let streams=value["streams"].as_array().unwrap();
        let video=streams.iter().find(|v|v["codec_type"]=="video").unwrap();assert_eq!(video["width"],metadata.width);assert_eq!(video["height"],metadata.height);
        if done.kind==JobKind::PreviewStill {assert_eq!(video["codec_name"],"png");assert_eq!(streams.len(),1);assert_eq!(output.mime_type,"image/png");}
        else {
            assert_eq!(video["nb_frames"],metadata.frame_count.to_string());let audio=streams.iter().find(|v|v["codec_type"]=="audio").unwrap();assert_eq!(audio["codec_name"],"aac");
            let duration:f64=audio["duration"].as_str().unwrap().parse().unwrap();assert!((duration-metadata.duration_us as f64/1_000_000.0).abs()<0.034,"audio duration={duration}, metadata={metadata:?}");
        }
        assert_eq!(fs::read(project.join("session.json")).unwrap(),summary);
        let reopened=JobManager::new(temp.path(),Path::new("ffmpeg")).unwrap();assert_eq!(reopened.output("fixture",&started.job_id).unwrap(),output);
        let mut bytes=Vec::new();loop {let chunk=reopened.read_output_chunk("fixture",&started.job_id,bytes.len() as u64,101).unwrap();bytes.extend(chunk.bytes);if chunk.eof{break;}}assert_eq!(bytes,fs::read(&output.path).unwrap());
    }
    // Unusable optional WAV stays video-only, while source/session bytes remain intact.
    fs::write(project.join("mic.wav"),b"corrupt WAV").unwrap();
    let started=manager.start_preview("fixture",0,PreviewRequest::Video{start_us:0,end_us:100_000}).unwrap();assert_eq!(manager.wait("fixture",&started.job_id).unwrap().state,JobState::Completed);
    let output=manager.output("fixture",&started.job_id).unwrap();let probe=std::process::Command::new("ffprobe").args(["-v","error","-show_streams","-of","json"]).arg(&output.path).output().unwrap();let value:serde_json::Value=serde_json::from_slice(&probe.stdout).unwrap();assert_eq!(value["streams"].as_array().unwrap().len(),1);assert_eq!(fs::read(project.join("session.json")).unwrap(),summary);
    let started=manager.start_named_export("fixture",0,ExportPreset::Balanced,"Named demo.mp4").unwrap();assert_eq!(manager.wait("fixture",&started.job_id).unwrap().state,JobState::Completed);let output=manager.output("fixture",&started.job_id).unwrap();assert_eq!(output.path.file_name().unwrap(),"Named demo.mp4");preserved(&project,&before);
}
