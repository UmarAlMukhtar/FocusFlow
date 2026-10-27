mod common;
use common::TestDir;
use focusflow_core::{ffmpeg::{probe_cfr_grid, PacketTime}, ErrorCode};
use std::{path::Path, process::Command, sync::atomic::AtomicBool, time::{Duration, Instant}};

#[cfg(target_os="linux")]
const HEADER: &str = "#format: frame checksums\n#version: 2\n#hash: adler32\n#software: Lavf62.3.100\n#tb 0: 1/15360\n#media_type 0: video\n#codec_id 0: h264\n#dimensions 0: 100x50\n#sar 0: 1/1\n#stream#, dts, pts, duration, size, hash\n";
#[cfg(target_os="linux")]
const ROWS: &str = "0, 0, 0, 512, 123, abcdef01\n0, 512, 512, 512, 45, 0123abcd\n0, 1024, 1024, 512, 67, 9876fedc\n";

#[test]
fn real_h264_grid_has_independently_known_count_and_endpoint_and_rejects_audio() {
    let dir = TestDir::new();
    let video = dir.path().join("screen.mp4");
    assert!(Command::new("ffmpeg").args(["-v","error","-f","lavfi","-i","color=c=blue:s=100x50:r=30","-frames:v","91","-c:v","libx264","-bf","0","-pix_fmt","yuv420p"]).arg(&video).status().unwrap().success());
    let before = std::fs::read(&video).unwrap();
    let saved = probe_cfr_grid(Path::new("ffmpeg"), &video, 91, &AtomicBool::new(false), Instant::now()+Duration::from_secs(5)).unwrap();
    assert_eq!((saved.completed_frames, saved.video.video.width, saved.video.video.height, saved.video.video.duration_us), (91,100,50,3_033_333));
    assert_eq!(saved.video.endpoint, PacketTime{ticks:46_592,numerator:1,denominator:15_360});
    assert_eq!(saved.video.first_packet_duration, PacketTime{ticks:512,numerator:1,denominator:15_360});
    assert_eq!(std::fs::read(&video).unwrap(), before);
    let audio = dir.path().join("audio.mp4");
    assert!(Command::new("ffmpeg").args(["-v","error","-i"]).arg(&video).args(["-f","lavfi","-i","anullsrc=r=48000:cl=mono","-map","0:v","-map","1:a","-c:v","copy","-c:a","aac","-t","3.033333"]).arg(&audio).status().unwrap().success());
    let error = probe_cfr_grid(Path::new("ffmpeg"), &audio, 91, &AtomicBool::new(false), Instant::now()+Duration::from_secs(5)).unwrap_err();
    assert_eq!(error.code, ErrorCode::EncoderFailed);
}

#[cfg(target_os="linux")]
mod linux {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, sync::atomic::Ordering};
    // Fixture executables are closed before spawning; serialize fixture execution with creation.
    static FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn tool(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fixture");
        fs::write(&path,format!("#!/usr/bin/python3\nimport os, sys, time\nfrom pathlib import Path\nPath(__file__ + '.pid').write_text(str(os.getpid()))\n{body}\n")).unwrap();
        fs::set_permissions(&path,fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn output_tool(dir: &Path, data: &[u8], after: &str) -> PathBuf {
        fs::write(dir.join("output"),data).unwrap();
        tool(dir,&format!("sys.stdout.buffer.write(Path(__file__).with_name('output').read_bytes())\nsys.stdout.flush()\n{after}"))
    }
    fn check(path: &Path, count: u64) -> focusflow_core::CoreResult<focusflow_core::ffmpeg::SavedCfrGrid> {
        probe_cfr_grid(path,Path::new("screen.mp4"),count,&AtomicBool::new(false),Instant::now()+Duration::from_secs(3))
    }
    fn reaped(path: &Path) {
        let pid = fs::read_to_string(path.with_extension("pid")).unwrap();
        assert!(!Path::new("/proc").join(pid.trim()).exists(),"original owned child still alive or unreaped");
    }
    #[test]
    fn literal_grid_and_all_stream_arguments() {
        let _lock=FIXTURES.lock().unwrap();let dir=TestDir::new();
        let path=output_tool(dir.path(),format!("{HEADER}{ROWS}").as_bytes(),"Path(__file__ + '.args').write_text('\\n'.join(sys.argv[1:]))");
        let saved=check(&path,3).unwrap();
        assert_eq!(saved.completed_frames,3);assert_eq!(saved.video.video.duration_us,100_000);
        assert_eq!(saved.video.endpoint,PacketTime{ticks:1536,numerator:1,denominator:15360});
        let args=fs::read_to_string(path.with_extension("args")).unwrap();
        assert_eq!(args.lines().collect::<Vec<_>>(),["-v","error","-copyts","-protocol_whitelist","file,pipe","-f","mov","-enable_drefs","0","-use_absolute_path","0","-i","screen.mp4","-map","0","-c","copy","-f","framehash","-hash","adler32","pipe:1"]);
        reaped(&path);
    }
    #[test]
    fn rejects_bad_interior_and_strict_grammar_without_first_tail_shortcut() {
        let _lock=FIXTURES.lock().unwrap();let dir=TestDir::new();let valid=format!("{HEADER}{ROWS}");
        let mut bad=vec![
            valid.replace("0, 512, 512, 512", "0, 513, 513, 512"),
            valid.replace("0, 512, 512, 512", "0, 512, 512, 511"),
            valid.replace("0, 512, 512, 512", "0, 512, 512, -1"),
            valid.replace("0, 512, 512, 512", "0, 512, 512, N/A"),
            valid.replace("0, 512, 512, 512", "0, 0, 512, 512"),
            valid.replace("0, 512, 512, 512", "0, 0, 0, 512"),
            valid.replace("0, 0, 0, 512", "0, 1, 1, 512"),
            valid.replace("0, 512, 512, 512", "1, 512, 512, 512"),
            valid.replace("#media_type 0: video", "#media_type 0: audio"),
            valid.replace("#codec_id 0: h264", "#codec_id 0: hevc"),
            valid.replace("#sar 0: 1/1", "#sar 0: 2/1"),
            valid.replace("#tb 0: 1/15360", "#tb 0: 1/15361"),
            valid.replace("#tb 0: 1/15360", "#tb 0: 1/2147483648"),
            valid.replace("#dimensions 0: 100x50", "#dimensions 0: 16385x50"),
            valid.replace("#version: 2\n", ""),
            format!("{valid}#version: 2\n"),
            format!("{valid}0, 1536, 1536, 512, 20, abcd0123\n"),
            format!("{HEADER}0, 0, 0, 512, 123, abcdef01\n"),
            valid.trim_end_matches('\n').to_owned(),
            format!("{}\n", "#".repeat(1025)),
            "#version: 2\n".repeat(65),
            valid.replace("abcdef01", "0xabcdef"),
            valid.replace("abcdef01", "abcdefgh"),
            valid.replace("0, 0, 0, 512, 123", "0, 0, 0, 512, 0"),
        ];
        for header in ["#media_type 1: audio\n", "#media_type 1: video\n", "#media_type 1: data\n", "#media_type 1: subtitle\n", "#version: 2\n"] {
            bad.push(valid.replace("#stream#", &format!("{header}#stream#")));
        }
        for number in ["-1","-0","00","+0","9223372036854775807","-9223372036854775808","9223372036854775808"] {
            bad.push(valid.replace("0, 0, 0, 512",&format!("0, {number}, {number}, 512")));
        }
        for data in bad.iter().map(|s| s.as_bytes()).chain([b"\xff\n".as_slice(),b"\0\n"]) {
            let path=output_tool(dir.path(),data,"");let error=check(&path,3).expect_err(&String::from_utf8_lossy(data));
            assert_eq!(error.code,ErrorCode::EncoderFailed);assert_eq!(error.field.as_deref(),Some("video"));reaped(&path);
        }
    }
    #[test]
    fn accepts_equivalent_rationals_unspecified_sar_and_crlf() {
        let _lock=FIXTURES.lock().unwrap();let dir=TestDir::new();
        let text=format!("{HEADER}{ROWS}").replace("1/15360","2/30720").replace("#sar 0: 1/1","#sar 0: 0/1").replace('\n',"\r\n");
        assert_eq!(check(&output_tool(dir.path(),text.as_bytes(),""),3).unwrap().video.video.duration_us,100_000);
    }
    #[test]
    fn streams_5000_rows_and_drains_diagnostics_beyond_retention_limit() {
        let _lock=FIXTURES.lock().unwrap();let dir=TestDir::new();
        let body=format!("sys.stdout.write({:?})\nfor n in range(5000):\n sys.stdout.write(f'0, {{n*512}}, {{n*512}}, 512, 123, abcdef01\\n')\n sys.stderr.write('x'*128)\nsys.stdout.flush()\nsys.stderr.flush()",HEADER);
        let path=tool(dir.path(),&body);let saved=check(&path,5000).unwrap();
        assert_eq!(saved.completed_frames,5000);assert_eq!(saved.video.video.duration_us,166_666_666);reaped(&path);
    }
    #[test]
    fn refuses_invalid_counts_expiry_and_prelaunch_cancel_without_spawning() {
        let _lock=FIXTURES.lock().unwrap();let dir=TestDir::new();let path=tool(dir.path(),"time.sleep(10)");
        for count in [0,2_592_001,u64::MAX] {assert_eq!(check(&path,count).unwrap_err().code,ErrorCode::EncoderFailed);}
        let error=probe_cfr_grid(&path,Path::new("screen.mp4"),3,&AtomicBool::new(false),Instant::now()).unwrap_err();assert_eq!(error.code,ErrorCode::EncoderFailed);
        let error=probe_cfr_grid(&path,Path::new("screen.mp4"),3,&AtomicBool::new(true),Instant::now()+Duration::from_secs(3)).unwrap_err();assert_eq!(error.code,ErrorCode::Cancelled);
        assert!(!path.with_extension("pid").exists());
    }
    #[test]
    fn idle_flood_and_closed_pipes_obey_deadline_and_reap_original_child() {
        let _lock=FIXTURES.lock().unwrap();let dir=TestDir::new();
        for body in ["time.sleep(20)","while True: os.write(2,b'x'*4096)","os.close(1)\nos.close(2)\ntime.sleep(20)","while True: os.write(1,b'x'*4096)"] {
            let path=tool(dir.path(),body);let started=Instant::now();
            let error=probe_cfr_grid(&path,Path::new("screen.mp4"),3,&AtomicBool::new(false),started+Duration::from_millis(250)).unwrap_err();
            assert_eq!(error.code,ErrorCode::EncoderFailed);assert!(started.elapsed()<Duration::from_secs(3));reaped(&path);
        }
    }
    #[test]
    fn cancellation_remains_responsive_during_idle_and_stderr_flood() {
        let _lock=FIXTURES.lock().unwrap();let dir=TestDir::new();
        for body in ["time.sleep(20)","while True: os.write(2,b'x'*4096)"] {
            let path=tool(dir.path(),body);let cancelled=AtomicBool::new(false);let started=Instant::now();
            std::thread::scope(|s| {
                s.spawn(|| {std::thread::sleep(Duration::from_millis(250));cancelled.store(true,Ordering::Relaxed);});
                assert_eq!(probe_cfr_grid(&path,Path::new("screen.mp4"),3,&cancelled,started+Duration::from_secs(10)).unwrap_err().code,ErrorCode::Cancelled);
            });
            assert!(started.elapsed()<Duration::from_secs(3));reaped(&path);
        }
    }
    #[test]
    fn complete_output_requires_successful_exit() {
        let _lock=FIXTURES.lock().unwrap();let dir=TestDir::new();let path=output_tool(dir.path(),format!("{HEADER}{ROWS}").as_bytes(),"sys.stderr.write('fixture failure')\nsys.exit(7)");
        let error=check(&path,3).unwrap_err();assert_eq!(error.code,ErrorCode::EncoderFailed);assert!(error.message.contains("fixture failure"));reaped(&path);
    }
    #[test]
    fn descendant_writer_eof_is_not_inferred_from_owned_child_exit() {
        let _lock=FIXTURES.lock().unwrap();let dir=TestDir::new();
        for pipe in [1,2] {
            let body=format!("pid=os.fork()\nif pid==0:\n os.close({})\n time.sleep(1.5)\n os._exit(0)\nPath(__file__ + '.descendant').write_text(str(pid))\nos._exit(0)",if pipe==1 {2} else {1});
            let path=output_tool(dir.path(),format!("{HEADER}{ROWS}").as_bytes(),&body);let start=Instant::now();
            let result=probe_cfr_grid(&path,Path::new("screen.mp4"),3,&AtomicBool::new(false),start+Duration::from_millis(250));
            assert!(start.elapsed()<Duration::from_secs(1));assert!(result.unwrap_err().message.contains("timed out"));reaped(&path);
        }
    }
}
