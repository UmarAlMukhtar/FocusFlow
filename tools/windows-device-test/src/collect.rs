//! Finite test coordinator around the promoted AudioSession; not a second capture implementation.
use focusflow_windows_audio_worker::{owned_child::{ExitObservation,TrackOutcome},protocol::{TrackKind,TrackRequest},session::AudioSession};
use std::{path::PathBuf,sync::{mpsc::Sender,Arc,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
#[derive(Debug)]
pub enum Progress { Ready, Finished(Collection) }
#[derive(Debug,serde::Serialize)]
pub struct Collection { pub clean_exact_exit:bool,pub direct_stop_attempted:Option<bool>,pub failure:Option<String>,pub outcome:serde_json::Value,pub frames:Option<u64>,pub inventory:Option<focusflow_windows_audio_worker::protocol::WavInventory>,pub control_elapsed_ms:u128 }
fn summarize(outcome:Option<&TrackOutcome>,stopped:bool,failure:Option<String>,started:Instant)->Collection{
    let clean=stopped&&failure.is_none()&&outcome.is_some_and(|o|o.exit==ExitObservation::Exited&&o.exit_code==Some(0)&&o.failure.is_none()&&o.terminal.is_some()&&o.usable.is_some());
    Collection{clean_exact_exit:clean,direct_stop_attempted:Some(stopped),failure,
        outcome:outcome.map(|o|serde_json::json!({"kind":format!("{:?}",o.kind),"exit":format!("{:?}",o.exit),"exitCode":o.exit_code,"failure":o.failure,"terminal":o.terminal,"producerValidatedInventory":o.usable})).unwrap_or(serde_json::Value::Null),
        inventory:outcome.and_then(|o|o.usable.clone()),frames:outcome.and_then(|o|o.usable.as_ref().map(|w|w.frames)),control_elapsed_ms:started.elapsed().as_millis()}
}
pub fn unconfirmed(reason:String)->Collection{Collection{clean_exact_exit:false,direct_stop_attempted:None,failure:Some(reason),outcome:serde_json::Value::Null,frames:None,inventory:None,control_elapsed_ms:0}}
pub fn run(exe:PathBuf,id:String,output:PathBuf,started:Instant,progress:&Sender<Progress>,cancel:&Arc<AtomicBool>)->Collection{
    let request=TrackRequest{kind:TrackKind::System,endpoint_id:id,explicitly_enabled:true,output_dir:output};
    let mut session=match AudioSession::begin(&exe,vec![request],started){Ok(s)=>s,Err(e)=>return summarize(None,false,Some(e),started)};
    let mut failure=None;let mut admitted=false;
    while started.elapsed()<Duration::from_secs(8){
        if cancel.load(Ordering::Acquire){failure=Some("Controller cancelled startup".into());break;}
        let poll=session.poll(Instant::now());
        if poll.failed||!session.outcomes().is_empty(){failure=Some("Requested system track failed before test admission".into());break;}
        if poll.ready{match session.video_admitted(){Ok(())=>admitted=true,Err(e)=>failure=Some(e)};break;}
        std::thread::sleep(Duration::from_millis(10));
    }
    if admitted{
        let _=progress.send(Progress::Ready);let end=Instant::now()+Duration::from_secs(40);
        while Instant::now()<end{
            if cancel.load(Ordering::Acquire){failure=Some("Controller cancelled collection".into());break;}
            session.poll(Instant::now());if !session.outcomes().is_empty(){failure=Some("System capture ended before the finite playback window completed".into());break;}
            std::thread::sleep(Duration::from_millis(10));
        }
    }else if failure.is_none(){failure=Some("System track was not Ready within the absolute eight-second startup budget".into());}
    let stopping=Instant::now();let errors=session.signal_all_stops(stopping);
    if !errors.is_empty(){failure=Some(format!("Direct Stop failed: {errors:?}"));}
    if failure.is_some(){session.cancel(stopping);}
    // Actual phase deadlines remain in AudioSession. The controller is finite even if no outcome arrives.
    let deadline=stopping+Duration::from_secs(126);
    loop{
        session.poll(Instant::now());
        if let Some(outcome)=session.outcomes().first(){return summarize(Some(outcome),true,failure,started);}
        if Instant::now()>=deadline{return summarize(None,true,Some("Cleanup deadline elapsed without a confirmed exact child outcome".into()),started);}
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]mod tests{
 use super::*;
 use focusflow_windows_audio_worker::{format::Encoding,protocol::{WavInventory,TerminalSummary}};
 fn outcome()->TrackOutcome{let inventory=WavInventory{frames:3,rate:48000,channels:2,bits:16,encoding:Encoding::Pcm,file_bytes:56};TrackOutcome{kind:TrackKind::System,terminal:Some(TerminalSummary{accepted_frames:3,written_complete_frames:3,synchronized_frames:None,usable:Some(inventory.clone()),retained_partial:true,cause:"Stop".into(),evidence_present:true}),exit:ExitObservation::Exited,exit_code:Some(0),usable:Some(inventory),failure:None}}
 #[test]fn pending_or_incomplete_child_cannot_pass_collection(){
  let o=outcome();assert!(summarize(Some(&o),true,None,Instant::now()).clean_exact_exit);
  assert!(!summarize(Some(&o),false,None,Instant::now()).clean_exact_exit);
  assert!(!summarize(Some(&o),true,Some("controller failed".into()),Instant::now()).clean_exact_exit);
  for field in 0..5{let mut o=outcome();match field{0=>o.exit=ExitObservation::CleanupPending,1=>o.exit_code=Some(1),2=>o.terminal=None,3=>o.usable=None,_=>o.failure=Some("failure".into())};assert!(!summarize(Some(&o),true,None,Instant::now()).clean_exact_exit);}
  let r=unconfirmed("blocked setup".into());assert!(!r.clean_exact_exit);assert_eq!(r.direct_stop_attempted,None);assert!(r.outcome.is_null());
 }
}
