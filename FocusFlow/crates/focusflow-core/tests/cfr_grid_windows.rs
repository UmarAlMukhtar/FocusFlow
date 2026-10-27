//! Self-contained Windows executable fixture; reaches the public probe and native PeekNamedPipe.
#[cfg(not(windows))]
fn main() { println!("Windows CFR pipe runtime qualification: not applicable on this OS"); }
#[cfg(windows)]
fn main() { if let Err(error) = native::run() { eprintln!("{error}"); std::process::exit(1); } }
#[cfg(windows)]
mod native {
    use std::{ffi::c_void, fs, io::{self, Write}, path::{Path, PathBuf}, process::{Command, Stdio}, sync::{atomic::{AtomicBool, Ordering}, mpsc}, thread, time::{Duration, Instant}};
    use focusflow_core::{ffmpeg::probe_cfr_grid, ErrorCode};
    type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
    #[link(name="kernel32")]
    extern "system" {
        fn OpenProcess(access:u32, inherit:i32, pid:u32)->*mut c_void;
        fn CloseHandle(handle:*mut c_void)->i32;
        fn WaitForSingleObject(handle:*mut c_void, ms:u32)->u32;
        fn TerminateProcess(handle:*mut c_void, code:u32)->i32;
        fn GetStdHandle(which:u32)->*mut c_void;
    }
    struct Handle(*mut c_void);
    impl Handle {
        fn open(pid:u32)->Result<Self> {
            // Fixture publishes its PID while parked until this exact noninheritable handle opens.
            let h=unsafe{OpenProcess(0x00100000|0x0001|0x1000,0,pid)};
            if h.is_null(){return Err(io::Error::last_os_error().into());} Ok(Self(h))
        }
        fn signaled(&self)->bool { unsafe{WaitForSingleObject(self.0,0)==0} }
    }
    impl Drop for Handle {fn drop(&mut self){unsafe{CloseHandle(self.0);}}}
    const HEADER:&str="#format: frame checksums\n#version: 2\n#hash: adler32\n#software: Lavf62.3.100\n#tb 0: 1/15360\n#media_type 0: video\n#codec_id 0: h264\n#dimensions 0: 100x50\n#sar 0: 1/1\n#stream#, dts, pts, duration, size, hash\n";
    fn wait_file(path:&Path, timeout:Duration)->Result<String> {
        let end=Instant::now()+timeout;
        while Instant::now()<end { if let Ok(s)=fs::read_to_string(path) {if !s.is_empty(){return Ok(s)}} thread::sleep(Duration::from_millis(2)); }
        Err(format!("Fixture handshake timed out: {}",path.display()).into())
    }
    fn fixture(input:&Path,args:&[String])->Result<()> {
        let expected=["-v","error","-copyts","-protocol_whitelist","file,pipe","-f","mov","-enable_drefs","0","-use_absolute_path","0","-i"];
        assert_eq!(&args[..12],expected);
        assert_eq!(&args[13..],["-map","0","-c","copy","-f","framehash","-hash","adler32","pipe:1"]);
        let mode=input.file_name().unwrap().to_str().unwrap();
        fs::write(input.with_extension("ready"),std::process::id().to_string())?;
        wait_file(&input.with_extension("go"),Duration::from_secs(2))?;
        // A fixture cannot outlive its test-owned five-second escape even if the probe regresses.
        thread::spawn(||{thread::sleep(Duration::from_secs(5));std::process::exit(91);});
        match mode {
            "idle"|"cancel-idle"=>thread::sleep(Duration::from_secs(5)),
            "stderr"|"cancel-stderr"=>loop {io::stderr().write_all(&[b'x';4096])?;},
            "stdout"=>loop {io::stdout().write_all(&[b'x';4096])?;},
            "closed"=>{
                // Only this generated process's standard pipe handles are closed.
                unsafe{CloseHandle(GetStdHandle(-11i32 as u32));CloseHandle(GetStdHandle(-12i32 as u32));}
                thread::sleep(Duration::from_secs(5));
            },
            "valid"|"exit7"|"rows5000"|"hold-stdout"|"hold-stderr"=>{
                if mode=="exit7"{eprint!("fixture failure: ");}
                let flood=(mode=="rows5000"||mode=="exit7").then(||thread::spawn(||{for _ in 0..5000 {io::stderr().write_all(&[b'x';128]).unwrap();}}));
                let mut stdout=io::stdout().lock();stdout.write_all(HEADER.as_bytes())?;
                for n in 0..if mode=="rows5000"{5000}else{3} {writeln!(stdout,"0, {}, {}, 512, 123, abcdef01",n*512,n*512)?;}stdout.flush()?;drop(stdout);
                if let Some(t)=flood{t.join().unwrap();}
                if mode=="exit7" {eprint!("fixture failure");std::process::exit(7);}
                if mode.starts_with("hold-") {
                    let child=Command::new(std::env::current_exe()?).arg("--hold").arg(input)
                        .stdin(Stdio::null()).stdout(if mode=="hold-stdout"{Stdio::inherit()}else{Stdio::null()})
                        .stderr(if mode=="hold-stderr"{Stdio::inherit()}else{Stdio::null()}).spawn()?;
                    fs::write(input.with_extension("descendant"),child.id().to_string())?;
                    wait_file(&input.with_extension("descendant-open"),Duration::from_secs(2))?;
                }
            }, _=>return Err("Unknown generated fixture mode".into())
        } Ok(())
    }
    pub fn run()->Result<()> {
        let args:Vec<String>=std::env::args().skip(1).collect();
        if args.first().map(String::as_str)==Some("--hold") {thread::sleep(Duration::from_secs(5));return Ok(());}
        if let Some(i)=args.iter().position(|x|x=="-i") {return fixture(Path::new(&args[i+1]),&args);}
        let root=std::env::var_os("CORE_QUAL_EVIDENCE").map(PathBuf::from).ok_or("CORE_QUAL_EVIDENCE must name a fresh generated-only directory")?;
        fs::create_dir(&root)?; let exe=std::env::current_exe()?;
        for mode in ["valid","idle","stderr","stdout","closed","cancel-idle","cancel-stderr","exit7","hold-stdout","hold-stderr","rows5000"] {
            let input=root.join(mode);let cancelled=AtomicBool::new(false);let start=Instant::now();
            let (tx,rx)=mpsc::sync_channel(1);
            thread::scope(|scope| -> Result<()> {
                let cancel=&cancelled;let input2=input.clone();
                let observer=scope.spawn(move||->Result<serde_json::Value>{
                    let pid=wait_file(&input2.with_extension("ready"),Duration::from_secs(2))?.parse()?;
                    let child=Handle::open(pid)?;assert!(!child.signaled());fs::write(input2.with_extension("go"),b"observed")?;
                    let descendant=if mode.starts_with("hold-") {
                        let pid=wait_file(&input2.with_extension("descendant"),Duration::from_secs(2))?.parse()?;
                        let handle=Handle::open(pid)?;assert!(!handle.signaled());fs::write(input2.with_extension("descendant-open"),b"observed")?;Some((pid,handle))
                    }else{None};
                    if mode.starts_with("cancel-") {thread::sleep(Duration::from_millis(250).saturating_sub(start.elapsed()));cancel.store(true,Ordering::Relaxed);}
                    rx.recv_timeout(Duration::from_secs(12))?;
                    let signaled=child.signaled();let mut descendant_cleanup=None;
                    if let Some((pid,h))=descendant {let was_alive=!h.signaled();unsafe{TerminateProcess(h.0,92);WaitForSingleObject(h.0,2000);}assert!(h.signaled());descendant_cleanup=Some(serde_json::json!({"pid":pid,"aliveAfterProbe":was_alive,"signaledAfterOwnedCleanup":true}));}
                    assert!(signaled,"exact original fixture handle still live");
                    Ok(serde_json::json!({"pid":pid,"observedAlive":true,"signaledAfterProbe":signaled,"descendant":descendant_cleanup}))
                });
                let success=mode=="valid"||mode=="rows5000";
                let deadline=if success||mode=="exit7"||mode.starts_with("cancel-"){Duration::from_secs(10)}else{Duration::from_millis(250)};
                let n=if mode=="rows5000"{5000}else{3};
                let result=probe_cfr_grid(&exe,&input,n,&cancelled,start+deadline);
                let elapsed=start.elapsed();let _=tx.send(());let ownership=observer.join().map_err(|_|"observer panicked")??;
                let details=match &result {
                    Ok(grid)=>serde_json::json!({"frames":grid.completed_frames,"endpoint":grid.video.endpoint.ticks,"durationUs":grid.video.video.duration_us}),
                    Err(e)=>serde_json::to_value(e)?,
                };
                fs::write(input.with_extension("json"),serde_json::to_vec_pretty(&serde_json::json!({"mode":mode,"elapsedMs":elapsed.as_millis(),"ownership":ownership,"result":details}))?)?;
                if success {let grid=result?;assert_eq!(grid.completed_frames,n);assert_eq!(grid.video.endpoint.ticks,n*512);assert_eq!(grid.video.video.duration_us,n*1_000_000/30);}
                else {let error=result.expect_err("fault fixture was accepted");assert_eq!(error.code,if mode.starts_with("cancel-"){ErrorCode::Cancelled}else{ErrorCode::EncoderFailed});assert!(elapsed<Duration::from_secs(3));assert!(error.message.len()<=96*1024+512);if mode=="exit7"{assert!(error.message.contains("fixture failure"));assert!(error.message.len()>=96*1024,"fixture did not exercise diagnostic retention cap");}if mode.starts_with("hold-"){assert!(error.message.contains("timed out"));}}
                Ok(())
            })?;
            println!("PASS {mode}");
        } Ok(())
    }
}
