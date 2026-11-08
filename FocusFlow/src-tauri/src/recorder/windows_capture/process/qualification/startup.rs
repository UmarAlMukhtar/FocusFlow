//! Private scalar-only startup observation. No encoder, admission, pixels, or capture authority.
use super::super::*;
use serde_json::{json,Value};
use std::io::{Read,Write};

pub(in crate::recorder) const MODE:&str="--focusflow-startup-metadata";
const ROWS:usize=32;
const COLLECT:Duration=Duration::from_secs(2);

pub(in crate::recorder) fn require_disposable()->Result<()> {
 let get=|key:&str|std::env::var(key).map_err(|_|format!("Metadata probe gate missing {key}"));
 for(key,want)in[("GITHUB_ACTIONS","true"),("RUNNER_ENVIRONMENT","github-hosted"),("RUNNER_OS","Windows"),("GITHUB_REPOSITORY","UmarAlMukhtar/FocusFlow"),("GITHUB_REF","refs/heads/codex-windows-startup-metadata"),("GITHUB_EVENT_NAME","push"),("GITHUB_WORKFLOW_REF","UmarAlMukhtar/FocusFlow/.github/workflows/windows-owned-product-qualification.yml@refs/heads/codex-windows-startup-metadata")]{if get(key)?!=want{return Err(format!("Metadata probe gate rejected {key}"));}}
 let sha=get("GITHUB_SHA")?;if sha.len()!=40||!sha.bytes().all(|v|v.is_ascii_hexdigit())||get("FOCUSFLOW_METADATA_REVIEWED_SHA")?!=sha{return Err("Metadata SHA handshake rejected".into());}
 if get("FOCUSFLOW_METADATA_REVIEWED_REPOSITORY")?!=get("GITHUB_REPOSITORY")?||get("FOCUSFLOW_METADATA_REVIEWED_REF")?!=get("GITHUB_REF")?{return Err("Metadata repository/ref handshake rejected".into());}
 let id=get("GITHUB_RUN_ID")?;let attempt=get("GITHUB_RUN_ATTEMPT")?;if id.is_empty()||attempt.is_empty()||!id.bytes().all(|v|v.is_ascii_digit())||!attempt.bytes().all(|v|v.is_ascii_digit())||get("FOCUSFLOW_METADATA_REVIEWED_RUN")?!=format!("{id}:{attempt}"){return Err("Metadata run handshake rejected".into());}Ok(())
}
fn save(dir:&Path,name:&str,value:&Value)->Result<()> {
 let bytes=serde_json::to_vec(value).map_err(err)?;if bytes.len()>64*1024{return Err("Metadata receipt exceeds64KiB".into());}
 let path=dir.join(name);let pending=path.with_extension("pending");std::fs::write(&pending,bytes).map_err(err)?;std::fs::rename(pending,path).map_err(err)
}
fn phase(dir:&Path,name:&str)->Result<()> {save(dir,"startup-phase.json",&json!({"phase":name,"qpc":qpc()?}))}
fn identity(item:&::windows_capture::GraphicsCaptureItem)->String{format!("{item:?}").chars().take(192).collect()}
fn scalar(item:&::windows_capture::GraphicsCaptureItem,frame:Option<&Frame>,kind:&str)->Value {
 let mut errors=Vec::new();
 let mut keep=|name:&str,value:Result<Value>|->Value{match value{Ok(v)=>v,Err(e)=>{errors.push(format!("{name}: {}",e.chars().take(64).collect::<String>()));Value::Null}}};
 let q0=keep("q0",qpc().map(|v|json!(v)));let item_identity=json!(identity(item));let size=keep("itemSize",item.Size().map(|s|json!([s.Width,s.Height])).map_err(err));
 let(content,native,texture)=if let Some(f)=frame{(keep("contentSize",f.content_size().map(|v|json!([v.0,v.1])).map_err(err)),keep("native100ns",f.timestamp().map(|v|json!(v.Duration)).map_err(err)),json!([f.width(),f.height()]))}else{(Value::Null,Value::Null,Value::Null)};
 let q1=keep("q1",qpc().map(|v|json!(v)));json!({"phase":kind,"q0":q0,"q1":q1,"itemIdentity":item_identity,"itemSize":size,"contentSize":content,"textureSize":texture,"native100ns":native,"readErrors":errors})
}
struct Observations {rows:Vec<Value>,callbacks:usize,dropped:usize,first:Option<Instant>,first_qpc:Option<i64>,reason:Option<&'static str>,closed:bool}
#[derive(Clone)]
struct ProbeFlags {item: ::windows_capture::GraphicsCaptureItem,state:Arc<Mutex<Observations>>,frequency:i64}
struct MetadataCapture {flags:ProbeFlags}
impl GraphicsCaptureApiHandler for MetadataCapture {
 type Flags=ProbeFlags;type Error=Box<dyn std::error::Error+Send+Sync>;
 fn new(ctx:Context<ProbeFlags>)->std::result::Result<Self,Self::Error>{Ok(Self{flags:ctx.flags})}
 fn on_frame_arrived(&mut self,frame:&mut Frame,control:InternalCaptureControl)->std::result::Result<(),Self::Error>{
  let mut state=self.flags.state.lock().map_err(|_|"Metadata state poisoned")?;
  if state.reason.is_some(){control.stop();return Ok(());}
  if state.callbacks>=ROWS-2||state.first.is_some_and(|t|t.elapsed()>=COLLECT){let reason=if state.callbacks>=ROWS-2{"rowBudget"}else{"collectionDeadline"};state.reason.get_or_insert(reason);control.stop();return Ok(());}
  let row=scalar(&self.flags.item,Some(frame),"callback");let begin=row["q0"].as_i64();let end=row["q1"].as_i64();
  if state.first.is_none(){state.first=Some(Instant::now());state.first_qpc=begin;}
  if state.first_qpc.zip(end).is_some_and(|(a,b)|i128::from(b)-i128::from(a)>i128::from(self.flags.frequency)*2){state.dropped+=1;state.reason=Some("collectionDeadline");control.stop();return Ok(());}
  state.rows.push(row);state.callbacks+=1;
  if state.callbacks>=ROWS-2{state.reason=Some("rowBudget");control.stop();}Ok(())
 }
 fn on_closed(&mut self)->std::result::Result<(),Self::Error>{let mut state=self.flags.state.lock().map_err(|_|"Metadata state poisoned")?;state.closed=true;state.reason.get_or_insert("captureClosed");Ok(())}
}
fn worker(window:u64,pid:u32,dir:&Path,job:&str)->Result<()> {
 require_disposable()?;control(1)?;
 // Close the query handle before capture so parent death still closes the last Job handle.
 {let wide:Vec<u16>=job.encode_utf16().chain([0]).collect();let job=Handle(unsafe{OpenJobObjectW(0x0004,false,PCWSTR(wide.as_ptr())).map_err(err)?}.0 as usize);in_job(unsafe{GetCurrentProcess()},job.raw())?;}
 let _apartment=Apartment::initialize()?;let owner=SourceOwner::prepare(window,pid)?;let captured:GraphicsCaptureItemType=verify_identity(window,pid)?.try_into().map_err(err)?;
 let GraphicsCaptureItemType::Window((item,window_handle))=captured else{return Err("Metadata probe requires a window item".into());};let id=identity(&item);let retained=item.clone();let same_item=retained==item;let freq=frequency()?;
 let state=Arc::new(Mutex::new(Observations{rows:Vec::with_capacity(ROWS),callbacks:0,dropped:0,first:None,first_qpc:None,reason:None,closed:false}));
 state.lock().map_err(err)?.rows.push(scalar(&item,None,"beforeStart"));
 let snapshot=|s:&Observations|json!({"metadataOnly":true,"selectedHwnd":window,"sourcePid":pid,"sourceThread":owner.thread,"itemIdentity":id,"sameItemClone":same_item,"frequency":freq,"rowLimit":ROWS,"collectionLimitMs":2000,"rows":s.rows,"callbacks":s.callbacks,"dropped":s.dropped,"stopReason":s.reason,"captureClosed":s.closed,"stopComplete":false,"stopError":null,"collectionError":null,"startupError":null,"readErrorCount":s.rows.iter().map(|v|v["readErrors"].as_array().map_or(0,Vec::len)).sum::<usize>()});
 save(dir,"startup-metadata.json",&state.lock().map(|s|snapshot(&s)).map_err(err)?)?;phase(dir,"starting")?;owner.require_available()?;
 if !same_item{return Err("Retained metadata item identity changed".into());}
 let flags=ProbeFlags{item:item.clone(),state:state.clone(),frequency:freq};
 let settings=Settings::new(PreparedWindow{item,window:window_handle},CursorCaptureSettings::WithoutCursor,DrawBorderSettings::Default,SecondaryWindowSettings::Default,MinimumUpdateIntervalSettings::Default,DirtyRegionSettings::Default,ColorFormat::Bgra8,flags);
 let capture=match MetadataCapture::start_free_threaded(settings){Ok(c)=>c,Err(e)=>{let message=err(e);let mut receipt=state.lock().map(|s|snapshot(&s)).map_err(err)?;receipt["startupError"]=json!(message);receipt["stopReason"]=json!("startupError");save(dir,"startup-metadata.json",&receipt)?;return Err(message);}};
 let collect=(||->Result<()>{
  state.lock().map_err(err)?.rows.push(scalar(&retained,None,"afterStart"));phase(dir,"collecting")?;let wait=Instant::now();
  loop{let mut s=state.lock().map_err(err)?;
   if let Some(reason)=owner.observe(){s.reason=Some(match reason{SourceAvailabilityEnd::Unavailable=>"sourceUnavailable",SourceAvailabilityEnd::Unconfirmed=>"sourceUnconfirmed"});break;}
   if s.reason.is_some(){break;}if s.first.map_or_else(||wait.elapsed(),|t|t.elapsed())>=COLLECT{s.reason=Some("collectionDeadline");break;}if capture.is_finished(){s.reason=Some("captureEnded");break;}drop(s);thread::sleep(POLL);
  }Ok(())
 })();
 let stopping=phase(dir,"stopping");
 let partial_snapshot=state.lock().map(|s|snapshot(&s)).map_err(err);
 let partial=partial_snapshot.as_ref().map_err(Clone::clone).and_then(|v|save(dir,"startup-metadata.json",v));
 let stopped=if capture.is_finished(){capture.wait().map_err(err)}else{capture.stop().map_err(err)};
 let collect=collect.and_then(|_|owner.require_available());
 let mut final_value=state.lock().map(|s|snapshot(&s)).map_err(err)?;final_value["stopComplete"]=json!(stopped.is_ok());final_value["stopError"]=json!(stopped.as_ref().err());final_value["collectionError"]=json!(collect.as_ref().err());
 let read_errors=final_value["rows"].as_array().ok_or("Metadata rows missing")?.iter().map(|v|v["readErrors"].as_array().map_or(0,Vec::len)).sum::<usize>();final_value["readErrorCount"]=json!(read_errors);
 save(dir,"startup-metadata.json",&final_value)?;phase(dir,"finished")?;collect?;stopping?;partial?;stopped?;
 if final_value["callbacks"].as_u64().unwrap_or(0)==0||read_errors!=0{return Err("Metadata probe has no callbacks or scalar read failures".into());}Ok(())
}
struct ChildScope {child:Child,job:Handle,finished:bool}
impl Drop for ChildScope{fn drop(&mut self){if !self.finished{unsafe{let _=TerminateJobObject(self.job.raw(),5);}let _=self.child.kill();let _=wait_child(&mut self.child,Instant::now()+STAGE);}}}
pub(in crate::recorder) fn run(dir:&Path,window:u64,pid:u32,exe:&Path)->Result<()> {
 require_disposable()?;if dir.read_dir().map_err(err)?.next().is_some(){return Err("Metadata probe directory must start empty".into());}
 let name=format!("Local\\FocusFlowMetadata-{}-{}",std::process::id(),qpc()?);let job=create_job(&name)?;
 let child=Command::new(exe).arg(MODE).arg(window.to_string()).arg(pid.to_string()).arg(dir).arg(&name).creation_flags(CREATE_NO_WINDOW.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(err)?;let mut scope=ChildScope{child,job,finished:false};
 unsafe{AssignProcessToJobObject(scope.job.raw(),HANDLE(scope.child.as_raw_handle())).map_err(err)?;}in_job(HANDLE(scope.child.as_raw_handle()),scope.job.raw())?;
 save(dir,"metadata-owned.json",&json!({"parent":std::process::id(),"worker":scope.child.id(),"qpc":qpc()?}))?;
 let observed=(||->Result<()>{let end=Instant::now()+STAGE;while !dir.join("metadata-handles-open").exists(){if Instant::now()>=end{return Err("Metadata exact observer handshake missing".into());}thread::sleep(POLL);}Ok(())})();
 let result=(||->Result<i32>{observed?;scope.child.stdin.as_mut().ok_or("Metadata worker stdin missing")?.write_all(&[1]).map_err(err)?;scope.child.stdin.take();
  let start=Instant::now();let mut phase_name=String::new();let mut phase_start=Instant::now();
  loop{if let Some(code)=scope.child.try_wait().map_err(err)?{return code.code().ok_or("Metadata worker exit code missing".into());}
   if start.elapsed()>Duration::from_secs(12){return Err("Metadata overall watchdog fired".into());}
   let path=dir.join("startup-phase.json");if path.is_file(){let bytes=std::fs::read(path).map_err(err)?;if bytes.len()>1024{return Err("Metadata phase cap".into());}let phase:Value=serde_json::from_slice(&bytes).map_err(err)?;let current=phase["phase"].as_str().ok_or("Metadata phase missing")?;
    if !["starting","collecting","stopping","finished"].contains(&current){return Err("Unknown metadata phase".into());}if current!=phase_name{phase_name=current.to_owned();phase_start=Instant::now();}
   }
   let limit=match phase_name.as_str(){"collecting"=>Duration::from_secs(3),"stopping"|"finished"=>STAGE,_=>Duration::from_secs(8)};if phase_start.elapsed()>limit{return Err(format!("Metadata {phase_name} watchdog fired"));}thread::sleep(POLL);
  }
 })();
 let fallback=result.is_err();if fallback{unsafe{let _=TerminateJobObject(scope.job.raw(),5);}let _=scope.child.kill();}let reaped=wait_child(&mut scope.child,Instant::now()+STAGE);scope.finished=reaped.is_ok();
 let mut output=Vec::new();if scope.finished{if let Some(stdout)=scope.child.stdout.take(){stdout.take(4097).read_to_end(&mut output).map_err(err)?;}}if output.len()>4096{return Err("Metadata worker output cap".into());}
 save(dir,"metadata-cleanup.json",&json!({"worker":scope.child.id(),"workerExit":reaped.as_ref().ok(),"workerSignaled":scope.finished,"fallback":fallback,"error":result.as_ref().err(),"workerMessage":String::from_utf8_lossy(&output)}))?;reaped?;let code=result?;if code!=0{return Err(format!("Metadata worker exited {code}"));}Ok(())
}
pub(in crate::recorder) fn dispatch(args:impl Iterator<Item=OsString>)->i32 {
 let result=(||->Result<()>{require_disposable()?;let values:Vec<_>=args.take(5).collect();if values.len()!=4{return Err("Invalid metadata worker arguments".into());}worker(values[0].to_str().ok_or("Metadata HWND")?.parse().map_err(err)?,values[1].to_str().ok_or("Metadata PID")?.parse().map_err(err)?,Path::new(&values[2]),values[3].to_str().ok_or("Metadata Job")?)})();
 match result{Ok(())=>0,Err(message)=>{let _=emit(Message::Error{message:message.chars().take(1024).collect()});1}}
}
