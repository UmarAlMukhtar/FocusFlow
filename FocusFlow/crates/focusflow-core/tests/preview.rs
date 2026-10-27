mod common;
use focusflow_core::{preview::{self, PreviewRequest}, render::{self, ExportPreset}};
#[test]
fn preview_selects_original_rational_grid() {
    let plan=render::compile(&common::fixture_project(), ExportPreset::Balanced).unwrap();
    let s=preview::select(&plan, &PreviewRequest::Video { start_us: 33_334, end_us: 100_001 }).unwrap();
    assert_eq!((s.first_frame,s.frame_count,s.fps,s.start_us,s.duration_us),(2,2,30,66_666,66_666));
    let s=preview::select(&plan, &PreviewRequest::Still { at_us: 33_334 }).unwrap();
    assert_eq!((s.first_frame,s.frame_count),(1,1));
    for request in [PreviewRequest::Video {start_us:1,end_us:2},PreviewRequest::Video {start_us:0,end_us:0},PreviewRequest::Video {start_us:0,end_us:30_000_001},PreviewRequest::Still{at_us:plan.duration_us}] {
        assert!(preview::select(&plan,&request).is_err(), "{request:?}");
    }
    assert!(serde_json::from_str::<PreviewRequest>(r#"{"type":"still","atUs":0,"extra":0}"#).is_err());
}

#[test]
fn preview_decoder_budget_counts_video_and_wav_before_start() {
    use focusflow_core::{project::*,store::ProjectStore,jobs::JobManager,ErrorCode};
    use std::{fs,path::Path};
    let temp=common::TestDir::new();let dir=temp.path().join("fixture");fs::create_dir(&dir).unwrap();common::write_legacy(&dir);
    let mut doc=common::fixture_project();doc.clips=(0..17).map(|i|Clip{id:format!("c{i}"),in_us:0,out_us:100_000}).collect();
    doc.microphone=Some(AudioAsset{path:"mic.wav".into()});
    let mut wav=hound::WavWriter::create(dir.join("mic.wav"),hound::WavSpec{channels:1,sample_rate:8000,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();for _ in 0..8000{wav.write_sample(1i16).unwrap();}wav.finalize().unwrap();
    fs::write(dir.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
    let store=ProjectStore::new(temp.path()).unwrap();let snapshot=store.snapshot("fixture",0).unwrap();let plan=render::compile(&doc,ExportPreset::Balanced).unwrap();
    let sel=preview::select(&plan,&PreviewRequest::Video{start_us:0,end_us:1_700_000}).unwrap();
    assert!(render::preview_filter_graph(&snapshot,&plan,&sel,true).unwrap_err().message.contains("32"));
    let manager=JobManager::new(temp.path(),Path::new("absent-encoder")).unwrap();
    let error=manager.start_preview("fixture",0,PreviewRequest::Video{start_us:0,end_us:1_700_000}).unwrap_err();assert_eq!(error.code,ErrorCode::InvalidDocument);assert!(!dir.join("exports").exists());
    let sel=preview::select(&plan,&PreviewRequest::Video{start_us:0,end_us:1_600_000}).unwrap();
    let args=focusflow_core::ffmpeg::preview_args(&snapshot,&plan,&sel,&dir,true,false).unwrap();
    assert_eq!(args.iter().filter(|a|*a=="-i").count(),32);
    assert_eq!(args.windows(2).filter(|pair|pair[0]=="-threads" && pair[1]=="1").count(),32);
}
#[test]
fn preview_bounds_exact_end_and_tiny_last_frame_are_explicit() {
    let mut doc=common::fixture_project();doc.video.duration_us=40_000_000;doc.clips[0].out_us=40_000_000;
    let plan=render::compile(&doc,ExportPreset::Balanced).unwrap();
    let s=preview::select(&plan,&PreviewRequest::Video{start_us:1,end_us:30_000_001}).unwrap();assert_eq!((s.first_frame,s.frame_count),(1,900));
    let s=preview::select(&plan,&PreviewRequest::Still{at_us:39_999_999}).unwrap();assert_eq!((s.first_frame,s.frame_count),(1199,1));
    assert!(preview::select(&plan,&PreviewRequest::Video{start_us:u64::MAX,end_us:u64::MAX}).is_err());
    doc.clips[0].out_us=1_000_001;let plan=render::compile(&doc,ExportPreset::Balanced).unwrap();
    let s=preview::select(&plan,&PreviewRequest::Video{start_us:1_000_000,end_us:1_000_001}).unwrap();assert_eq!((s.first_frame,s.frame_count),(30,1));
}
