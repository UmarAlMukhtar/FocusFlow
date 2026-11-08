//! Manifest-free lifecycle. The embedded Session::arm/finish and Completion are never called.
use super::*;
pub(in crate::recorder) const MODE:&str="--focusflow-private-cursor-wgc";
pub(in crate::recorder) struct Proof {inner:Session,pub width:u32,pub height:u32}
impl Proof {
 pub(in crate::recorder) fn prepare(dir:&Path,window:u64,pid:u32,with_cursor:bool,ffmpeg:&Path,exe:&Path)->Result<Self>{
  crate::recorder::windows_qualification::cursor::require_disposable()?;
  if dir.read_dir().map_err(err)?.next().is_some(){return Err("Private proof directory must start empty".into());}
  let name=format!("Local\\FocusFlowPrivate-{}-{}",std::process::id(),qpc()?);let job=create_job(&name)?;
  let movie=dir.join(if with_cursor{"proof-with-cursor.mp4"}else{"proof-without-cursor.mp4"});
  let child=Command::new(exe).arg(MODE).arg(window.to_string()).arg(pid.to_string()).arg(&movie).arg(ffmpeg).arg(&name).arg(if with_cursor{"with"}else{"without"})
   .creation_flags(CREATE_NO_WINDOW.0).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(err)?;
  let(tx,messages)=mpsc::sync_channel(8);let mut inner=Session{child,job,encoder:None,messages,manifest:None,cursor_admission:cursor::Admission::embedded(),defaults:crate::export::ExportSettings::default().legacy_defaults().map_err(err)?,finished:false};
  let mut stdout=inner.child.stdout.take().ok_or("Private worker stdout missing")?;
  thread::Builder::new().name("private-wgc-control".into()).spawn(move||{for _ in 0..8{let m=read_message(&mut stdout);let end=matches!(m,Ok(Message::Terminal{..}|Message::Error{..})|Err(_));if tx.send(m).is_err()||end{return;}}}).map_err(err)?;
  // The exact owned worker remains gated until this non-inheritable Job has been assigned and queried.
  unsafe{AssignProcessToJobObject(inner.job.raw(),HANDLE(inner.child.as_raw_handle())).map_err(err)?;}in_job(HANDLE(inner.child.as_raw_handle()),inner.job.raw())?;inner.send(1).map_err(err)?;
  let Message::Prepared{width,height,cursor:cursor::Admission::Embedded{..}}=inner.receive(Instant::now()+Duration::from_secs(8)).map_err(err)? else{return Err("Private worker preparation failed".into());};
  Ok(Self{inner,width,height})
 }
 pub(in crate::recorder) fn arm(&mut self,dir:&Path)->Result<()> {
  let s=&mut self.inner;let end=Instant::now()+Duration::from_secs(5);s.send(2).map_err(err)?;
  let Message::Encoder{pid}=s.receive(end).map_err(err)? else{return Err("Private encoder ownership missing".into());};
  // Worker retains this exact Child and blocks on gate 4 until the parent's handle opens.
  let h=Handle(unsafe{OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|PROCESS_SYNCHRONIZE,false,pid).map_err(err)?}.0 as usize);in_job(h.raw(),s.job.raw())?;s.encoder=Some(h);arm_observation(s,dir)?;s.send(4).map_err(err)?;
  if !matches!(s.receive(end).map_err(err)?,Message::Ready{}){return Err("Private startup acknowledgement missing".into());}
  // Ready precedes the first admitted epoch; observe only after a complete causal cell reaches the writer.
  let deadline=Instant::now()+STAGE;while !dir.join("first-write-started.json").exists(){if Instant::now()>=deadline||s.child.try_wait().map_err(err)?.is_some(){return Err("Private first admitted cell not observed".into());}thread::sleep(POLL);}Ok(())
 }
 pub(in crate::recorder) fn ended(&mut self)->Result<bool>{self.inner.child.try_wait().map(|s|s.is_some()).map_err(err)}
 pub(in crate::recorder) fn finish(mut self,dir:&Path)->Result<()> {
  let s=&mut self.inner;let end=Instant::now()+Duration::from_secs(10);let sent=if s.child.try_wait().map_err(err)?.is_none(){s.send(3).is_ok()}else{false};let mut ack=None;
  let summary=loop{match s.receive(end).map_err(err)?{Message::Ack{cutoff} if ack.is_none()=>ack=Some(cutoff),Message::Terminal{summary,cursor:cursor::Terminal::Embedded}=>break summary,_=>return Err("Private terminal protocol order invalid".into())}};
  summary.check_stop_ack(sent,ack).map_err(err)?;let status=wait_child(&mut s.child,end)?;summary.clock(status).map_err(err)?;
  if !s.encoder.as_ref().is_some_and(|h|unsafe{WaitForSingleObject(h.raw(),0)}==WAIT_OBJECT_0){return Err("Private exact encoder handle not signaled".into());}
  s.finished=true;s.child.stdin.take();for name in ["capture.json","capture.windows-v2","capture.windows-v3","project.json","screen.mp4","session.json"]{if dir.join(name).exists(){return Err("Private proof published forbidden capture/import authority".into());}}
  let value=serde_json::json!({"privateProof":true,"neverImport":true,"clock":summary,"workerExit":status,"encoderSignaled":true});std::fs::write(dir.join("proof-summary.json"),serde_json::to_vec_pretty(&value).map_err(err)?).map_err(err)
 }
}
pub(in crate::recorder::windows_capture::process) fn save_pixels(path:&Path,p:&OwnedPixels,g:Geometry,seq:u64)->Result<()> {
 if seq>=2{return Ok(());}if g.width>1920||g.height>1080{return Err("Private raw exemplar exceeds 1920x1080 cap".into());}
 let mut bytes=format!("P7\nWIDTH {}\nHEIGHT {}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n",g.encoded_width,g.encoded_height).into_bytes();
 for px in p.bytes.chunks_exact(4){bytes.extend_from_slice(&[px[2],px[1],px[0],255]);}
 std::fs::write(path.with_file_name(format!("raw-{seq}.pam")),bytes).map_err(err)
}
