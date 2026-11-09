fn main(){
    let args:Vec<_>=std::env::args_os().skip(1).collect();
    if let Some(code)=focusflow_windows_audio_worker::worker::dispatch(args.clone().into_iter()){std::process::exit(code);}
    if !args.is_empty(){eprintln!("Launch without arguments; select a device interactively.");std::process::exit(64);}
    #[cfg(windows)] let code=match windows_main(){Ok(code)=>code,Err(error)=>{eprintln!("Failed: {error}");71}};
    #[cfg(not(windows))] let code={eprintln!("Unavailable: this device test requires Windows.");69};
    std::process::exit(code);
}

#[cfg(windows)]
fn windows_main()->Result<i32,String>{
    use focusflow_windows_device_test::{analysis,collect::{self,Progress},selected_index,write_reference};
    use std::{io::{self,Write},sync::{mpsc,Arc,atomic::{AtomicBool,Ordering}},time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
    fn line()->Result<String,String>{let mut value=String::new();io::stdin().read_line(&mut value).map_err(|e|e.to_string())?;Ok(value)}
    fn save(path:&std::path::Path,value:&serde_json::Value)->Result<(),String>{let mut f=std::fs::OpenOptions::new().create_new(true).write(true).open(path).map_err(|e|e.to_string())?;f.write_all(&serde_json::to_vec_pretty(value).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;f.sync_all().map_err(|e|e.to_string())}
    let exe=std::env::current_exe().map_err(|e|e.to_string())?;
    let parent=exe.parent().ok_or("Executable has no parent directory")?;
    let root=parent.join("results");std::fs::create_dir_all(&root).map_err(|e|e.to_string())?;
    let stamp=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|e.to_string())?.as_millis();
    let output=root.join(format!("run-{stamp}-{}",std::process::id()));std::fs::create_dir(&output).map_err(|e|e.to_string())?;
    let source:serde_json::Value=serde_json::from_str(include_str!("../source-bindings.json")).map_err(|e|e.to_string())?;
    println!("FocusFlow local loopback test (not an app release). Results: {}",output.display());
    println!("No microphone, routing/settings changes, network or upload. Other sounds on the selected output may be recorded.");
    let devices=match focusflow_windows_audio_worker::native::list_output_devices(){Ok(devices)=>devices,Err(error)=>{save(&output.join("result.json"),&serde_json::json!({"status":"Failed","reason":error,"source":source}))?;return Err(error);}};
    if devices.is_empty(){save(&output.join("result.json"),&serde_json::json!({"status":"Unavailable","reason":"No active Windows render outputs","source":source}))?;println!("Unavailable: no active Windows outputs.");return Ok(69);}
    for(index,device)in devices.iter().enumerate(){println!("{}. {}\n   {}",index+1,device.display_name,device.id);}
    print!("Select one existing output number (blank cancels): ");io::stdout().flush().map_err(|e|e.to_string())?;
    let input=line()?;if input.trim().is_empty(){return Ok(0);}let selected=&devices[selected_index(&input,devices.len())?];
    let reference=output.join("reference.wav");write_reference(&reference)?;
    println!("Reference WAV (12 seconds): {}",reference.display());
    println!("Prepare your normal audio player to play that file once on the selected existing output, at a comfortable volume. Do not change Windows device/mixer settings for this test. Close unrelated sounds.");
    println!("When CAPTURING appears, play the file within 20 seconds. Recording ends automatically after 40 seconds. Playback route is not independently verified.");
    print!("Type START to enable this one system-only capture, or anything else to cancel: ");io::stdout().flush().map_err(|e|e.to_string())?;
    if line()?.trim()!="START"{return Ok(0);}
    let id=selected.id.clone();// OwnedTrack::begin_native_mode creates this absent child with create_dir; only its parent exists.
    let capture_dir=output.join("capture");let worker_dir=capture_dir.clone();let (tx,rx)=mpsc::channel();let cancel=Arc::new(AtomicBool::new(false));let controller_cancel=cancel.clone();let started=Instant::now();
    let thread=std::thread::Builder::new().name("loopback-test-controller".into()).spawn(move||{
        let result=std::panic::catch_unwind(||collect::run(exe,id,worker_dir,started,&tx,&controller_cancel));
        let result=result.unwrap_or_else(|_|collect::unconfirmed("Controller panicked; exact child cleanup is unconfirmed".into()));let _=tx.send(Progress::Finished(result));
    }).map_err(|e|e.to_string())?;
    let mut ready=false;
    let collection=loop{
        let limit=if ready{Duration::from_secs(180)}else{Duration::from_secs(8)};
        let remaining=limit.saturating_sub(started.elapsed());
        match rx.recv_timeout(remaining){
            Ok(Progress::Ready)=>{ready=true;println!("CAPTURING: play reference.wav now. Keep this window open; collecting for 40 seconds, then finalizing.");}
            Ok(Progress::Finished(result))=>break result,
            Err(_)=>{
                cancel.store(true,Ordering::Release);
                let deadline=Instant::now()+Duration::from_secs(2);
                let result=loop{match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())){Ok(Progress::Finished(r))=>break r,Ok(Progress::Ready)=>continue,Err(_)=>break collect::unconfirmed("Controller wait timed out; setup/child cleanup remains unconfirmed. Process exit closes owned handles but is not an observed worker exit.".into())}};
                break result;
            }
        }
    };
    if thread.is_finished(){let _=thread.join();} // Never claim a blocked setup thread was joined.
    let mut status="Failed";let mut analysis_result=serde_json::Value::Null;
    if collection.clean_exact_exit{
        if let Some(inventory)=&collection.inventory{
            match analysis::inspect_inventory(&capture_dir.join("source.partial.wav"),inventory){
                Ok(report)=>{status=report.status;analysis_result=serde_json::to_value(report).map_err(|e|e.to_string())?;}
                Err(error)=>analysis_result=serde_json::json!({"status":"Failed","reason":error}),
            }
        }
    }
    let result=serde_json::json!({"status":status,"scope":"Manual known-WAV source-content test only","selectedEndpoint":{"id":selected.id,"label":selected.display_name},"explicitSystemOptIn":true,"microphoneRequested":false,"playbackRoute":"User-selected; not independently verified","audioVideoTiming":"Unqualified; signature offset is not timing authority","collection":collection,"analysis":analysis_result,"source":source});
    save(&output.join("result.json"),&result)?;
    println!("{status}: {}",output.join("result.json").display());
    println!("Files stay local. PASS means clean owned capture and supported content signature only; not physical route or A/V qualification.");
    Ok(match status{"Pass"=>0,"Unavailable"=>69,_=>71})
}
