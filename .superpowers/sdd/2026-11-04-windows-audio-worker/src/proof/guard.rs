//! Private positive guard. No approval exists in this source-only increment.
#[path="approval.rs"]
pub mod approval;
#[path="helper.rs"]
pub mod helper;
use crate::Result;
use std::path::{Path, PathBuf};
const KEYS: [&str; 7] = ["GITHUB_REPOSITORY", "GITHUB_REF", "GITHUB_WORKFLOW_REF", "GITHUB_WORKFLOW_SHA", "GITHUB_SHA", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"];
#[derive(Clone)]
pub struct ProofContext { hosted: [String; 3], tuple: [String; 7], measured: Vec<Identity>, executable: PathBuf }
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity { path: String, sha256: String, bytes: u64 }
struct Pin { path: &'static str, sha256: &'static str, bytes: u64 }
pub struct ApprovedProof { tuple: [&'static str; 7], files: &'static [Pin], route: &'static str, route_owned: bool }
pub struct ProofPermit { executable: PathBuf, route: String }
impl ProofPermit {
 pub(crate) fn executable(&self)->&Path {&self.executable}
 pub(super) fn route(&self)->&str {&self.route}
}
// No CLI/JSON/environment parser constructs ApprovedProof. Task 6 must review its trust root.
static APPROVED: Option<ApprovedProof> = None;
pub fn compiled_approval() -> Option<&'static ApprovedProof> { APPROVED.as_ref() }
fn check_hosted(context:&ProofContext, approved:&ApprovedProof)->Result<()> {
    if context.hosted != ["true", "github-hosted", "Windows"] { return Err("Private proof requires an explicitly approved GitHub-hosted Windows run".into()); }
    for (index,expected) in approved.tuple.iter().enumerate() {
        if expected.is_empty() || expected.trim()!=*expected || context.tuple[index]!=*expected { return Err(format!("Private proof denied: {}",KEYS[index])); }
    }
    let [repo,reference,workflow,workflow_sha,sha,run,attempt]=approved.tuple;
    if !repo.contains('/') || !reference.starts_with("refs/")
        || !workflow.starts_with(&format!("{repo}/.github/workflows/")) || !workflow.ends_with(&format!("@{reference}"))
        || !hex(workflow_sha,40) || !hex(sha,40)
        || [run,attempt].iter().any(|v|v.parse::<u64>().ok().is_none_or(|n|n==0)) {
        return Err("Private proof approval tuple is incomplete".into());
    }
    if !approved.route_owned || approved.route.is_empty() || approved.route.len()>2048 || approved.route.contains('\0') {
        return Err("No independently approved owned render route".into());
    }
    if !(3..=512).contains(&approved.files.len())
        || approved.files.iter().filter(|p|p.path=="@executable").count()!=1
        || !approved.files.iter().any(|p|p.path=="FIXTURE-SOURCE-MAP.json")
        || !approved.files.iter().any(|p|p.path.starts_with("src/")) {return Err("Approval requires executable, source and fixture pins".into());}
    let mut paths=std::collections::BTreeSet::new();
    for pin in approved.files {
        if pin.path.is_empty() || !hex(pin.sha256,64) || pin.bytes==0 || pin.bytes>512*1024*1024 || !paths.insert(pin.path)
            || (pin.path!="@executable" && (pin.path.contains('\\') || !Path::new(pin.path).components().all(|c|matches!(c,std::path::Component::Normal(_))))) {
            return Err("Invalid or duplicate immutable file pin".into());
        }
    }
    Ok(())
}
fn hex(value:&str,len:usize)->bool {value.len()==len&&value.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))}
/// Pure comparison. Runtime measured identities are produced only by the read-only fingerprint gate.
fn verify_hosted(context:&ProofContext, approved:&ApprovedProof)->Result<ProofPermit> {
    check_hosted(context,approved)?;
    if context.measured.len()!=approved.files.len() || !context.executable.is_absolute() {return Err("Missing measured file identities".into());}
    for (actual,pin) in context.measured.iter().zip(approved.files) {
        if actual.path!=pin.path || actual.sha256!=pin.sha256 || actual.bytes!=pin.bytes {return Err(format!("Actual file identity mismatch: {}",pin.path));}
    }
    Ok(ProofPermit { executable:context.executable.clone(),route:approved.route.into() })
}
fn authorize_with(mut context: ProofContext, approved:&ApprovedProof, mut fingerprint:impl FnMut(&str)->Result<Identity>)->Result<ProofPermit> {
    check_hosted(&context,approved)?;
    // Discard any supplied observations; every accepted identity comes from this measurement boundary.
    context.measured.clear();
    for pin in approved.files { context.measured.push(fingerprint(pin.path)?); }
    verify_hosted(&context,approved)
}
pub fn authorize_environment(source_root:&Path)->Result<ProofPermit> {
    let approved=compiled_approval().ok_or("Native-GO is absent; this source-only private proof is denied")?;
    let env=|key|std::env::var(key).unwrap_or_default();
    let mut context=ProofContext {hosted:[env("GITHUB_ACTIONS"),env("RUNNER_ENVIRONMENT"),env("RUNNER_OS")],tuple:KEYS.map(env),measured:Vec::new(),executable:PathBuf::new()};
    check_hosted(&context,approved)?;
    context.executable=std::env::current_exe().map_err(|e|e.to_string())?;
    let executable=context.executable.clone();
    authorize_with(context,approved,|name| {
        let path=if name=="@executable" {executable.clone()} else {source_root.join(name)};
        fingerprint(name,&path)
    })
}
#[cfg(not(windows))]
fn fingerprint(_name:&str,_path:&Path)->Result<Identity> {Err("Private proof fingerprinting requires Windows".into())}
#[cfg(windows)]
fn fingerprint(name:&str,path:&Path)->Result<Identity> {fingerprint_locked(name,path).map(|(identity,_)|identity)}
#[cfg(windows)]
fn fingerprint_locked(name:&str,path:&Path)->Result<(Identity,std::fs::File)> {cng::fingerprint(name,path)}

#[cfg(test)]
mod tests {
 use super::*;
 use std::cell::Cell;
 const H: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
 static PINS: [Pin;3] = [Pin{path:"@executable",sha256:H,bytes:10},Pin{path:"src/lib.rs",sha256:H,bytes:11},Pin{path:"FIXTURE-SOURCE-MAP.json",sha256:H,bytes:12}];
 pub(super) fn approved()->ApprovedProof { ApprovedProof { tuple:["owner/repo","refs/heads/proof","owner/repo/.github/workflows/proof.yml@refs/heads/proof","aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","123456","1"],files:&PINS,route:"owned-render-id",route_owned:true } }
 pub(super) fn context(a:&ApprovedProof)->ProofContext { ProofContext { hosted:["true".into(),"github-hosted".into(),"Windows".into()],tuple:a.tuple.map(String::from),measured:a.files.iter().map(identity).collect(),executable:PathBuf::from("/approved/private_audio_proof.exe") } }
 fn identity(p:&Pin)->Identity { Identity{path:p.path.into(),sha256:p.sha256.into(),bytes:p.bytes} }
 // Removing any one positive tuple or fingerprint comparison must fail this matrix.
 #[test] fn exact_tuple_and_actual_fingerprint_gate_precedes_owned_operations() {
  let a=approved(); let good=context(&a);
  assert!(verify_hosted(&good,&a).is_ok());
  for index in 0..7 { for value in ["", "mismatch"] { let mut c=good.clone(); c.tuple[index]=value.into(); let reads=Cell::new(0);let native=Cell::new(0);let result=authorize_with(c,&a,|_|{reads.set(reads.get()+1);Ok(identity(&PINS[0]))}).map(|_|native.set(native.get()+1));assert!(result.is_err(),"field {index}");assert_eq!(reads.get(),0);assert_eq!(native.get(),0); } }
  for index in 0..3 { for value in ["", "false", "self-hosted", "Linux"] {let mut c=good.clone();c.hosted[index]=value.into();let calls=Cell::new(0);assert!(authorize_with(c,&a,|_|{calls.set(1);Ok(identity(&PINS[0]))}).is_err());assert_eq!(calls.get(),0);} }
  for index in 0..3 {for mode in 0..4 {let mut c=good.clone();match mode {0=>c.measured[index].sha256.clear(),1=>c.measured[index].sha256="f".repeat(64),2=>c.measured[index].bytes+=1,_=>c.measured[index].path="substitute".into()};assert!(verify_hosted(&c,&a).is_err());}}
  let mut c=good.clone();c.measured.pop();assert!(verify_hosted(&c,&a).is_err());
  let mut c=good.clone();c.measured.push(identity(&PINS[0]));assert!(verify_hosted(&c,&a).is_err());
 }
 // Failing reads and measured mismatch must not invoke even injected native operations.
 #[test] fn measurement_failure_or_forged_context_cannot_mint_permit() {
  let a=approved();let good=context(&a);let native=Cell::new(0);
  assert!(authorize_with(good.clone(),&a,|_|Err("changed or unreadable actual file".into())).map(|_|native.set(1)).is_err());
  assert!(authorize_with(good.clone(),&a,|path|Ok(Identity{path:path.into(),sha256:"f".repeat(64),bytes:10})).map(|_|native.set(1)).is_err());assert_eq!(native.get(),0);
  let mut cursor=0;assert!(authorize_with(good,&a,|_|{let x=identity(&PINS[cursor]);cursor+=1;Ok(x)}).map(|_|native.set(1)).is_ok());assert_eq!(native.get(),1);assert_eq!(cursor,3);
 }
 #[test] fn approval_requires_all_pins_route_and_nonempty_independent_tuple() {
  let a=approved();let c=context(&a);
  for index in 0..7 {let mut bad=approved();bad.tuple[index]="";assert!(verify_hosted(&c,&bad).is_err());}
  let mut bad=approved();bad.route="";assert!(verify_hosted(&c,&bad).is_err());
  let mut bad=approved();bad.route_owned=false;assert!(verify_hosted(&c,&bad).is_err());
  let mut bad=approved();bad.files=&[];assert!(verify_hosted(&c,&bad).is_err());
 }
}

#[cfg(windows)]
mod cng {
 use super::*;
 use std::io::Read;
 use windows::{core::PCWSTR,Win32::Security::Cryptography::*};
 struct Algorithm(BCRYPT_ALG_HANDLE);
 impl Drop for Algorithm {fn drop(&mut self){unsafe {let _=BCryptCloseAlgorithmProvider(self.0,0);}}}
 struct Hash(BCRYPT_HASH_HANDLE);
 impl Drop for Hash {fn drop(&mut self){unsafe {let _=BCryptDestroyHash(self.0);}}}
 pub(super) fn fingerprint(name:&str,path:&Path)->Result<(Identity,std::fs::File)> {
  crate::wav::reject_links(path)?;
  // Hold a read-only non-write/non-delete-shared handle while measuring the exact file.
  use std::os::windows::fs::OpenOptionsExt;
  let mut file=std::fs::OpenOptions::new().read(true).share_mode(1).open(path).map_err(|e|e.to_string())?;
  let metadata=file.metadata().map_err(|e|e.to_string())?;
  if !metadata.is_file() || metadata.len()==0 || metadata.len()>512*1024*1024 {return Err("Fingerprint file is not a bounded nonempty regular file".into());}
  let digest=digest_reader(&mut file,metadata.len())?;
  Ok((Identity{path:name.into(),sha256:digest,bytes:metadata.len()},file))
 }
 pub(super) fn digest_bytes(bytes:&[u8])->Result<String> {digest_reader(&mut std::io::Cursor::new(bytes),bytes.len() as u64)}
 fn digest_reader(file:&mut impl Read,expected:u64)->Result<String> {
  // SAFETY: fresh CNG handles, system-owned hash object, fixed writable output and live input slices.
  // Hash drops before its algorithm on every return; no audio, COM, Job or endpoint is accessed here.
  unsafe {
   let mut raw=BCRYPT_ALG_HANDLE::default();BCryptOpenAlgorithmProvider(&mut raw,BCRYPT_SHA256_ALGORITHM,PCWSTR::null(),BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0)).ok().map_err(|e|e.to_string())?;
   let algorithm=Algorithm(raw);let mut raw_hash=BCRYPT_HASH_HANDLE::default();
   BCryptCreateHash(algorithm.0,&mut raw_hash,None,None,0).ok().map_err(|e|e.to_string())?;
   let hash=Hash(raw_hash);let mut buffer=[0u8;65536];let mut total=0u64;
   loop {let n=file.read(&mut buffer).map_err(|e|e.to_string())?;if n==0 {break;}total+=n as u64;if total>expected{return Err("Fingerprint file changed during read".into());}BCryptHashData(hash.0,&buffer[..n],0).ok().map_err(|e|e.to_string())?;}
   if total!=expected{return Err("Fingerprint file changed during read".into());}
   let mut digest=[0u8;32];BCryptFinishHash(hash.0,&mut digest,0).ok().map_err(|e|e.to_string())?;
   Ok(digest.iter().map(|v|format!("{v:02x}")).collect())
  }
 }
}

#[cfg(test)]
mod owned_order_tests {
 use super::*;
 use crate::{proof::{FixtureOps,ProbeError,ProofResult,run_fixture_with},session::AudioSession,owned_child::{OwnedTrack,ChildOps,SharedRegistry,PipeRead},protocol::{TrackRequest,ClockStamp},pending::ObservedExit};
 use std::{sync::{Arc,Mutex},time::Instant};
 struct Ops{trace:Arc<Mutex<Vec<&'static str>>>,available:bool}
 struct Child{trace:Arc<Mutex<Vec<&'static str>>>}
 impl ChildOps for Child {
  fn assign_job(&mut self)->Result<()> {self.trace.lock().unwrap().push("assign");Ok(())}
  fn send_init(&mut self,_:&[u8])->Result<()> {self.trace.lock().unwrap().push("init");Err("injected actual gate failure".into())}
  fn read_stdout(&mut self,_:&mut[u8])->Result<PipeRead>{Ok(PipeRead::Eof)}
  fn clock(&mut self)->Result<(Instant,ClockStamp)>{Ok((Instant::now(),ClockStamp{qpc:1,frequency:1000}))}
  fn observe_exit(&mut self)->Result<Option<ObservedExit>>{Ok(Some(ObservedExit{code:Some(71)}))}
  fn signal_stop(&mut self)->Result<()> {self.trace.lock().unwrap().push("stop");Ok(())}
  fn close_control(&mut self){self.trace.lock().unwrap().push("close");}
  fn force(&mut self)->Result<()> {self.trace.lock().unwrap().push("force");Ok(())}
 }
 impl FixtureOps for Ops {
  fn probe(&mut self,_:&ProofPermit,_:&str)->std::result::Result<(),ProbeError> {self.trace.lock().unwrap().push("probe");if self.available{Ok(())}else{Err(ProbeError::Unavailable("missing exact route".into()))}}
  fn capture(&mut self,_:&ProofPermit,request:TrackRequest,now:Instant)->Result<AudioSession>{self.trace.lock().unwrap().push("capture");let trace=self.trace.clone();AudioSession::begin_with(vec![request],now,SharedRegistry::new(),||Ok("11111111-1111-1111-1111-111111111111".into()),move |_|Ok(Box::new(Child{trace:trace.clone()})))}
  fn renderer(&mut self,_:&ProofPermit,_:TrackRequest,_:Instant)->Result<OwnedTrack>{self.trace.lock().unwrap().push("renderer");Err("must not render after capture gate failure".into())}
 }
 fn permit()->ProofPermit {ProofPermit{executable:"/approved/private.exe".into(),route:"owned-render-id".into()}}
 #[test] fn exact_route_probe_and_actual_session_gate_precede_renderer() {
  let trace=Arc::new(Mutex::new(Vec::new()));let mut ops=Ops{trace:trace.clone(),available:true};
  for index in 0..7 {for value in ["","mismatch"] {
   let approved=super::tests::approved();let mut context=super::tests::context(&approved);context.tuple[index]=value.into();
   assert!(verify_hosted(&context,&approved).map(|permit|run_fixture_with(permit,"owned-render-id",Path::new("/output"),&mut ops)).is_err());
   assert!(trace.lock().unwrap().is_empty());
  }}
  assert!(matches!(run_fixture_with(permit(),"other",Path::new("/output"),&mut ops),ProofResult::Unavailable{..}));assert!(trace.lock().unwrap().is_empty());
  ops.available=false;assert!(matches!(run_fixture_with(permit(),"owned-render-id",Path::new("/output"),&mut ops),ProofResult::Unavailable{..}));assert_eq!(*trace.lock().unwrap(),["probe"]);
  trace.lock().unwrap().clear();ops.available=true;let result=run_fixture_with(permit(),"owned-render-id",Path::new("/output"),&mut ops);assert!(matches!(result,ProofResult::Failed{..}));assert_eq!(*trace.lock().unwrap(),["probe","capture","assign","init","stop"]);
 }
}

#[cfg(test)]
mod collected_tests {
 use super::*;
 use crate::{proof::{FixtureOps,ProbeError,ProofResult,run_fixture_with},format::{Format,Encoding},session::AudioSession,owned_child::{OwnedTrack,ChildOps,SharedRegistry,PipeRead},protocol::{self,TrackRequest,ClockStamp,Event,EventV1,TerminalSummary,WavInventory,TrackKind},pending::ObservedExit};
 use std::{collections::VecDeque,time::Instant};
 const NONCE:&str="11111111-1111-1111-1111-111111111111";
 fn stamp()->ClockStamp{ClockStamp{qpc:1,frequency:1000}}
 struct Child {renderer:bool,frames:VecDeque<Vec<u8>>,ended:bool,trailing:&'static str}
 impl Child {
  fn push(&mut self,event:Event){self.frames.push_back(protocol::encode_event(&EventV1{version:1,nonce:NONCE.into(),kind:TrackKind::System,event}).unwrap());}
  fn end(&mut self,stop:bool){if self.ended{return;}self.ended=true;self.push(if stop{Event::StopAck{observed:stamp()}}else{Event::Ending{cause:"fixture complete".into(),observed:stamp()}});self.push(Event::CaptureReleased{at:stamp()});let inventory=if self.renderer{None}else{Some(WavInventory{frames:1,rate:48000,channels:2,bits:16,encoding:Encoding::Pcm,file_bytes:48})};self.push(Event::Terminal{outcome:TerminalSummary{accepted_frames:if self.renderer{0}else{1},written_complete_frames:if self.renderer{0}else{1},synchronized_frames:if self.renderer{None}else{Some(1)},usable:inventory,retained_partial:false,cause:if self.renderer{"fixture complete"}else{"parent stop"}.into(),evidence_present:!self.renderer}});
   match self.trailing {"malformed"=>self.frames.push_back(b"not a valid protocol frame\n".to_vec()),"truncated"=>self.frames.push_back(b"{\"version\":".to_vec()),"extra"=>self.push(Event::CaptureReleased{at:stamp()}),_=>()}
  }
 }
 impl ChildOps for Child {
  fn assign_job(&mut self)->Result<()> {Ok(())}
  fn send_init(&mut self,frame:&[u8])->Result<()> {protocol::decode_init_frame(frame,NONCE).map_err(|e|e.to_string())?;self.push(Event::Ready{format:Format{rate:48000,channels:2,bits:16,valid_bits:16,align:4,encoding:Encoding::Pcm}});if self.renderer{self.end(false);}Ok(())}
  fn read_stdout(&mut self,out:&mut[u8])->Result<PipeRead>{if let Some(mut bytes)=self.frames.pop_front(){let n=out.len().min(bytes.len());out[..n].copy_from_slice(&bytes[..n]);bytes.drain(..n);if !bytes.is_empty(){self.frames.push_front(bytes);}Ok(PipeRead::Data(n))}else if self.ended{Ok(PipeRead::Eof)}else{Ok(PipeRead::Empty)}}
  fn clock(&mut self)->Result<(Instant,ClockStamp)>{Ok((Instant::now(),stamp()))}
  fn observe_exit(&mut self)->Result<Option<ObservedExit>>{Ok(if self.ended&&self.frames.is_empty(){Some(ObservedExit{code:Some(0)})}else{None})}
  fn signal_stop(&mut self)->Result<()> {self.end(true);Ok(())}
  fn close_control(&mut self){}
  fn force(&mut self)->Result<()> {Ok(())}
 }
 struct Ops {trailing:&'static str,probe_fault:Option<ProbeError>,spawns:usize}
 impl FixtureOps for Ops {
  fn probe(&mut self,_:&ProofPermit,_:&str)->std::result::Result<(),ProbeError> {match self.probe_fault.clone() {Some(error)=>Err(error),None=>Ok(())}}
  fn capture(&mut self,_:&ProofPermit,request:TrackRequest,now:Instant)->Result<AudioSession>{self.spawns+=1;AudioSession::begin_with(vec![request],now,SharedRegistry::new(),||Ok(NONCE.into()),|_|Ok(Box::new(Child{renderer:false,frames:VecDeque::new(),ended:false,trailing:""})))}
  fn renderer(&mut self,_:&ProofPermit,request:TrackRequest,deadline:Instant)->Result<OwnedTrack>{self.spawns+=1;OwnedTrack::begin_with(request,deadline,NONCE.into(),SharedRegistry::new(),||Ok(Box::new(Child{renderer:true,frames:VecDeque::new(),ended:false,trailing:self.trailing})))}
  fn pause(&mut self){}
 }
 // This is protocol/ownership policy evidence through real owners, not generated audio or real exits.
 #[test] fn clean_actual_owned_collection_is_pending_never_passed() {
  let permit=ProofPermit{executable:"/approved/private.exe".into(),route:"owned-render-id".into()};
  match run_fixture_with(permit,"owned-render-id",Path::new("/output"),&mut Ops{trailing:"",probe_fault:None,spawns:0}) {
   ProofResult::Collected{receipts,qualification_pending}=>{assert!(qualification_pending);assert_eq!(receipts.capture.len(),1);assert_eq!(receipts.capture[0].exit_code,Some(0));assert_eq!(receipts.renderer.unwrap().exit_code,Some(0));assert!(receipts.microphone.starts_with("Unqualified"));},other=>panic!("clean collection was promoted or misclassified: {other:?}")
  }
 }

 // First notice and raw Terminal stay retained; later invalid protocol must still reject collection.
 #[test] fn renderer_late_protocol_faults_cannot_reuse_the_raw_terminal() {
  for trailing in ["malformed","truncated","extra"] {
   let permit=ProofPermit{executable:"/approved/private.exe".into(),route:"owned-render-id".into()};
   let mut ops=Ops{trailing,probe_fault:None,spawns:0};
   match run_fixture_with(permit,"owned-render-id",Path::new("/output"),&mut ops) {
    ProofResult::Failed{receipts,..}=>{let renderer=receipts.renderer.unwrap();assert_eq!(renderer.exit_code,Some(0));assert_eq!(renderer.failure.as_deref(),Some("fixture complete"));assert_eq!(renderer.terminal.unwrap().cause,"fixture complete");assert!(renderer.usable.is_none());},
    other=>panic!("{trailing} late protocol fault was misclassified: {other:?}"),
   }
   assert_eq!(ops.spawns,2);
  }
 }
 // Every operational error must fail before opening either actual owner, independent of wording.
 #[test] fn native_probe_operational_faults_fail_before_capture_or_render_spawn() {
  for reason in ["CoInitializeEx failed","CoCreateInstance failed","GetDevice E_ACCESSDENIED","GetState failed","Activate failed","GetMixFormat failed","no suitable route"] {
   let permit=ProofPermit{executable:"/approved/private.exe".into(),route:"owned-render-id".into()};
   let mut ops=Ops{trailing:"",probe_fault:Some(reason.to_string().into()),spawns:0};
   match run_fixture_with(permit,"owned-render-id",Path::new("/output"),&mut ops) {
    ProofResult::Failed{reason:actual,receipts}=>{assert_eq!(actual,reason);assert!(receipts.capture.is_empty());assert!(receipts.renderer.is_none());},
    other=>panic!("operational fault was misclassified: {other:?}"),
   }
   assert_eq!(ops.spawns,0);
  }
 }

 #[test] fn declared_route_unavailability_never_spawns_either_owner() {
  for error in [crate::proof::render::lookup_failure(0x80070490u32 as i32,"missing".into()),ProbeError::Unavailable("inactive".into()),ProbeError::Unavailable("unsupported mix".into())] {
   let permit=ProofPermit{executable:"/approved/private.exe".into(),route:"owned-render-id".into()};
   let mut ops=Ops{trailing:"",probe_fault:Some(error),spawns:0};
   assert!(matches!(run_fixture_with(permit,"owned-render-id",Path::new("/output"),&mut ops),ProofResult::Unavailable{..}));assert_eq!(ops.spawns,0);
  }
 }
 #[test] fn protocol_status_requires_drained_clean_exit_and_rejects_forced_zero_exit() {
  for cancel in [false,true] {
   let now=Instant::now();let request=TrackRequest{kind:TrackKind::System,endpoint_id:"owned-render-id".into(),explicitly_enabled:true,output_dir:std::env::temp_dir().join("protocol-status")};
   let mut track=OwnedTrack::begin_with(request,now+std::time::Duration::from_secs(8),NONCE.into(),SharedRegistry::new(),||Ok(Box::new(Child{renderer:true,frames:VecDeque::new(),ended:false,trailing:""}))).unwrap();
   assert!(track.poll(now).outcome.is_none());assert!(!track.completed_protocol());
   if cancel {track.cancel_storage(now).unwrap();}
   let outcome=track.poll(now).outcome.unwrap();assert_eq!(outcome.exit_code,Some(0));assert!(outcome.terminal.is_some());assert_eq!(track.completed_protocol(),!cancel);
  }
 }
}
