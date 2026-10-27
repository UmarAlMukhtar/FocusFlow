use std::{ffi::OsString, fs, path::{Path, PathBuf}, process::Command, sync::atomic::{AtomicU64, Ordering}};
use focusflow_core::{ffmpeg, project::*, render::{self, ExportPreset}, store::RenderSnapshot};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new(width: u32, height: u32) -> Self {
        let path = std::env::temp_dir().join(format!("focusflow-render-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        let fixture = Self(path);
        // Lossless synthetic source preserves odd dimensions and solid trim-boundary markers.
        run("ffmpeg", &["-v", "error", "-y", "-f", "lavfi", "-i", &format!("testsrc2=size={}x{}:rate=60:duration=3,format=yuv444p,crop={width}:{height}:exact=1,drawbox=color=black:t=fill,drawbox=color=green:t=fill:enable='between(t,0.8,0.9999)',drawbox=color=blue:t=fill:enable='between(t,1,1.9999)',drawbox=color=red:t=fill:enable='between(t,2,2.2499)'", width+1, height+1), "-c:v", "libx264", "-preset", "ultrafast", "-qp", "0", "-pix_fmt", "yuv444p", fixture.0.join("screen.mp4").to_str().unwrap()]);
        fixture
    }
    fn snapshot(&self, width: u32, height: u32) -> RenderSnapshot {
        RenderSnapshot { project_dir: self.0.clone(), prepared_focus: None, prepared_cursor: None, document: ProjectDocument {
            frame_style: Default::default(), schema_version: 1, project_id: "fixture".into(), revision: 0,
            cursor: None, cursor_settings: Default::default(),
        video: VideoAsset { cursor: CursorMode::Embedded, path: "screen.mp4".into(), width, height, duration_us: 3_000_000 },
            microphone: None, timing: Timing { provenance: TimingProvenance::Measured, audio_offset_us: Some(0) },
            camera: CameraDefaults { pan_transition_us: 180_000, settle_us: 120_000 },
            clips: vec![Clip { id: "first".into(), in_us: 0, out_us: 3_000_000 }],
            zooms: vec![], clicks: vec![], warnings: vec![],
        } }
    }
    fn audio(&self, seconds: f64, onset: f64) {
        run("ffmpeg", &["-v", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency=440:sample_rate=48000:duration={seconds},volume='if(lt(t,{onset}),0,1)':eval=frame"), self.0.join("mic.wav").to_str().unwrap()]);
    }
}
impl Drop for Fixture { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }
fn run(program: &str, args: &[&str]) -> Vec<u8> {
    let out = Command::new(program).args(args).output().unwrap();
    assert!(out.status.success(), "{program}: {}", String::from_utf8_lossy(&out.stderr)); out.stdout
}
fn render(snapshot: &RenderSnapshot, preset: ExportPreset, audio: bool) -> PathBuf {
    let plan = render::compile(&snapshot.document, preset).unwrap();
    let graph = render::filter_graph(snapshot, &plan, audio).unwrap();
    assert!(graph.contains("trim=start_frame="), "production graph must retain mapped source ranges: {graph}");
    assert!(graph.contains(":d=1:"), "video must generate one zoompan frame per input frame");
    assert!(!graph.contains("drawtext"));
    fs::write(snapshot.project_dir.join(ffmpeg::FILTER_SCRIPT), graph).unwrap();
    let args = ffmpeg::render_args(snapshot, &plan, &snapshot.project_dir, audio).unwrap();
    assert!(!args.contains(&OsString::from("-af")), "padding belongs inside the complex graph");
    let out = Command::new("ffmpeg").args(args).output().unwrap();
    assert!(out.status.success(), "render: {}", String::from_utf8_lossy(&out.stderr));
    snapshot.project_dir.join(ffmpeg::RENDER_OUTPUT)
}
fn probe(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&run("ffprobe", &["-v", "error", "-show_streams", "-of", "json", path.to_str().unwrap()])).unwrap()
}
fn check(path: &Path, fps: u32, duration: f64, audio: bool, width: u32, height: u32) {
    let p = probe(path); let streams = p["streams"].as_array().unwrap();
    let video = streams.iter().find(|s| s["codec_type"] == "video").unwrap();
    assert_eq!(video["codec_name"], "h264"); assert_eq!(video["pix_fmt"], "yuv420p");
    assert_eq!(video["width"], width); assert_eq!(video["height"], height);
    assert_eq!(video["r_frame_rate"], format!("{fps}/1"));
    let vd: f64 = video["duration"].as_str().unwrap().parse().unwrap();
    assert!((vd-duration).abs() <= 1.0/fps as f64 + 0.001, "video duration={vd}, expected={duration}");
    let a = streams.iter().find(|s| s["codec_type"] == "audio"); assert_eq!(a.is_some(), audio);
    if let Some(a) = a {
        assert_eq!(a["codec_name"], "aac");
        let ad: f64 = a["duration"].as_str().unwrap().parse().unwrap();
        assert!((ad-duration).abs() <= 1.0/fps as f64 + 0.001, "audio duration={ad}");
        let start = |s: &serde_json::Value| s["start_time"].as_str().unwrap().parse::<f64>().unwrap();
        assert!((start(a)-start(video)).abs() < 0.05);
        println!("{fps}fps {width}x{height}: H264 {vd}s / AAC {ad}s; start delta={}s", start(a)-start(video));
    } else { println!("{fps}fps {width}x{height}: H264 {vd}s; audio absent"); }
}
fn frame(path: &Path, at: f64) -> Vec<u8> {
    run("ffmpeg", &["-v", "error", "-i", path.to_str().unwrap(), "-ss", &at.to_string(), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
}
fn pixel(frame: &[u8], width: usize, x: usize, y: usize) -> [u8;3] {
    let i=(y*width+x)*3; frame[i..i+3].try_into().unwrap()
}
fn loudness(path: &Path, start: f64, duration: f64) -> f64 {
    let bytes = run("ffmpeg", &["-v", "error", "-i", path.to_str().unwrap(), "-ss", &start.to_string(), "-t", &duration.to_string(), "-vn", "-ac", "1", "-ar", "48000", "-f", "f32le", "-"]);
    let values: Vec<f64> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap()) as f64).collect();
    assert!(!values.is_empty()); (values.iter().map(|v|v*v).sum::<f64>()/values.len() as f64).sqrt()
}
#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn retained_ranges_keep_audio_video_and_clicks_aligned() {
    for (preset, width, height) in [(ExportPreset::Balanced,160,96),(ExportPreset::HighQuality,161,97)] {
        let fixture=Fixture::new(width,height); fixture.audio(3.0,1.3);
        let mut s=fixture.snapshot(width,height); s.document.microphone=Some(AudioAsset{path:"mic.wav".into()});
        s.document.clips=vec![Clip{id:"first".into(),in_us:0,out_us:1_000_000},Clip{id:"second".into(),in_us:2_000_000,out_us:3_000_000}];
        s.document.zooms=vec![ZoomElement{mode:ZoomMode::Fixed,id:"zoom".into(),clip_id:"first".into(),start_us:0,end_us:1_000_000,x:0.5,y:0.5,scale:2.0,intro_us:200_000,hold_us:0,outro_us:0}];
        s.document.clicks=vec![Click{at_us:0,x:0.35,y:0.5},Click{at_us:200_000,x:0.35,y:0.5},Click{at_us:950_000,x:0.5,y:0.5},Click{at_us:1_500_000,x:0.5,y:0.5}];
        let source_video=fs::read(fixture.0.join("screen.mp4")).unwrap();
        let source_audio=fs::read(fixture.0.join("mic.wav")).unwrap();
        let out=render(&s,preset,true); let w=width/2*2; let h=height/2*2;
        assert_eq!(source_video,fs::read(fixture.0.join("screen.mp4")).unwrap());
        assert_eq!(source_audio,fs::read(fixture.0.join("mic.wav")).unwrap());
        check(&out,preset.fps(),2.0,true,w,h);
        let midpoint=frame(&out,0.1); // cosine intro midpoint: scale1.5, center x44, outer radius21
        let mid_center=pixel(&midpoint,w as usize,44,h as usize/2);
        let mid_edge=pixel(&midpoint,w as usize,62,h as usize/2);
        let mid_outside=pixel(&midpoint,w as usize,70,h as usize/2);
        assert!(mid_center.iter().all(|v|*v<30) && mid_edge.iter().all(|v|*v>80) && mid_outside.iter().all(|v|*v<30),
            "cosine camera midpoint ring: center={mid_center:?}, edge={mid_edge:?}, outside={mid_outside:?}");
        let ring=frame(&out,0.233334); let cx=(w as f64*0.2).round() as usize; let cy=h as usize/2;
        let center=pixel(&ring,w as usize,cx,cy); let edge=pixel(&ring,w as usize,cx+25,cy); let outside=pixel(&ring,w as usize,cx+33,cy);
        assert!(center.iter().all(|v|*v<30), "ring center must remain open: {center:?}");
        assert!(edge.iter().all(|v|*v>130), "ring radius must magnify with camera: {edge:?}");
        assert!(outside.iter().all(|v|*v<30), "outside ring radius: {outside:?}");
        let before=pixel(&frame(&out,0.9),w as usize,5,5); let after=pixel(&frame(&out,1.05),w as usize,5,5);
        assert!(before[1]>80 && before[0]<30, "retained pre-cut green: {before:?}");
        assert!(after[0]>180 && after[1]<30, "retained post-cut red: {after:?}");
        // The click at source .95 must stop at the originating clip's cut, even though its fade lasts .3s.
        let cut=pixel(&frame(&out,1.033334),w as usize,w as usize/2+13,h as usize/2);
        assert!(cut[0]>180 && cut[1]<40 && cut[2]<40, "ring leaked across cut: {cut:?}");
        let removed=pixel(&frame(&out,1.5),w as usize,w as usize/2+13,h as usize/2);
        assert!(removed.iter().all(|v|*v<30), "removed-range click leaked: {removed:?}");
        assert!(loudness(&out,0.7,0.1)<0.001 && loudness(&out,1.1,0.1)>0.05);
        println!("trimmed audio onset is silent before cut and audible after cut; cosine midpoint edge={mid_edge:?}");
        println!("retained green/red frames, transformed ring center={center:?} edge={edge:?} outside={outside:?}, cut={cut:?}");
    }
}
#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn optional_audio_lengths_do_not_trim_video() {
    let fixture=Fixture::new(160,96);
    for preset in [ExportPreset::Balanced,ExportPreset::HighQuality] {
        for kind in ["none","short","long","header","corrupt","disabled"] {
            let mut s=fixture.snapshot(160,96);
            match kind {
                "short"=>fixture.audio(0.4,0.0), "long"|"disabled"=>fixture.audio(5.0,0.0),
                "header"=>{ let spec=hound::WavSpec{channels:1,sample_rate:48000,bits_per_sample:16,sample_format:hound::SampleFormat::Int}; hound::WavWriter::create(fixture.0.join("mic.wav"),spec).unwrap().finalize().unwrap(); },
                "corrupt"=>fs::write(fixture.0.join("mic.wav"),vec![3;200]).unwrap(), _=>{}
            }
            if kind!="none" {s.document.microphone=Some(AudioAsset{path:"mic.wav".into()});}
            let audio=matches!(kind,"short"|"long"); let out=render(&s,preset,kind!="disabled");
            check(&out,preset.fps(),3.0,audio,160,96);
            if kind=="short" {assert!(loudness(&out,0.1,0.1)>0.05);assert!(loudness(&out,2.5,0.1)<0.001);}
            println!("optional audio case={kind} passed");
        }
    }
}
#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn signed_audio_offsets_are_applied_once() {
    let fixture=Fixture::new(160,96); fixture.audio(3.0,1.0);
    // Source audio onset 1s maps to video 0.8/1.2s; removing video [0,.4) maps these to .4/.8s.
    for offset in [-200_000,200_000] {
        let mut s=fixture.snapshot(160,96); s.document.microphone=Some(AudioAsset{path:"mic.wav".into()});
        s.document.timing.audio_offset_us=Some(offset);
        s.document.clips[0].in_us=400_000;
        let expected=0.6+offset as f64/1_000_000.0;
        let out=render(&s,ExportPreset::Balanced,true); check(&out,30,2.6,true,160,96);
        let before=loudness(&out,expected-0.13,0.05); let after=loudness(&out,expected+0.08,0.05);
        assert!(before<0.001 && after>0.05, "offset={offset}: onset expected={expected}, before={before}, after={after}");
        let bytes=run("ffmpeg", &["-v","error","-i",out.to_str().unwrap(),"-vn","-ac","1","-ar","48000","-f","f32le","-"]);
        let first=bytes.chunks_exact(4).position(|b| f32::from_le_bytes(b.try_into().unwrap()).abs()>0.03).unwrap() as f64/48000.0;
        assert!((first-expected).abs()<0.05, "decoded onset={first}, expected={expected}");
        // Visible blue onset1s maps to .6s; audible marker leads/lags it by the measured offset.
        let visual_before=pixel(&frame(&out,0.55),160,5,5);
        let visual_after=pixel(&frame(&out,0.65),160,5,5);
        assert!(visual_before[1]>80 && visual_before[0]<30 && visual_after[2]>180 && visual_after[0]<30,
            "visible onset expected .6s: before={visual_before:?}, after={visual_after:?}");
        assert!((first-0.6-offset as f64/1_000_000.0).abs()<0.05);
        let marker=pixel(&frame(&out,1.65),160,5,5); assert!(marker[0]>180 && marker[1]<30);
        println!("offset={offset}us decoded audible onset={first}s expected={expected}s before RMS={before} after RMS={after}; red video marker at1.65s={marker:?}");
    }
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn fractional_cuts_share_one_output_frame_grid() {
    let fixture=Fixture::new(160,96); fixture.audio(3.0,0.0);
    for preset in [ExportPreset::Balanced,ExportPreset::HighQuality] {
        let mut s=fixture.snapshot(160,96); s.document.microphone=Some(AudioAsset{path:"mic.wav".into()});
        s.document.clips=vec![Clip{id:"tiny-eof".into(),in_us:2_999_999,out_us:3_000_000}];
        for i in 0..16 {s.document.clips.push(Clip{id:format!("red-{i}"),in_us:2_000_000,out_us:2_049_999});}
        s.document.clips.push(Clip{id:"tiny-middle".into(),in_us:1_500_000,out_us:1_500_001});
        s.document.clips.push(Clip{id:"green-tail".into(),in_us:800_000,out_us:1_000_000});
        let duration=s.document.clips.iter().map(|c|c.out_us-c.in_us).sum::<u64>();
        let out=render(&s,preset,true); check(&out,preset.fps(),duration as f64/1_000_000.0,true,160,96);
        let p=probe(&out); let count=p["streams"][0]["nb_frames"].as_str().unwrap().parse::<u64>().unwrap();
        assert_eq!(count,(duration*preset.fps() as u64).div_ceil(1_000_000));
        let first=pixel(&frame(&out,0.0),160,5,5); assert!(first.iter().all(|v|*v<30),"tiny EOF range must sample last source frame: {first:?}");
        let tail=pixel(&frame(&out,0.9),160,5,5);
        assert!(tail[1]>80 && tail[0]<30,"fractional clips accumulated drift; expected retained green tail at .9s: {tail:?}");
        println!("fractional grid {}fps: {count}frames duration={}us, EOF={first:?}, green tail={tail:?}",preset.fps(),duration);
    }
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn single_source_frame_and_subframe_eof_cut_still_render() {
    let fixture=Fixture::new(160,96);
    run("ffmpeg", &["-v","error","-y","-f","lavfi","-i","testsrc2=size=160x96:rate=120:duration=0.008333","-frames:v","1","-c:v","libx264","-preset","ultrafast","-qp","0","-pix_fmt","yuv444p",fixture.0.join("screen.mp4").to_str().unwrap()]);
    for preset in [ExportPreset::Balanced,ExportPreset::HighQuality] {
        let mut s=fixture.snapshot(160,96); s.document.video.duration_us=8_333;
        s.document.clips[0].in_us=8_332; s.document.clips[0].out_us=8_333;
        let out=render(&s,preset,false); check(&out,preset.fps(),0.000001,false,160,96);
        let p=probe(&out); assert_eq!(p["streams"][0]["nb_frames"],"1");
        assert_eq!(frame(&out,0.0).len(),160*96*3);
        println!("single120fps source frame,1us EOF retained -> one{}fps output frame",preset.fps());
    }
}

#[test]
fn graph_and_args_validate_plan_paths_and_optional_audio_consistently() {
    let dir=std::env::temp_dir().join(format!("focusflow-render-contract-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
    fs::create_dir(&dir).unwrap(); let fixture=Fixture(dir);
    fs::write(fixture.0.join("screen.mp4"),b"path-only fixture; not decoded").unwrap();
    let mut s=fixture.snapshot(1,1);
    s.document.microphone=Some(AudioAsset{path:"missing.wav".into()});
    let plan=render::compile(&s.document,ExportPreset::SmallFile).unwrap();
    let graph=render::filter_graph(&s,&plan,true).unwrap();
    let args=ffmpeg::render_args(&s,&plan,&fixture.0,true).unwrap();
    assert!(graph.contains(":s=2x2:")); assert!(!graph.contains("[a]"));
    assert!(args.contains(&OsString::from("-an"))); assert!(!args.contains(&OsString::from("[a]")));
    assert!(args.windows(2).any(|pair|pair==[OsString::from("-protocol_whitelist"),OsString::from("file,pipe")]));
    assert!(args.windows(2).any(|pair|pair==[OsString::from("-f"),OsString::from("mov")]));
    assert!(args.windows(2).any(|pair|pair==[OsString::from("-enable_drefs"),OsString::from("0")]));
    assert!(args.windows(2).any(|pair|pair==[OsString::from("-use_absolute_path"),OsString::from("0")]));
    let wav_spec=hound::WavSpec{channels:1,sample_rate:48_000,bits_per_sample:16,sample_format:hound::SampleFormat::Int};
    let mut wav=hound::WavWriter::create(fixture.0.join("missing.wav"),wav_spec).unwrap();
    wav.write_sample(0i16).unwrap(); wav.finalize().unwrap();
    let with_audio=ffmpeg::render_args(&s,&plan,&fixture.0,true).unwrap();
    assert!(with_audio.windows(2).any(|pair|pair==[OsString::from("-f"),OsString::from("wav")]));
    let mut foreign=plan.clone(); foreign.duration_us+=1;
    assert_eq!(render::filter_graph(&s,&foreign,false).unwrap_err().code,focusflow_core::ErrorCode::InvalidDocument);
    assert_eq!(ffmpeg::render_args(&s,&foreign,&fixture.0,false).unwrap_err().code,focusflow_core::ErrorCode::InvalidDocument);
    s.document.microphone=Some(AudioAsset{path:"../outside.wav".into()});
    assert_eq!(render::filter_graph(&s,&plan,false).unwrap_err().code,focusflow_core::ErrorCode::InvalidDocument);
    assert_eq!(ffmpeg::render_args(&s,&plan,&fixture.0,false).unwrap_err().code,focusflow_core::ErrorCode::InvalidDocument);
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn fractional_audio_cuts_share_one_native_sample_grid() {
    let fixture=Fixture::new(160,96);
    run("ffmpeg", &["-v","error","-y","-f","lavfi","-i","sine=frequency=440:sample_rate=8000:duration=3",fixture.0.join("mic.wav").to_str().unwrap()]);
    let mut s=fixture.snapshot(160,96); s.document.microphone=Some(AudioAsset{path:"mic.wav".into()});
    s.document.clips=(0..300).map(|i|Clip{id:format!("tiny-{i}"),in_us:0,out_us:188}).collect();
    let plan=render::compile(&s.document,ExportPreset::HighQuality).unwrap();
    let graph=render::filter_graph(&s,&plan,true).unwrap();
    let script=fixture.0.join(ffmpeg::FILTER_SCRIPT); fs::write(&script,graph).unwrap();
    let wav=fixture.0.join("filtered.wav");
    // Decode the actual production graph to PCM: AAC padding would hide a one-native-sample bound.
    run("ffmpeg", &["-v","error","-y","-i",fixture.0.join("screen.mp4").to_str().unwrap(),"-i",fixture.0.join("mic.wav").to_str().unwrap(),"-filter_complex_script",script.to_str().unwrap(),"-map","[v]","-an","-f","null","-","-map","[a]","-c:a","pcm_s16le",wav.to_str().unwrap()]);
    let mut reader=hound::WavReader::open(wav).unwrap(); assert_eq!(reader.spec().sample_rate,8000);
    let expected=(plan.duration_us*8000).div_ceil(1_000_000);
    assert_eq!(reader.duration() as u64,expected,"per-clip native-sample rounding accumulated across300tiny cuts");
    let samples:Vec<i16>=reader.samples::<i16>().map(Result::unwrap).collect();
    let onset=samples.iter().position(|v|v.unsigned_abs()>300).unwrap(); assert!(onset<=2,"unexpected native onset={onset}");
    let out=render(&s,ExportPreset::HighQuality,true); check(&out,60,plan.duration_us as f64/1_000_000.0,true,160,96);
    let streams=probe(&out);
    assert_eq!(streams["streams"][0]["nb_frames"].as_str().unwrap().parse::<u64>().unwrap(),
        (plan.duration_us*60).div_ceil(1_000_000), "output arguments must preserve the graph's allocated frames");
    println!("300fractional cuts: native8kHz samples={} expected={expected}, onset sample={onset}, exact plan={}us",samples.len(),plan.duration_us);
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn contained_playlist_cannot_open_an_outside_media_segment() {
    let fixture=Fixture::new(160,96);
    let project=fixture.0.join("project"); fs::create_dir(&project).unwrap();
    let outside=fixture.0.join("outside.ts");
    run("ffmpeg", &["-v","error","-y","-f","lavfi","-i","color=c=red:size=160x96:rate=30:duration=1","-c:v","libx264","-pix_fmt","yuv420p","-f","mpegts",outside.to_str().unwrap()]);
    let playlist=project.join("screen.m3u8");
    fs::write(&playlist,b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:1\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:1.0,\n../outside.ts\n#EXT-X-ENDLIST\n").unwrap();
    let original_playlist=fs::read(&playlist).unwrap(); let original_outside=fs::read(&outside).unwrap();
    let probed=ffmpeg::probe_video(Path::new("ffmpeg"),&playlist);
    let mut s=fixture.snapshot(160,96); s.project_dir=project.clone();
    s.document.video.path="screen.m3u8".into(); s.document.video.duration_us=1_000_000; s.document.clips[0].out_us=1_000_000;
    let plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();
    fs::write(project.join(ffmpeg::FILTER_SCRIPT),render::filter_graph(&s,&plan,false).unwrap()).unwrap();
    let out=Command::new("ffmpeg").args(ffmpeg::render_args(&s,&plan,&project,false).unwrap()).output().unwrap();
    let decoded=out.status.success().then(||pixel(&frame(&project.join(ffmpeg::RENDER_OUTPUT),0.1),160,5,5));
    println!("contained HLS: probe accepted={}, render accepted={}, outside decoded RGB={decoded:?}",probed.is_ok(),out.status.success());
    assert_eq!(original_playlist,fs::read(&playlist).unwrap()); assert_eq!(original_outside,fs::read(&outside).unwrap());
    assert!(probed.is_err() && !out.status.success(),"both probe and production render must reject a playlist that follows an outside segment: {}", String::from_utf8_lossy(&out.stderr));
}


#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn thirty_two_camera_targets_parse_and_match_isolated_boundary_samples() {
    let fixture = Fixture::new(160, 96);
    // Asymmetric detail makes zoom/pan observable against the identical source in both renders.
    run("ffmpeg", &["-v", "error", "-y", "-f", "lavfi", "-i",
        "testsrc=size=160x96:rate=30:duration=65", "-c:v", "libx264", "-preset", "ultrafast",
        "-qp", "0", "-pix_fmt", "yuv444p", fixture.0.join("screen.mp4").to_str().unwrap()]);
    let mut s = fixture.snapshot(160, 96);
    s.document.video.duration_us = 65_000_000;
    s.document.clips[0].out_us = 65_000_000;
    s.document.zooms = (0..32).map(|i| ZoomElement {
        mode: focusflow_core::project::ZoomMode::Fixed, id: format!("target-{i}"), clip_id: "first".into(), start_us: (1 + i * 2) * 1_000_000,
        end_us: (1 + i * 2) * 1_000_000 + 800_000, x: 0.35, y: 0.65,
        scale: 2.0, intro_us: 200_000, hold_us: 200_000, outro_us: 400_000,
    }).collect();
    let plan = render::compile(&s.document, ExportPreset::Balanced).unwrap();
    assert!(plan.camera.len() >= 160);
    let output = render(&s, ExportPreset::Balanced, false);
    check(&output, 30, 65.0, false, 160, 96);
    let many = fixture.0.join("many-targets.mp4");
    fs::rename(output, &many).unwrap();
    s.document.zooms = vec![s.document.zooms.last().unwrap().clone()];
    let isolated = render(&s, ExportPreset::Balanced, false);
    // Include frames just before/at/after intro, active, hold and outro boundaries.
    for n in [1889, 1890, 1891, 1893, 1895, 1896, 1897, 1913, 1914, 1915, 1919, 1920, 1921, 1926, 1931, 1932, 1933] {
        let a = frame(&many, n as f64 / 30.0);
        let b = frame(&isolated, n as f64 / 30.0);
        assert_eq!(a.len(), 160 * 96 * 3);
        assert_eq!(a.len(), b.len());
        let mean = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y) as f64).sum::<f64>() / a.len() as f64;
        assert!(mean < 3.0, "camera boundary frame {n}: mean pixel difference {mean}");
    }
}

#[test]
fn accepted_click_boundary_builds_every_ring_in_a_bounded_graph() {
    let dir = std::env::temp_dir().join(format!("focusflow-graph-budget-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&dir).unwrap();
    let fixture = Fixture(dir);
    fs::write(fixture.0.join("screen.mp4"), b"path-only fixture; not decoded").unwrap();
    let mut s = fixture.snapshot(160, 96);
    s.document.clips = (0..64).map(|i| Clip { id: format!("repeat-{i}"), in_us: 0, out_us: 1_000_000 }).collect();
    s.document.clicks = (0..128).rev().map(|i| Click { at_us: i * 1000, x: 0.25, y: 0.75 }).collect();
    let plan = render::compile(&s.document, ExportPreset::Balanced).unwrap();
    let graph = render::filter_graph(&s, &plan, false).unwrap();
    assert_eq!(graph.matches("overlay=x=").count(), 8192);
    assert!(graph.len() <= 32 * 1024 * 1024);
    assert!(graph.contains("[click63_127]zoompan="));
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn frame_style_composes_padding_gradient_corners_shadow_and_aspects() {
    let fixture=Fixture::new(160,96); let mut s=fixture.snapshot(160,96);
    s.document.schema_version=2;
    s.document.frame_style=FrameStyle{padding_px:16,corner_radius_px:16,
        background:FrameBackground::Solid{color:"#ffffff".into()},..Default::default()};
    let out=render(&s,ExportPreset::Balanced,false); check(&out,30,3.0,false,160,96);
    let data=frame(&out,0.2);
    let bg=pixel(&data,160,3,3); assert!(bg.iter().all(|v|*v>235),"padding is background: {bg:?}");
    let center=pixel(&data,160,80,48); assert!(center.iter().all(|v|*v<20),"content retained: {center:?}");
    let geo=focusflow_core::frame::geometry(&s.document.frame_style,160,96).unwrap();
    let corner=pixel(&data,160,geo.content_x as usize,geo.content_y as usize);
    assert!(corner.iter().all(|v|*v>220),"rounded corner must reveal background: {corner:?}");
    s.document.frame_style.shadow=true;
    let out=render(&s,ExportPreset::Balanced,false); let shadow=frame(&out,0.2);
    let sy=(geo.content_y+geo.content_height+5) as usize;
    let plain=pixel(&data,160,80,sy); let shaded=pixel(&shadow,160,80,sy);
    assert!(plain[0]>235 && shaded[0]+15<plain[0],"soft shadow: plain={plain:?}, shadow={shaded:?}");
    for (canvas,w,h) in [(CanvasFormat::Landscape,160,90),(CanvasFormat::Portrait,90,160),(CanvasFormat::Square,160,160)] {
        s.document.frame_style=FrameStyle{canvas,padding_px:12,background:FrameBackground::Gradient{start:"#ff0000".into(),end:"#0000ff".into()},..Default::default()};
        let out=render(&s,ExportPreset::Balanced,false); check(&out,30,3.0,false,w,h);
        let data=frame(&out,0.2); let top=pixel(&data,w as usize,2,2); let bottom=pixel(&data,w as usize,w as usize-3,h as usize-3);
        assert!(top[0]>220 && top[2]<35 && bottom[2]>220 && bottom[0]<35,"diagonal gradient: {top:?}/{bottom:?}");
        let center=pixel(&data,w as usize,w as usize/2,h as usize/2); assert!(center.iter().all(|v|*v<20),"square corners must preserve opaque content: {center:?}");
        let g=focusflow_core::frame::geometry(&s.document.frame_style,160,96).unwrap();
        assert!((g.content_width as f64/g.content_height as f64-160.0/96.0).abs()<0.06);
    }
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn identity_style_keeps_schema_one_pixels_and_graph_unchanged() {
    let fixture=Fixture::new(161,97); let mut s=fixture.snapshot(161,97);
    let old_plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();
    let old_graph=render::filter_graph(&s,&old_plan,false).unwrap();
    let old_out=render(&s,ExportPreset::Balanced,false); let old_pixels=frame(&old_out,1.2);
    s.document.schema_version=2;
    let new_plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();
    assert_eq!(old_graph,render::filter_graph(&s,&new_plan,false).unwrap());
    let new_out=render(&s,ExportPreset::Balanced,false);
    assert_eq!(old_pixels,frame(&new_out,1.2)); check(&new_out,30,3.0,false,160,96);
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn styled_camera_clicks_and_audio_share_capture_space() {
    let fixture=Fixture::new(160,96); fixture.audio(3.0,0.0); let mut s=fixture.snapshot(160,96);
    s.document.schema_version=2; s.document.microphone=Some(AudioAsset{path:"mic.wav".into()});
    s.document.frame_style=FrameStyle{canvas:CanvasFormat::Square,padding_px:16,corner_radius_px:12,shadow:true,
        background:FrameBackground::Solid{color:"#ffffff".into()}};
    s.document.zooms=vec![ZoomElement{mode:ZoomMode::Fixed,id:"zoom".into(),clip_id:"first".into(),start_us:0,end_us:1_000_000,x:0.5,y:0.5,scale:2.0,intro_us:200_000,hold_us:0,outro_us:0}];
    s.document.clicks=vec![Click{at_us:200_000,x:0.35,y:0.5}];
    let before=fs::read(fixture.0.join("screen.mp4")).unwrap(); let audio=fs::read(fixture.0.join("mic.wav")).unwrap();
    let out=render(&s,ExportPreset::HighQuality,true); check(&out,60,3.0,true,160,160);
    let data=frame(&out,0.233334); let center=pixel(&data,160,42,80); let edge=pixel(&data,160,62,80); let outside=pixel(&data,160,70,80);
    assert!(center.iter().all(|v|*v<30) && edge.iter().all(|v|*v>100) && outside.iter().all(|v|*v<30),"scaled capture ring center={center:?} edge={edge:?} outside={outside:?}");
    assert!(loudness(&out,0.1,0.1)>0.05); assert_eq!(fs::read(fixture.0.join("screen.mp4")).unwrap(),before); assert_eq!(fs::read(fixture.0.join("mic.wav")).unwrap(),audio);
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn preview_video_and_still_use_requested_sample_window() {
    use focusflow_core::preview::{PreviewRequest,PreviewSelection};
    let f=Fixture::new(160,96);let s=f.snapshot(160,96);
    let plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();
    // Explicit selection keeps this regression independent from selection API implementation.
    let sel=PreviewSelection{first_frame:30,frame_count:3,fps:30,start_us:1_000_000,duration_us:100_000};
    fs::write(f.0.join(ffmpeg::FILTER_SCRIPT),render::preview_filter_graph(&s,&plan,&sel,false).unwrap()).unwrap();
    let args=ffmpeg::preview_args(&s,&plan,&sel,&f.0,false,false).unwrap();
    let out=Command::new("ffmpeg").args(args).output().unwrap();assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));
    let p=probe(&f.0.join(ffmpeg::RENDER_OUTPUT));
    assert_eq!(p["streams"][0]["nb_frames"],"3");
    assert!(pixel(&frame(&f.0.join(ffmpeg::RENDER_OUTPUT),0.0),160,5,5)[2]>180);
    let sel=focusflow_core::preview::select(&plan,&PreviewRequest::Still{at_us:1_000_001}).unwrap();
    fs::write(f.0.join(ffmpeg::FILTER_SCRIPT),render::preview_filter_graph(&s,&plan,&sel,false).unwrap()).unwrap();
    let out=Command::new("ffmpeg").args(ffmpeg::preview_args(&s,&plan,&sel,&f.0,false,true).unwrap()).output().unwrap();
    assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));
    assert!(f.0.join("output.partial.png").is_file());
}

// Keep the production inputs and graph; select a lossless raw sink to compare pre-encode pixels/samples.
fn raw_render(s: &RenderSnapshot, selection: Option<&focusflow_core::preview::PreviewSelection>, audio: bool, size: (u32,u32)) -> Vec<u8> {
    let plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();
    let (mut graph,mut args)=if let Some(sel)=selection {
        (render::preview_filter_graph(s,&plan,sel,audio).unwrap(),ffmpeg::preview_args(s,&plan,sel,&s.project_dir,audio,false).unwrap())
    } else {
        (render::filter_graph(s,&plan,audio).unwrap(),ffmpeg::render_args(s,&plan,&s.project_dir,audio).unwrap())
    };
    if selection.is_none() {
        graph=graph.replace("[v]","[full]"); graph.push_str(&format!(";\n[full]scale={}:{},setsar=1[v]",size.0,size.1));
    }
    fs::write(s.project_dir.join(ffmpeg::FILTER_SCRIPT),graph).unwrap();
    args.truncate(args.iter().position(|a| a=="-map").unwrap());
    for arg in &mut args { if arg == "pipe:1" { *arg = "pipe:2".into(); } }
    if audio {
        args.extend(["-map","[v]","-f","null","/dev/null","-map","[a]","-c:a","pcm_s16le","-f","s16le","-"].map(Into::into));
    } else {
        args.extend(["-map","[v]","-c:v","rawvideo","-threads","1","-pix_fmt","rgb24","-f","rawvideo","-"].map(Into::into));
    }
    let out=Command::new("ffmpeg").args(args).output().unwrap();
    assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr)); out.stdout
}
fn same_bytes(actual: &[u8], expected: &[u8], context: &str) {
    assert_eq!(actual.len(),expected.len(),"{context}: byte count");
    assert!(actual==expected,"{context}: first different byte {:?}",actual.iter().zip(expected).position(|(a,b)| a!=b));
}
#[test]
#[ignore = "requires FFmpeg; generated fixtures only"]
fn preview_preserves_fractional_cuts_camera_clicks_style_and_still_pixels() {
    use focusflow_core::preview::{self,PreviewRequest};
    let f=Fixture::new(160,96);let mut s=f.snapshot(160,96);
    s.document.schema_version=2;
    s.document.frame_style=FrameStyle { canvas:CanvasFormat::Square, background:FrameBackground::Gradient{start:"#112233".into(),end:"#987654".into()},padding_px:12,corner_radius_px:9,shadow:true };
    s.document.clips=vec![Clip{id:"one".into(),in_us:123_457,out_us:900_009},Clip{id:"tiny".into(),in_us:1_000_000,out_us:1_000_013},Clip{id:"two".into(),in_us:2_000_003,out_us:2_900_017},Clip{id:"last".into(),in_us:1_000_007,out_us:1_300_019}];
    s.document.zooms=vec![ZoomElement{mode:ZoomMode::Fixed,id:"z".into(),clip_id:"one".into(),start_us:150_000,end_us:750_000,x:0.4,y:0.55,scale:2.5,intro_us:500_000,hold_us:0,outro_us:100_000}];
    s.document.clicks=vec![Click{at_us:200_000,x:0.4,y:0.5},Click{at_us:800_000,x:0.5,y:0.6},Click{at_us:2_050_000,x:0.5,y:0.5}];
    let plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();let size=(160,160);let frame_size=(size.0*size.1*3) as usize;
    let full=raw_render(&s,None,false,size);
    for (start,end) in [(133_334,500_001),(700_001,1_000_001),(1_600_001,plan.duration_us)] {
        let sel=preview::select(&plan,&PreviewRequest::Video{start_us:start,end_us:end}).unwrap();
        let bytes=raw_render(&s,Some(&sel),false,size);
        same_bytes(&bytes,&full[sel.first_frame as usize*frame_size..(sel.first_frame+sel.frame_count) as usize*frame_size],&format!("range {start}..{end}"));
    }
    let sel=preview::select(&plan,&PreviewRequest::Still{at_us:233_334}).unwrap();
    fs::write(f.0.join(ffmpeg::FILTER_SCRIPT),render::preview_filter_graph(&s,&plan,&sel,false).unwrap()).unwrap();
    let out=Command::new("ffmpeg").args(ffmpeg::preview_args(&s,&plan,&sel,&f.0,false,true).unwrap()).output().unwrap();
    assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));
    let png=frame(&f.0.join("output.partial.png"),0.0);
    same_bytes(&png,&full[sel.first_frame as usize*frame_size..(sel.first_frame+1) as usize*frame_size],"still PNG documented frame");
}
#[test]
#[ignore = "requires FFmpeg; generated fixtures only"]
fn preview_sparse_vfr_predecessor_and_shifted_timestamps_match_export() {
    use focusflow_core::preview::{self,PreviewRequest};
    let f=Fixture::new(64,48);
    for shifted in [false,true] {
        let filter=if shifted {"select='eq(n,0)+eq(n,30)+eq(n,150)+eq(n,210)',setpts=PTS+100/TB"} else {"select='eq(n,0)+eq(n,30)+eq(n,150)+eq(n,210)'"};
        run("ffmpeg",&["-v","error","-y","-f","lavfi","-i","testsrc2=size=64x48:rate=60:duration=4","-vf",filter,"-fps_mode","vfr","-c:v","libx264","-g","240","-pix_fmt","yuv420p",f.0.join("screen.mp4").to_str().unwrap()]);
        let mut s=f.snapshot(64,48);s.document.video.duration_us=3_516_667;s.document.clips[0].out_us=3_516_667;
        let plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();let full=raw_render(&s,None,false,(64,48));let frame_size=64*48*3;
        for (start,end) in [(1_566_666,2_233_334),(3_466_667,3_516_667)] {
            let sel=preview::select(&plan,&PreviewRequest::Video{start_us:start,end_us:end}).unwrap();
            same_bytes(&raw_render(&s,Some(&sel),false,(64,48)),&full[sel.first_frame as usize*frame_size..(sel.first_frame+sel.frame_count) as usize*frame_size],&format!("sparse VFR shifted={shifted} at {start}"));
        }
    }
}
#[test]
#[ignore = "requires FFmpeg; generated fixtures only"]
fn preview_native_audio_intersections_offsets_silence_and_eof_match_export() {
    use focusflow_core::preview::{self,PreviewRequest};
    let f=Fixture::new(64,48);let mut s=f.snapshot(64,48);
    s.document.microphone=Some(AudioAsset{path:"mic.wav".into()});
    s.document.clips=vec![Clip{id:"a".into(),in_us:123_457,out_us:700_013},Clip{id:"tiny".into(),in_us:1,out_us:14},Clip{id:"b".into(),in_us:1_200_017,out_us:2_000_019},Clip{id:"c".into(),in_us:0,out_us:400_003}];
    for rate in [44_100,48_000,11_025] {
        let mut wav=hound::WavWriter::create(f.0.join("mic.wav"),hound::WavSpec{channels:2,sample_rate:rate,bits_per_sample:16,sample_format:hound::SampleFormat::Int}).unwrap();
        for n in 0..rate {wav.write_sample((n%30000) as i16).unwrap();wav.write_sample(-((n%30000) as i16)).unwrap();}wav.finalize().unwrap();
        for offset in [-1_500_000,-123_457,0,123_457,1_500_000] {
            s.document.timing.audio_offset_us=Some(offset);
            let plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();let full=if offset == -1_500_000 { vec![0; (plan.duration_us*rate as u64).div_ceil(1_000_000) as usize*4] } else { raw_render(&s,None,true,(64,48)) };
            for (start,end) in [(0,200_001),(533_334,800_001),(1_133_334,plan.duration_us)] {
                let sel=preview::select(&plan,&PreviewRequest::Video{start_us:start,end_us:end}).unwrap();
                let a=(sel.first_frame*rate as u64).div_ceil(30) as usize*4;
                let b=((sel.first_frame+sel.frame_count)*rate as u64).div_ceil(30).min((plan.duration_us*rate as u64).div_ceil(1_000_000)) as usize*4;
                same_bytes(&raw_render(&s,Some(&sel),true,(64,48)),&full[a..b],&format!("rate={rate} offset={offset} start={start}"));
            }
        }
    }
}
#[test]
#[ignore = "requires FFmpeg; generated fixtures only"]
fn preview_downscales_after_complete_style_composition() {
    use focusflow_core::preview::{self,PreviewRequest};
    let f=Fixture::new(1000,600);let mut s=f.snapshot(1000,600);s.document.schema_version=2;
    s.document.clips[0].out_us=100_000;
    s.document.frame_style=FrameStyle{canvas:CanvasFormat::Portrait,background:FrameBackground::Solid{color:"#abcdef".into()},padding_px:20,corner_radius_px:15,shadow:true};
    let g=focusflow_core::frame::geometry(&s.document.frame_style,1000,600).unwrap();let size=preview::dimensions(g.width,g.height);
    assert_eq!(size,(538,960));
    let plan=render::compile(&s.document,ExportPreset::Balanced).unwrap();let sel=preview::select(&plan,&PreviewRequest::Video{start_us:0,end_us:100_000}).unwrap();
    same_bytes(&raw_render(&s,Some(&sel),false,size),&raw_render(&s,None,false,size),"final composed downscale");
}

// These full-source imports catch rounded Duration text dropping the last click or adding an export frame.
fn precise_endpoint_import_render(frames: u64, duration_us: u64, counts: [u64; 2]) {
    use focusflow_core::{legacy::LegacyDefaults, store::ProjectStore};
    let root = Fixture(std::env::temp_dir().join(format!("focusflow-precise-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))));
    let dir = root.0.join("recording");
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("screen.mp4");
    run("ffmpeg", &["-v", "error", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=30", "-frames:v", &frames.to_string(), "-c:v", "libx264", "-threads", "1", "-preset", "veryfast", "-bf", "2", "-g", "60", "-pix_fmt", "yuv420p", source.to_str().unwrap()]);
    fs::write(dir.join("clicks.json"), b"[{\"timestamp\":3.032,\"x\":80,\"y\":45,\"button\":\"left\"}]").unwrap();
    fs::write(dir.join("timeline.json"), b"[]").unwrap();
    let before: Vec<_> = ["screen.mp4", "clicks.json", "timeline.json"].into_iter().map(|p| (p, fs::read(dir.join(p)).unwrap())).collect();
    #[cfg(unix)]
    println!("source {frames} frames SHA256: {}", String::from_utf8(run("sha256sum", &[source.to_str().unwrap()])).unwrap().trim());
    let defaults = LegacyDefaults { scale: 2.0, intro_us: 200_000, hold_us: 300_000, outro_us: 200_000, pan_transition_us: 200_000, settle_us: 100_000 };
    let store = ProjectStore::new(&root.0).unwrap();
    let doc = store.open_or_import("recording", Path::new("ffmpeg"), defaults).unwrap();
    assert_eq!(doc.video.duration_us, duration_us, "{frames}-frame source endpoint");
    assert_eq!((doc.clips[0].in_us, doc.clips[0].out_us), (0, duration_us));
    assert_eq!(doc.clicks.len(), 1);
    assert_eq!(doc.clicks[0].at_us, 3_032_000);
    assert_eq!(doc.timing.provenance, TimingProvenance::LegacyUnknown);
    assert_eq!(doc.timing.audio_offset_us, None);
    assert_eq!(store.open_or_import("recording", Path::new("absent-ffmpeg"), defaults).unwrap(), doc);
    let project_bytes = fs::read(dir.join("project.json")).unwrap();
    let snapshot = store.snapshot("recording", doc.revision).unwrap();
    for (preset, count) in [ExportPreset::Balanced, ExportPreset::HighQuality].into_iter().zip(counts) {
        let out = render(&snapshot, preset, false);
        let bytes = run("ffprobe", &["-v", "error", "-count_frames", "-select_streams", "v:0", "-show_entries", "stream=nb_frames,nb_read_frames", "-of", "json", out.to_str().unwrap()]);
        let info: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for field in ["nb_frames", "nb_read_frames"] { assert_eq!(info["streams"][0][field], count.to_string(), "{frames} source frames at {} fps: {info}", preset.fps()); }
        assert_eq!(ffmpeg::probe_video(Path::new("ffmpeg"), &out).unwrap().duration_us, duration_us);
        println!("precise import {frames}frames duration={duration_us}us click=3032000us; {}fps output={count} decoded frames, no extra trailing frame", preset.fps());
    }
    for (name, bytes) in before { assert_eq!(fs::read(dir.join(name)).unwrap(), bytes, "source {name} changed"); }
    assert_eq!(fs::read(dir.join("project.json")).unwrap(), project_bytes);
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn precise_91_frame_import_retains_tail_click_and_exports_91_182_frames() {
    precise_endpoint_import_render(91, 3_033_333, [91, 182]);
}

#[test]
#[ignore = "requires FFmpeg and ffprobe; generated fixtures only"]
fn precise_92_frame_import_exports_92_184_frames() {
    precise_endpoint_import_render(92, 3_066_666, [92, 184]);
}
