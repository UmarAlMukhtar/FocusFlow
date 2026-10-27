pub(crate) fn append_audio_encoding_args(args: &mut Vec<String>) {
    // Silence pads an ended microphone track so the video determines export length.
    args.extend(["-c:a", "aac", "-b:a", "128k", "-af", "apad", "-shortest"].map(str::to_owned));
}

#[cfg(test)]
mod tests {
    use super::append_audio_encoding_args;
    use std::{fs, path::Path, process::Command, time::SystemTime};

    fn run(program: &str, args: &[String]) -> String {
        let output = Command::new(program).args(args).output().expect(program);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).expect("UTF-8 command output")
    }

    fn probe(path: &Path, stream: &str, field: &str) -> String {
        run("ffprobe", &[
            "-v".into(), "error".into(), "-select_streams".into(), stream.into(),
            "-show_entries".into(), format!("stream={field}"), "-of".into(),
            "default=noprint_wrappers=1:nokey=1".into(), path.to_string_lossy().into_owned(),
        ]).trim().to_owned()
    }

    #[test]
    #[ignore = "requires FFmpeg and ffprobe on PATH; run with --ignored"]
    fn microphone_length_does_not_trim_or_extend_video() {
        let unique = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("focusflow-export-{}-{unique}", std::process::id()));
        fs::create_dir(&root).unwrap();

        for filtered in [false, true] {
            for mic_seconds in [None, Some("0.4"), Some("5")] {
                let output = root.join(format!("{filtered}-{}.mp4", mic_seconds.unwrap_or("none")));
                let mut args: Vec<String> = [
                    "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
                    "testsrc2=size=128x72:rate=30:duration=3",
                ].map(str::to_owned).into();
                if let Some(duration) = mic_seconds {
                    args.extend(["-f".into(), "lavfi".into(), "-i".into(),
                        format!("sine=frequency=440:sample_rate=48000:duration={duration}")]);
                }
                if filtered {
                    args.extend(["-filter_complex".into(),
                        "[0:v]zoompan=z=1:d=1:s=128x72:fps=30,setsar=1[v]".into(),
                        "-map".into(), "[v]".into()]);
                } else {
                    args.extend(["-map".into(), "0:v".into()]);
                }
                if mic_seconds.is_some() {
                    args.extend(["-map".into(), "1:a".into()]);
                    append_audio_encoding_args(&mut args);
                } else {
                    args.push("-an".into());
                }
                args.extend(["-c:v", "libx264", "-threads", "1", "-preset", "veryfast", "-pix_fmt", "yuv420p"].map(str::to_owned));
                args.push(output.to_string_lossy().into_owned());
                run("ffmpeg", &args);

                let video_seconds: f64 = probe(&output, "v:0", "duration").parse().unwrap();
                assert!((video_seconds - 3.0).abs() < 0.08,
                    "filtered={filtered}, mic={mic_seconds:?}: video duration {video_seconds}");
                let video_start: f64 = probe(&output, "v:0", "start_time").parse().unwrap();
                if mic_seconds.is_some() {
                    assert_eq!(probe(&output, "a:0", "codec_name"), "aac");
                    let audio_seconds: f64 = probe(&output, "a:0", "duration").parse().unwrap();
                    assert!((audio_seconds - 3.0).abs() < 0.08,
                        "filtered={filtered}, mic={mic_seconds:?}: audio duration {audio_seconds}");
                    let audio_start: f64 = probe(&output, "a:0", "start_time").parse().unwrap();
                    assert!((video_start - audio_start).abs() < 0.05,
                        "audio/video start timestamps differ: {audio_start}, {video_start}");
                    println!("filtered={filtered}, mic={mic_seconds:?}: video={video_seconds}s, audio={audio_seconds}s, start delta={}s", audio_start - video_start);
                } else {
                    assert!(probe(&output, "a:0", "codec_name").is_empty());
                    println!("filtered={filtered}, mic=none: video={video_seconds}s, no audio stream");
                }
            }
        }
        fs::remove_dir_all(&root).unwrap();
    }
}
