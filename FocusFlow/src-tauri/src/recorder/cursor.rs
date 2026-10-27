//! Owned provisional telemetry and bounded exact-clock cursor asset finalization.
use super::{CaptureBounds, RecorderError, RecorderResult};
use focusflow_core::cursor::{CursorIndex, CursorShape, ARROW_GEOMETRY, MAX_INDEX_BYTES,
    MAX_NATIVE_BYTES, MAX_RECORD_BYTES, MAX_SAMPLES, MAX_SAMPLES_BYTES, MAX_SHAPES};
use serde::{Deserialize, Serialize};
use std::{fs::{self, File, OpenOptions}, io::Write, net::Shutdown, os::{fd::AsFd, unix::net::UnixStream},
    path::{Path, PathBuf}, sync::{mpsc::{self, Receiver, SyncSender}, Arc, Condvar, Mutex},
    thread::{self, JoinHandle}, time::{Duration, Instant}};
use x11rb::{connection::Connection, errors::ReplyError, protocol::{xfixes::{self, ConnectionExt as _}, xproto::{ConnectionExt as _, MapState}},
    reexports::x11rb_protocol::{parse_display::{self, ConnectAddress}, xauth}, rust_connection::{DefaultStream, RustConnection}};

const STARTUP: Duration = Duration::from_secs(3);
const GRACE: Duration = Duration::from_millis(100);
const TICK: Duration = Duration::from_nanos(16_666_667);
fn error(reason: &'static str) -> RecorderError { RecorderError::new("cursor_unavailable", reason, true) }

#[derive(Clone, Copy, Debug)]
pub(super) struct Source { pub bounds: CaptureBounds, pub window: Option<u32> }
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Outcome {
    pub raw_path: PathBuf,
    pub committed_bytes: u64,
    pub records: usize,
    pub shapes: Vec<CursorShape>,
    pub cutoff_ns: Option<i128>,
    pub trustworthy_prefix: bool,
    pub degraded: bool,
    pub reason: Option<&'static str>,
}
impl Outcome {
    fn missing(raw_path: PathBuf, cutoff_ns: Option<i128>) -> Self {
        Self { raw_path, committed_bytes:0, records:0, shapes:vec![], cutoff_ns, trustworthy_prefix:false,
            degraded:true, reason:Some("Cursor worker did not return a trustworthy result") }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RawRecord {
    #[serde(with = "nanoseconds")]
    pub before_ns: i128,
    #[serde(with = "nanoseconds")]
    pub after_ns: i128,
    pub state: RawState,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum RawState {
    Observed {
        #[serde(deserialize_with = "finite")]
        x: f64,
        #[serde(deserialize_with = "finite")]
        y: f64,
        visible: bool,
        #[serde(rename = "shapeId", deserialize_with = "shape_id")]
        shape_id: Option<String>,
        root: u32,
        #[serde(rename = "originX")]
        origin_x: i32,
        #[serde(rename = "originY")]
        origin_y: i32,
        discontinuity: bool,
    },
    Barrier {},
}
mod nanoseconds {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};
    pub fn serialize<S: Serializer>(v: &i128, s:S) -> Result<S::Ok,S::Error> {
        if *v < 0 {return Err(serde::ser::Error::custom("Negative cursor time"));}
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de,D:Deserializer<'de>>(d:D) -> Result<i128,D::Error> {
        let s=String::deserialize(d)?;
        if s.is_empty() || s.len()>39 || (s.len()>1 && s.starts_with('0')) || !s.bytes().all(|c|c.is_ascii_digit()) {
            return Err(D::Error::custom("Noncanonical cursor time"));
        }
        s.parse().map_err(|_|D::Error::custom("Cursor time overflow"))
    }
}
fn finite<'de,D:serde::Deserializer<'de>>(d:D)->Result<f64,D::Error> {
    let x=f64::deserialize(d)?;
    if x.is_finite() {Ok(x)} else {Err(serde::de::Error::custom("Nonfinite cursor position"))}
}
fn shape_id<'de,D:serde::Deserializer<'de>>(d:D)->Result<Option<String>,D::Error> {
    let id=Option::<String>::deserialize(d)?;
    if id.as_ref().is_some_and(|s|s.len()!=9 || !s.starts_with("shape-") || !s.as_bytes()[6..].iter().all(u8::is_ascii_digit) || s[6..].parse::<usize>().map_or(true,|n|n>=MAX_SHAPES)) {
        return Err(serde::de::Error::custom("Invalid cursor shape ID"));
    }
    Ok(id)
}
fn midpoint(before:i128,after:i128)->Option<i128> {
    let span=after.checked_sub(before)?;
    if before<0 || !(0..=20_000_000).contains(&span) {None} else {before.checked_add(span/2)}
}
fn now(epoch:Instant)->i128 {Instant::now().checked_duration_since(epoch).unwrap_or_default().as_nanos() as i128}

#[derive(Clone, Copy)]
struct Limits {shapes:usize,decoded:usize,index:u64,rows:usize,raw_bytes:u64,line:usize}
impl Default for Limits {
    fn default()->Self {Self{shapes:MAX_SHAPES,decoded:MAX_NATIVE_BYTES,index:MAX_INDEX_BYTES,rows:MAX_SAMPLES,raw_bytes:MAX_SAMPLES_BYTES,line:MAX_RECORD_BYTES}}
}
struct RawWriter<W> {writer:W,bytes:u64,records:usize,limits:Limits,closed:bool}
impl<W:Write> RawWriter<W> {
    fn new(writer:W,limits:Limits)->Self {Self{writer,bytes:0,records:0,limits,closed:false}}
    fn append(&mut self,row:&RawRecord)->Result<(), &'static str> {
        if self.closed {return Err("Cursor telemetry is closed");}
        let result=(|| {
            let mut line=serde_json::to_vec(row).map_err(|_|"Cursor telemetry serialization failed")?;
            line.push(b'\n');
            if line.len()>self.limits.line || self.records>=self.limits.rows || self.bytes.checked_add(line.len() as u64).is_none_or(|n|n>self.limits.raw_bytes) {
                return Err("Cursor telemetry budget exhausted");
            }
            self.writer.write_all(&line).map_err(|_|"Cursor telemetry write failed")?;
            self.bytes+=line.len() as u64;self.records+=1;Ok(())
        })();
        if result.is_err() {self.closed=true;}
        result
    }
}

#[derive(PartialEq)]
struct NativeImage {geometry:(u32,u32,u32,u32),pixels:Vec<u8>,opaque:bool}
// X11 reply parsing allocates before these limits; this transport requires a trusted local server.
impl NativeImage {
    fn convert(i:&xfixes::GetCursorImageReply)->Result<Self,&'static str> {
        if !(1..=512).contains(&i.width) || !(1..=512).contains(&i.height) || i.xhot>=i.width || i.yhot>=i.height || i.cursor_image.len()!=usize::from(i.width)*usize::from(i.height) {
            return Err("Cursor image geometry is invalid");
        }
        let mut pixels=Vec::with_capacity(i.cursor_image.len()*4);let mut opaque=false;
        for p in &i.cursor_image {
            let a=p>>24;
            if a==0 {pixels.extend_from_slice(&[0;4]);continue;}
            opaque=true;
            for c in [(p>>16)&255,(p>>8)&255,p&255] {
                if c>a {return Err("Cursor image pixels are invalid");}
                pixels.push(((c*255+a/2)/a).min(255) as u8);
            }
            pixels.push(a as u8);
        }
        Ok(Self{geometry:(i.width.into(),i.height.into(),i.xhot.into(),i.yhot.into()),pixels,opaque})
    }
}
fn index_bytes(bounds:CaptureBounds,shapes:Vec<CursorShape>)->Result<Vec<u8>,&'static str> {
    // Maximum duration digits reserve exact serialized space before the accepted clock is known.
    serde_json::to_vec(&CursorIndex{schema_version:1,width:bounds.width,height:bounds.height,duration_us:u64::MAX,samples_path:"cursor/samples.ndjson".into(),shapes}).map_err(|_|"Cursor index serialization failed")
}
struct Assets {root:PathBuf,bounds:CaptureBounds,shapes:Vec<CursorShape>,images:Vec<NativeImage>,decoded:usize,limits:Limits,degraded:bool,reason:Option<&'static str>}
impl Assets {
    fn create(root:&Path,bounds:CaptureBounds,limits:Limits)->Result<(Self,File),&'static str> {
        fs::create_dir(root.join("cursor")).map_err(|_|"Could not create owned cursor directory")?;
        fs::create_dir(root.join("cursor/shapes")).map_err(|_|"Could not create owned cursor shapes directory")?;
        let raw=OpenOptions::new().write(true).create_new(true).open(root.join("cursor/raw.ndjson")).map_err(|_|"Could not create owned cursor telemetry")?;
        Ok((Self{root:root.into(),bounds,shapes:vec![],images:vec![],decoded:0,limits,degraded:false,reason:None},raw))
    }
    fn degrade(&mut self,reason:&'static str) {self.degraded=true;self.reason.get_or_insert(reason);}
    fn admit(&mut self,i:&xfixes::GetCursorImageReply)->Result<(String,(u32,u32,u32,u32),bool),&'static str> {
        if self.degraded {return Err("Cursor image capture was degraded");}
        let result=(|| {
            let image=NativeImage::convert(i)?;
            if let Some(n)=self.images.iter().position(|p|p==&image) {return Ok((self.shapes[n].id.clone(),image.geometry,image.opaque));}
            if self.shapes.len()>=self.limits.shapes || self.decoded+image.pixels.len()>self.limits.decoded {return Err("Cursor image budget exhausted");}
            let id=format!("shape-{:03}",self.shapes.len());let path=format!("cursor/shapes/{id}.pam");
            let (width,height,hotspot_x,hotspot_y)=image.geometry;
            let shape=CursorShape{id:id.clone(),path:path.clone(),width,height,hotspot_x,hotspot_y};
            let mut shapes=self.shapes.clone();shapes.push(shape.clone());
            if index_bytes(self.bounds,shapes)?.len() as u64>self.limits.index {return Err("Cursor index budget exhausted");}
            let mut file=OpenOptions::new().create_new(true).write(true).open(self.root.join(path)).map_err(|_|"Could not create cursor image")?;
            write!(file,"P7\nWIDTH {width}\nHEIGHT {height}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n").and_then(|_|file.write_all(&image.pixels)).map_err(|_|"Could not write cursor image")?;
            let geometry=image.geometry;let opaque=image.opaque;
            self.decoded+=image.pixels.len();self.images.push(image);self.shapes.push(shape);
            Ok((id,geometry,opaque))
        })();
        if let Err(reason)=result {self.degrade(reason);}
        result
    }
}
#[derive(Clone,Copy,PartialEq,Eq)]
struct Geometry {root:u32,x:i32,y:i32,width:u32,height:u32,viewable:bool,same_screen:bool}
#[derive(Default)]
struct Continuity {last:Option<(u32,i32,i32)>}
impl Continuity {
    fn barrier(&mut self)->RawState {self.last=None;RawState::Barrier {}}
    fn observe(&mut self,source:Source,root:u32,before:Geometry,after:Geometry,x:i32,y:i32,image:Option<(String,(u32,u32,u32,u32))>,opaque:bool)->RawState {
        if before!=after || before.root!=root || !before.same_screen || !before.viewable || before.width<source.bounds.width || before.height<source.bounds.height {return self.barrier();}
        let (origin_x,origin_y)=if source.window.is_some() {(before.x,before.y)} else {(source.bounds.x,source.bounds.y)};
        let identity=(root,origin_x,origin_y);let discontinuity=self.last!=Some(identity);self.last=Some(identity);
        let x=i64::from(x)-i64::from(origin_x);let y=i64::from(y)-i64::from(origin_y);
        let (shape_id,(w,h,hx,hy))=match image {Some((id,g))=>(Some(id),g),None=>(None,ARROW_GEOMETRY)};
        let margin=(-2048..=i64::from(source.bounds.width)+2048).contains(&x) && (-2048..=i64::from(source.bounds.height)+2048).contains(&y);
        let visible=margin && opaque && x-i64::from(hx)<i64::from(source.bounds.width) && y-i64::from(hy)<i64::from(source.bounds.height) && x-i64::from(hx)+i64::from(w)>0 && y-i64::from(hy)+i64::from(h)>0;
        let (x,y)=if margin {(x as f64/f64::from(source.bounds.width),y as f64/f64::from(source.bounds.height))} else {(0.0,0.0)};
        RawState::Observed{x,y,visible,shape_id,root,origin_x,origin_y,discontinuity}
    }
}
fn local_address(display:Option<&str>)->Result<(ConnectAddress<'static>,usize,u16),&'static str> {
    let parsed=parse_display::parse_display(display).map_err(|_|"Local X11 display could not be parsed")?;
    for address in parsed.connect_instruction() {
        if let ConnectAddress::Socket(path)=address {return Ok((ConnectAddress::Socket(path),parsed.screen.into(),parsed.display));}
    }
    Err("Cursor capture requires a local Unix X11 connection")
}
#[derive(Default)]
struct Control {stopped:bool,epoch:Option<Instant>,cutoff_ns:Option<i128>,socket:Option<UnixStream>}
#[derive(Default)]
struct Shared {control:Mutex<Control>,wake:Condvar}
impl Shared {
    fn lock(&self)->std::sync::MutexGuard<'_,Control> {self.control.lock().unwrap_or_else(|p|p.into_inner())}
    fn stop(&self) {
        let mut c=self.lock();
        if !c.stopped {c.stopped=true;c.cutoff_ns=c.epoch.map(now);}
        self.wake.notify_all();
    }
    fn interrupt(&self) {
        self.stop();
        if let Some(socket)=self.lock().socket.take() {let _=socket.shutdown(Shutdown::Both);}
    }
    fn pause(&self,duration:Duration) {
        let _=self.wake.wait_timeout_while(self.lock(),duration,|c|!c.stopped).unwrap_or_else(|p|p.into_inner());
    }
}
fn setup_with_auth(shared:&Shared,stream:DefaultStream,screen:usize,auth:impl FnOnce()->(Vec<u8>,Vec<u8>))->Result<RustConnection,&'static str> {
    let socket=UnixStream::from(AsFd::as_fd(&stream).try_clone_to_owned().map_err(|_|"Could not own cursor connection shutdown")?);
    {
        let mut c=shared.lock();
        if c.stopped {let _=socket.shutdown(Shutdown::Both);return Err("Cursor startup was stopped");}
        c.socket=Some(socket);
    }
    let (name,data)=auth();
    if shared.lock().stopped {return Err("Cursor startup was stopped");}
    RustConnection::connect_to_stream_with_auth_info(stream,screen,name,data).map_err(|_|"Cursor X11 setup failed")
}
#[cfg(test)]
fn setup_stream(shared:&Shared,stream:DefaultStream,screen:usize,name:Vec<u8>,data:Vec<u8>)->Result<RustConnection,&'static str> {
    setup_with_auth(shared,stream,screen,||(name,data))
}
// Local connect/auth and filesystem calls have no universal interruption deadline.
fn connect(shared:&Shared)->Result<(RustConnection,usize),&'static str> {
    let (address,screen,display)=local_address(None)?;
    let (stream,(family,address))=DefaultStream::connect(&address).map_err(|_|"Could not connect to local X11 cursor source")?;
    let c=setup_with_auth(shared,stream,screen,||xauth::get_auth(family,&address,display).unwrap_or(None).unwrap_or_default())?;
    Ok((c,screen))
}

pub(super) struct Sampler {
    shared:Arc<Shared>,worker:Option<JoinHandle<()>>,readiness:Receiver<Result<(),&'static str>>,
    acknowledgement:Receiver<()>,result:Receiver<Outcome>,raw_path:PathBuf,outcome:Option<Outcome>,started:bool,
}
impl Sampler {
    pub fn prepare(session_dir:&Path,source:Source)->RecorderResult<Self> {
        let mut sampler=Self::spawn(session_dir,source,connect)?;
        sampler.ready(STARTUP)?;Ok(sampler)
    }
    fn spawn(session_dir:&Path,source:Source,connector:impl FnOnce(&Shared)->Result<(RustConnection,usize),&'static str>+Send+'static)->RecorderResult<Self> {
        if source.bounds.width==0 || source.bounds.height==0 || source.window==Some(0) {return Err(error("Cursor source geometry or window is invalid"));}
        let (assets,file)=Assets::create(session_dir,source.bounds,Limits::default()).map_err(error)?;
        let raw_path=session_dir.join("cursor/raw.ndjson");
        let shared=Arc::new(Shared::default());let worker_shared=Arc::clone(&shared);
        let (ready_tx,readiness)=mpsc::sync_channel(1);let (ack_tx,acknowledgement)=mpsc::sync_channel(1);let (result_tx,result)=mpsc::sync_channel(1);
        let worker=thread::Builder::new().name("cursor-sampler".into()).spawn(move || {
            let out=run(&worker_shared,source,assets,file,connector,ready_tx,ack_tx);
            let _=result_tx.send(out);
        }).map_err(|_|error("Could not start cursor worker"))?;
        Ok(Self{shared,worker:Some(worker),readiness,acknowledgement,result,raw_path,outcome:None,started:false})
    }
    fn ready(&mut self,timeout:Duration)->RecorderResult<()> {
        let reason=match self.readiness.recv_timeout(timeout) {Ok(Ok(()))=>return Ok(()),Ok(Err(reason))=>reason,Err(_)=>"Cursor startup did not become ready"};
        self.shared.interrupt();self.cleanup();Err(error(reason))
    }
    pub fn start(&mut self,epoch:Instant)->RecorderResult<()> {
        if self.started {return Err(error("Cursor sampling has already started"));}
        {
            let mut c=self.shared.lock();
            if c.stopped || epoch>Instant::now() {return Err(error("Cursor sampling cannot start with this epoch"));}
            c.epoch=Some(epoch);self.started=true;self.shared.wake.notify_all();
        }
        if self.acknowledgement.recv_timeout(STARTUP).is_err() {
            self.shared.interrupt();self.cleanup();return Err(error("Cursor sampling did not acknowledge start"));
        }
        Ok(())
    }
    pub fn request_stop(&self) {self.shared.stop();}
    pub fn finish(mut self)->Outcome {
        self.cleanup();self.outcome.take().unwrap_or_else(||Outcome::missing(self.raw_path.clone(),self.shared.lock().cutoff_ns))
    }
    fn cleanup(&mut self) {
        let Some(worker)=self.worker.take() else {return;};
        self.request_stop();
        let result=self.result.recv_timeout(GRACE).ok();
        // Shutdown interrupts both setup and pending reply reads; joining remains mandatory.
        self.shared.interrupt();
        let joined=worker.join().is_ok();
        self.outcome=Some(if joined {result.or_else(||self.result.try_recv().ok()).unwrap_or_else(||Outcome::missing(self.raw_path.clone(),self.shared.lock().cutoff_ns))}
            else {Outcome::missing(self.raw_path.clone(),self.shared.lock().cutoff_ns)});
    }
}
impl Drop for Sampler {fn drop(&mut self) {self.cleanup();}}
fn commit<W:Write>(shared:&Shared,raw:&mut RawWriter<W>,row:&RawRecord)->Result<bool,&'static str> {
    let mut c=shared.lock();
    if c.stopped {return Ok(false);}
    // Hold the same latch through the complete write so cutoff cannot precede a late commit.
    if let Err(reason)=raw.append(row) {
        c.stopped=true;c.cutoff_ns=c.epoch.map(now);shared.wake.notify_all();return Err(reason);
    }
    Ok(true)
}
fn run(shared:&Shared,source:Source,mut assets:Assets,file:File,
    connector:impl FnOnce(&Shared)->Result<(RustConnection,usize),&'static str>,ready:SyncSender<Result<(),&'static str>>,ack:SyncSender<()>)->Outcome {
    let mut raw=RawWriter::new(file,assets.limits);let mut trustworthy=true;
    let startup=(|| {
        let (c,screen)=connector(shared)?;
        let root=c.setup().roots.get(screen).ok_or("Cursor X11 screen is unavailable")?.root;
        let version=c.xfixes_query_version(2,0).map_err(|_|"XFixes cursor images are unavailable")?.reply().map_err(|_|"XFixes cursor images are unavailable")?;
        if version.major_version<2 {return Err("XFixes cursor images are unavailable");}
        let g=geometry(&c,source,root).map_err(|_|"Cursor source admission failed")?;
        if g.root!=root || !g.same_screen || !g.viewable || g.width<source.bounds.width || g.height<source.bounds.height {return Err("Cursor source geometry is unavailable");}
        if shared.lock().stopped {return Err("Cursor startup was stopped");}
        Ok((c,root))
    })();
    match startup {
        Err(reason)=>{assets.degrade(reason);let _=ready.send(Err(reason));shared.stop();}
        Ok((connection,root))=>{
            let _=ready.send(Ok(()));
            let epoch={
                let c=shared.wake.wait_while(shared.lock(),|c|!c.stopped && c.epoch.is_none()).unwrap_or_else(|p|p.into_inner());
                if c.stopped {None} else {c.epoch}
            };
            if let Some(epoch)=epoch {
                let _=ack.send(());let mut continuity=Continuity::default();
                while !shared.lock().stopped {
                    match observe(&connection,source,root,epoch,&mut assets,&mut continuity) {
                        Ok(row)=>match commit(shared,&mut raw,&row) {
                            Ok(true)=>{},Ok(false)=>break,Err(reason)=>{assets.degrade(reason);break;}
                        },
                        Err(reason)=>{assets.degrade(reason);shared.stop();break;}
                    }
                    shared.pause(TICK);
                }
            }
        }
    }
    shared.stop();
    if raw.writer.flush().is_err() {trustworthy=false;assets.degrade("Cursor telemetry flush failed");}
    Outcome{raw_path:assets.root.join("cursor/raw.ndjson"),committed_bytes:raw.bytes,records:raw.records,shapes:assets.shapes,
        cutoff_ns:shared.lock().cutoff_ns,trustworthy_prefix:trustworthy,degraded:assets.degraded,reason:assets.reason}
}
fn geometry(c:&RustConnection,source:Source,root:u32)->Result<Geometry,ReplyError> {
    if let Some(window)=source.window {
        let g=c.get_geometry(window)?.reply()?;
        let xy=c.translate_coordinates(window,root,0,0)?.reply()?;
        let attr=c.get_window_attributes(window)?.reply()?;
        Ok(Geometry{root:g.root,x:xy.dst_x.into(),y:xy.dst_y.into(),width:g.width.into(),height:g.height.into(),viewable:attr.map_state==MapState::VIEWABLE,same_screen:xy.same_screen})
    } else {
        Ok(Geometry{root,x:source.bounds.x,y:source.bounds.y,width:source.bounds.width,height:source.bounds.height,viewable:true,same_screen:true})
    }
}
fn observe(c:&RustConnection,source:Source,root:u32,epoch:Instant,assets:&mut Assets,continuity:&mut Continuity)->Result<RawRecord,&'static str> {
    let mut before_ns=now(epoch);
    let result=(|| -> Result<_,ReplyError> {
        let mut before=geometry(c,source,root)?;
        let image=c.xfixes_get_cursor_image()?.reply();
        let image=match image {
            Ok(i)=>match NativeImage::convert(&i) {
                Ok(native)=>Some((i,native.opaque)),
                Err(reason)=>{
                    assets.degrade(reason);before_ns=now(epoch);before=geometry(c,source,root)?;None
                }
            },
            Err(ReplyError::X11Error(_))=>{
                assets.degrade("Cursor image reply was unavailable");
                // The fallback position owns a fresh bracket, including its window evidence.
                before_ns=now(epoch);before=geometry(c,source,root)?;None
            }
            Err(e)=>return Err(e),
        };
        let pointer=c.query_pointer(root)?.reply()?;
        let after=geometry(c,source,root)?;
        let after_ns=now(epoch);
        Ok((before,after,image,pointer,after_ns))
    })();
    match result {
        Err(ReplyError::X11Error(_))=>Ok(RawRecord{before_ns,after_ns:now(epoch),state:continuity.barrier()}),
        Err(_)=>Err("Cursor X11 connection was lost"),
        Ok((before,after,image,pointer,after_ns))=>{
            let state=if midpoint(before_ns,after_ns).is_none() || !pointer.same_screen || pointer.root!=root {continuity.barrier()} else {
                let (x,y,shape,opaque)=match image {
                    Some((i,opaque))=>{let admitted=assets.admit(&i).ok();(i.x.into(),i.y.into(),admitted.map(|(id,g,_)|(id,g)),opaque)},
                    None=>(pointer.root_x.into(),pointer.root_y.into(),None,true),
                };
                continuity.observe(source,root,before,after,x,y,shape,opaque)
            };
            Ok(RawRecord{before_ns,after_ns,state})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            loop {
                let p = std::env::temp_dir().join(format!("focusflow-cursor-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
                match fs::create_dir(&p) { Ok(()) => return Self(p), Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue, Err(e) => panic!("{e}") }
            }
        }
    }
    impl Drop for Dir { fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); } }
    fn source() -> Source { Source { bounds: CaptureBounds { x:-100,y:20,width:100,height:50 }, window:None } }
    fn image(width:u16,height:u16,xhot:u16,yhot:u16,pixels:Vec<u32>) -> xfixes::GetCursorImageReply {
        xfixes::GetCursorImageReply { sequence:0,length:0,x:0,y:0,width,height,xhot,yhot,cursor_serial:7,cursor_image:pixels }
    }
    fn row(before:i128, after:i128) -> RawRecord { RawRecord { before_ns:before,after_ns:after,state:RawState::Barrier {} } }
    #[test]
    fn numeric_argb_unpremultiplies_and_rejects_invalid_components() {
        let p=NativeImage::convert(&image(4,1,0,0,vec![0x00ffffff,0x01010001,0x80402080,0xff123456])).unwrap();
        assert_eq!(p.pixels,vec![0,0,0,0,255,0,255,1,128,64,255,128,18,52,86,255]);
        assert!(NativeImage::convert(&image(1,1,0,0,vec![0x01020000])).is_err());
        assert!(!NativeImage::convert(&image(1,1,0,0,vec![0x00abcdef])).unwrap().opaque);
    }
    #[test]
    fn image_geometry_payload_and_hotspot_are_validated_before_retention() {
        for i in [image(0,1,0,0,vec![]),image(513,1,0,0,vec![0;513]),image(1,1,1,0,vec![0]),image(1,1,0,1,vec![0]),image(2,1,0,0,vec![0])] { assert!(NativeImage::convert(&i).is_err()); }
        assert!(NativeImage::convert(&image(512,1,511,0,vec![0;512])).is_ok());
    }
    #[test]
    fn shape_identity_uses_content_and_hotspot_not_server_serial() {
        let dir=Dir::new();let (mut assets,_)=Assets::create(&dir.0,source().bounds,Limits::default()).unwrap();
        let mut i=image(2,1,0,0,vec![0xff000000;2]);
        assert_eq!(assets.admit(&i).unwrap().0,"shape-000");i.cursor_serial=99;
        assert_eq!(assets.admit(&i).unwrap().0,"shape-000");i.cursor_image[0]=0xff123456;
        assert_eq!(assets.admit(&i).unwrap().0,"shape-001");i.xhot=1;
        assert_eq!(assets.admit(&i).unwrap().0,"shape-002");
        assert_eq!(assets.shapes.len(),3);
        assert!(Assets::create(&dir.0,source().bounds,Limits::default()).is_err());
    }
    #[test]
    fn every_image_budget_admits_n_and_rejects_n_plus_one() {
        for field in 0..3 {
            let dir=Dir::new();let mut limits=Limits::default();
            let first=CursorShape{id:"shape-000".into(),path:"cursor/shapes/shape-000.pam".into(),width:1,height:1,hotspot_x:0,hotspot_y:0};
            match field {0=>limits.shapes=1,1=>limits.decoded=4,_=>limits.index=index_bytes(source().bounds,vec![first]).unwrap().len() as u64}
            let (mut a,_)=Assets::create(&dir.0,source().bounds,limits).unwrap();
            assert!(a.admit(&image(1,1,0,0,vec![0xff000000])).is_ok());
            assert!(a.admit(&image(1,1,0,0,vec![0xffffffff])).is_err());
            assert_eq!(a.shapes.len(),1);assert_eq!(a.decoded,4);
        }
    }
    #[test]
    fn raw_schema_requires_canonical_ns_and_every_observed_field() {
        let valid=r#"{"beforeNs":"0","afterNs":"1","state":{"kind":"observed","x":0.5,"y":-0.5,"visible":true,"shapeId":null,"root":1,"originX":-100,"originY":20,"discontinuity":true}}"#;
        let v:serde_json::Value=serde_json::from_str(valid).unwrap();
        assert!(serde_json::from_str::<RawRecord>(valid).is_ok());
        for key in ["x","y","visible","shapeId","root","originX","originY","discontinuity"] {let mut v=v.clone();v["state"].as_object_mut().unwrap().remove(key);assert!(serde_json::from_value::<RawRecord>(v).is_err(),"{key}");}
        for ns in ["-1","+1","01","1.0"," 1","170141183460469231731687303715884105728"] {let mut v=v.clone();v["beforeNs"]=ns.into();assert!(serde_json::from_value::<RawRecord>(v).is_err());}
        for bad in [valid.replace("\"root\":1","\"root\":1,\"root\":2"),valid.replace("\"shapeId\":null","\"shapeId\":\"../bad\""),valid.replace("\"x\":0.5","\"x\":1e999"),valid.replace("\"root\":1","\"other\":1,\"root\":1")] {assert!(serde_json::from_str::<RawRecord>(&bad).is_err());}
        assert_eq!(serde_json::to_string(&row(0,1)).unwrap(),r#"{"beforeNs":"0","afterNs":"1","state":{"kind":"barrier"}}"#);
    }
    #[test]
    fn raw_commit_counts_full_lines_and_retains_every_fresh_observation() {
        let mut raw=RawWriter::new(Vec::new(),Limits::default());
        for i in 0..10 {raw.append(&row(i,i+1)).unwrap();}
        assert_eq!(raw.records,10);assert_eq!(raw.bytes,raw.writer.len() as u64);
        for line in raw.writer.split(|b|*b==b'\n').filter(|l|!l.is_empty()) {assert!(serde_json::from_slice::<RawRecord>(line).is_ok());}
    }
    #[test]
    fn raw_n_n_plus_one_limits_include_barriers_and_newline() {
        let len=serde_json::to_vec(&row(0,1)).unwrap().len()+1;
        for field in 0..3 {
            let mut limits=Limits::default();match field {0=>limits.rows=1,1=>limits.raw_bytes=len as u64,_=>limits.line=len}
            let mut raw=RawWriter::new(Vec::new(),limits);raw.append(&row(0,1)).unwrap();
            if field<2 {assert!(raw.append(&row(1,2)).is_err());} else {assert!(raw.append(&row(10,11)).is_err());}
            assert_eq!((raw.records,raw.bytes),(1,len as u64));
        }
    }
    struct Partial {bytes:Vec<u8>, remaining:usize}
    impl Write for Partial {
        fn write(&mut self,b:&[u8])->std::io::Result<usize> {if self.remaining==0 {return Err(std::io::ErrorKind::StorageFull.into());}let n=self.remaining.min(b.len());self.bytes.extend_from_slice(&b[..n]);self.remaining-=n;Ok(n)}
        fn flush(&mut self)->std::io::Result<()> {Ok(())}
    }
    #[test]
    fn partial_tail_never_advances_commit_and_cannot_resume() {
        let len=serde_json::to_vec(&row(0,1)).unwrap().len()+1;
        let mut raw=RawWriter::new(Partial{bytes:vec![],remaining:len+3},Limits::default());
        raw.append(&row(0,1)).unwrap();assert!(raw.append(&row(1,2)).is_err());assert!(raw.append(&row(2,3)).is_err());
        assert_eq!((raw.records,raw.bytes),(1,len as u64));assert_eq!(raw.writer.bytes.len(),len+3);
    }
    fn geometry(x:i32,y:i32)->Geometry {Geometry{root:1,x,y,width:100,height:50,viewable:true,same_screen:true}}
    #[test]
    fn signed_geometry_overlap_margin_and_arrow_fallback() {
        let mut g=Continuity::default();let s=source();let p=geometry(-100,20);
        let r=g.observe(s,1,p,p,-101,20,None,true);
        assert!(matches!(r,RawState::Observed{x,visible:true,shape_id:None,discontinuity:true,..} if x == -0.01));
        let r=g.observe(s,1,p,p,-3000,20,None,true);
        assert!(matches!(r,RawState::Observed{x:0.0,y:0.0,visible:false,..}));
        let r=g.observe(s,1,p,p,-100,20,Some(("shape-000".into(),(1,1,0,0))),false);
        assert!(matches!(r,RawState::Observed{visible:false,..}));
    }
    #[test]
    fn geometry_barriers_break_continuity_and_window_dimensions_stay_fixed() {
        let mut g=Continuity::default();let mut s=source();s.window=Some(2);let p=geometry(-100,20);
        assert!(matches!(g.observe(s,1,p,p,-50,20,None,true),RawState::Observed{discontinuity:true,..}));
        assert!(matches!(g.observe(s,1,p,p,-50,20,None,true),RawState::Observed{discontinuity:false,..}));
        for bad in [Geometry{root:2,..p},Geometry{same_screen:false,..p},Geometry{viewable:false,..p},Geometry{width:99,..p},Geometry{x:2,..p}] {
            assert_eq!(g.observe(s,1,p,bad,-50,20,None,true),RawState::Barrier {});
            assert!(matches!(g.observe(s,1,p,p,-50,20,None,true),RawState::Observed{discontinuity:true,..}));
        }
        let moved=Geometry{x:50,width:200,..p};
        assert!(matches!(g.observe(s,1,moved,moved,100,20,None,true),RawState::Observed{x:0.5,discontinuity:true,..}));
    }
    #[test]
    fn bracket_rejects_reversal_and_twenty_ms_plus_one() {
        assert_eq!(midpoint(0,20_000_000),Some(10_000_000));assert_eq!(midpoint(1,0),None);assert_eq!(midpoint(0,20_000_001),None);assert_eq!(midpoint(-1,1),None);
    }
    #[test]
    fn display_parser_admits_only_local_socket_instructions() {
        for d in ["host:0","tcp/localhost:0","localhost:0"] {assert!(local_address(Some(d)).is_err());}
        for d in [":98","unix/:98","unix/host:98.1"] {let (addr,_,_)=local_address(Some(d)).unwrap();assert_eq!(addr,ConnectAddress::Socket("/tmp/.X11-unix/X98".into()));}
    }
    #[derive(Clone,Copy)]
    enum Peer { SetupStall, ReplyStall, Missing, Disconnect, Healthy, InvalidImage, ImageError, WrongRoot, OtherScreen, SlowImage }
    fn fake(peer:Peer)->(UnixStream,JoinHandle<()>,Receiver<()>) {
        use x11rb::protocol::xproto::*;
        use x11rb::x11_utils::Serialize as XSerialize;
        let (client,mut server)=UnixStream::pair().unwrap();let (seen_tx,seen)=mpsc::sync_channel(1);
        let handle=thread::spawn(move || {
            server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut header=[0;12];
            if let Err(e)=server.read_exact(&mut header) {assert_eq!(e.kind(),std::io::ErrorKind::UnexpectedEof);return;}
            let n=u16::from_ne_bytes([header[6],header[7]]) as usize;let m=u16::from_ne_bytes([header[8],header[9]]) as usize;
            let mut auth=vec![0;(n+3)/4*4+(m+3)/4*4];server.read_exact(&mut auth).unwrap();
            if matches!(peer,Peer::SetupStall) {seen_tx.send(()).unwrap();assert_eq!(server.read(&mut [0]).unwrap(),0);return;}
            let setup=Setup{status:1,protocol_major_version:11,resource_id_base:0x200000,resource_id_mask:0x1fffff,maximum_request_length:65535,
                roots:vec![Screen{root:1,width_in_pixels:1920,height_in_pixels:1080,root_depth:24,..Default::default()}],..Default::default()};
            let mut bytes=XSerialize::serialize(&setup);let len=((bytes.len()-8)/4) as u16;bytes[6..8].copy_from_slice(&len.to_ne_bytes());server.write_all(&bytes).unwrap();
            let mut sequence=0u16;
            loop {
                let mut h=[0;4];if let Err(e)=server.read_exact(&mut h) {assert_eq!(e.kind(),std::io::ErrorKind::UnexpectedEof);break;}
                let mut rest=vec![0;usize::from(u16::from_ne_bytes([h[2],h[3]]))*4-4];server.read_exact(&mut rest).unwrap();sequence+=1;
                let mut bytes=match (h[0],h[1]) {
                    (98,_)=>{let n=usize::from(u16::from_ne_bytes([rest[0],rest[1]]));let is_xfixes=&rest[4..4+n]==b"XFIXES";
                        XSerialize::serialize(&QueryExtensionReply{sequence,length:0,present:is_xfixes&&!matches!(peer,Peer::Missing),major_opcode:200,first_event:80,first_error:150}).to_vec()}
                    (200,0)=>XSerialize::serialize(&xfixes::QueryVersionReply{sequence,length:0,major_version:2,minor_version:0}).to_vec(),
                    (200,4)=>{
                        seen_tx.try_send(()).ok();
                        if matches!(peer,Peer::ReplyStall) {assert_eq!(server.read(&mut [0]).unwrap(),0);return;}
                        if matches!(peer,Peer::Disconnect) {return;}
                        if matches!(peer,Peer::ImageError) {let mut error=[0u8;32];error[1]=2;error[2..4].copy_from_slice(&sequence.to_ne_bytes());error[8..10].copy_from_slice(&4u16.to_ne_bytes());error[10]=200;server.write_all(&error).unwrap();continue;}
                        if matches!(peer,Peer::SlowImage) {thread::sleep(Duration::from_millis(25));}
                        let mut i=image(1,1,0,0,vec![0xff123456]);i.sequence=sequence;i.length=1;i.x=10;i.y=20;if matches!(peer,Peer::InvalidImage) {i.x=-50;i.cursor_image[0]=0x01020000;thread::sleep(Duration::from_millis(25));}XSerialize::serialize(&i).to_vec()
                    }
                    (38,_)=>XSerialize::serialize(&QueryPointerReply{same_screen:!matches!(peer,Peer::OtherScreen),sequence,length:0,root:if matches!(peer,Peer::WrongRoot){2}else{1},child:0,root_x:10,root_y:20,win_x:10,win_y:20,mask:0u16.into()}).to_vec(),
                    other=>panic!("unexpected fake request {other:?}")
                };
                if bytes.len()<32 {bytes.resize(32,0);}if let Err(e)=server.write_all(&bytes) {assert!(matches!(e.kind(),std::io::ErrorKind::BrokenPipe|std::io::ErrorKind::ConnectionReset));break;}
            }
        });
        (client,handle,seen)
    }
    fn sampler(dir:&Dir,peer:Peer)->(Sampler,JoinHandle<()>,Receiver<()>) {
        let (socket,peer,seen)=fake(peer);
        let sampler=Sampler::spawn(&dir.0,source(),move |shared| {
            let (stream,_)=DefaultStream::from_unix_stream(socket).map_err(|_|"Test connection failed")?;
            setup_stream(shared,stream,0,vec![],vec![]).map(|c|(c,0))
        }).unwrap();
        (sampler,peer,seen)
    }
    #[test]
    fn startup_timeout_interrupts_stalled_setup_and_joins_owned_peer() {
        let dir=Dir::new();let (mut s,p,seen)=sampler(&dir,Peer::SetupStall);
        seen.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(s.ready(Duration::from_millis(20)).is_err());assert!(s.worker.is_none());p.join().unwrap();
        assert!(s.shared.lock().socket.is_none());
    }
    #[test]
    fn stop_before_socket_publication_is_latched_and_joined() {
        let dir=Dir::new();let (socket,p,_)=fake(Peer::SetupStall);let (release,wait)=mpsc::sync_channel(1);
        let mut s=Sampler::spawn(&dir.0,source(),move |shared| {wait.recv().unwrap();let (stream,_)=DefaultStream::from_unix_stream(socket).unwrap();setup_stream(shared,stream,0,vec![],vec![]).map(|c|(c,0))}).unwrap();
        s.request_stop();release.send(()).unwrap();assert!(s.ready(Duration::from_secs(1)).is_err());assert!(s.worker.is_none());p.join().unwrap();
    }
    #[test]
    fn prepared_drop_and_repeated_running_stop_close_and_join() {
        for cycle in 0..4 {let dir=Dir::new();let (mut s,p,seen)=sampler(&dir,Peer::Healthy);s.ready(Duration::from_secs(1)).unwrap();
            let shared=Arc::clone(&s.shared);
            if cycle==0 {assert!(matches!(seen.try_recv(),Err(mpsc::TryRecvError::Empty)));drop(s);} else {
                let epoch=Instant::now();s.start(epoch).unwrap();assert!(s.start(epoch).is_err());seen.recv_timeout(Duration::from_secs(1)).unwrap();
                thread::sleep(Duration::from_millis(30));s.request_stop();s.request_stop();let outcome=s.finish();assert!(outcome.trustworthy_prefix);assert!(outcome.records>0);assert!(outcome.cutoff_ns.is_some());
            }
            assert!(shared.lock().socket.is_none());p.join().unwrap();
        }
    }
    #[test]
    fn stalled_post_setup_reply_is_interrupted_without_cached_refresh() {
        let dir=Dir::new();let (mut s,p,seen)=sampler(&dir,Peer::ReplyStall);s.ready(Duration::from_secs(1)).unwrap();s.start(Instant::now()).unwrap();
        seen.recv_timeout(Duration::from_secs(1)).unwrap();let out=s.finish();p.join().unwrap();
        assert_eq!((out.records,out.committed_bytes),(0,0));assert!(out.cutoff_ns.is_some());assert!(out.trustworthy_prefix);
    }
    #[test]
    fn missing_extension_connection_loss_and_panic_have_controlled_results() {
        let dir=Dir::new();let (mut s,p,_)=sampler(&dir,Peer::Missing);assert!(s.ready(Duration::from_secs(1)).is_err());assert!(s.worker.is_none());p.join().unwrap();
        let dir=Dir::new();let (mut s,p,_)=sampler(&dir,Peer::Disconnect);s.ready(Duration::from_secs(1)).unwrap();s.start(Instant::now()).unwrap();
        thread::sleep(Duration::from_millis(30));let out=s.finish();p.join().unwrap();assert!(out.degraded);assert_eq!(out.records,0);assert!(out.reason.unwrap().len()<100);
        let dir=Dir::new();let (socket,p,_)=fake(Peer::Healthy);
        let mut s=Sampler::spawn(&dir.0,source(),move |shared| {
            let (stream,_)=DefaultStream::from_unix_stream(socket).unwrap();
            let _connection=setup_stream(shared,stream,0,vec![],vec![]).unwrap();
            panic!("test worker panic after socket publication")
        }).unwrap();
        assert!(s.ready(Duration::from_secs(1)).is_err());assert!(s.shared.lock().socket.is_none());
        let out=s.finish();p.join().unwrap();assert!(!out.trustworthy_prefix);assert_eq!(out.committed_bytes,0);
    }
    #[test]
    fn cutoff_is_immutable_and_late_rows_cannot_commit() {
        let shared=Shared::default();shared.lock().epoch=Some(Instant::now());shared.stop();let cutoff=shared.lock().cutoff_ns;
        thread::sleep(Duration::from_millis(2));shared.stop();assert_eq!(shared.lock().cutoff_ns,cutoff);
        let mut raw=RawWriter::new(Vec::new(),Limits::default());assert!(!commit(&shared,&mut raw,&row(0,1)).unwrap());assert_eq!(raw.records,0);
    }

    #[test]
    fn invalid_image_uses_fresh_query_pointer_position_and_bracket() {
        for mode in [Peer::InvalidImage,Peer::ImageError] {
        let dir=Dir::new();let (mut s,p,seen)=sampler(&dir,mode);s.ready(Duration::from_secs(1)).unwrap();s.start(Instant::now()).unwrap();
        seen.recv_timeout(Duration::from_secs(1)).unwrap();thread::sleep(Duration::from_millis(60));let out=s.finish();p.join().unwrap();
        assert!(out.records>0);let data=fs::read(&out.raw_path).unwrap();let first=data.split(|b|*b==b'\n').next().unwrap();let r:RawRecord=serde_json::from_slice(first).unwrap();
        assert!(matches!(r.state,RawState::Observed{x:1.1,y:0.0,shape_id:None,..}));assert!(midpoint(r.before_ns,r.after_ns).is_some());assert!(out.degraded);
        }
    }
    #[test]
    fn result_channel_loss_does_not_infer_a_prefix_from_disk() {
        let dir=Dir::new();let (mut s,p,seen)=sampler(&dir,Peer::Healthy);s.ready(Duration::from_secs(1)).unwrap();s.start(Instant::now()).unwrap();
        seen.recv_timeout(Duration::from_secs(1)).unwrap();thread::sleep(Duration::from_millis(30));s.result=mpsc::sync_channel(1).1;
        let out=s.finish();p.join().unwrap();assert!(!out.trustworthy_prefix);assert_eq!(out.committed_bytes,0);
    }
    fn consumer_accept(root:&Path,bounds:CaptureBounds,shapes:&[CursorShape])->Arc<focusflow_core::cursor::PreparedCursor> {
        use focusflow_core::project::*;
        let duration_us=5_000_000;
        let index=CursorIndex{schema_version:1,width:bounds.width,height:bounds.height,duration_us,samples_path:"cursor/samples.ndjson".into(),shapes:shapes.to_vec()};
        fs::write(root.join("cursor/index.json"),serde_json::to_vec(&index).unwrap()).unwrap();
        let samples=shapes.iter().enumerate().map(|(i,s)|format!("{}\n",serde_json::json!({"atUs":i*100,"x":0.5,"y":0.5,"visible":true,"shapeId":s.id,"discontinuity":false}))).collect::<String>();
        fs::write(root.join("cursor/samples.ndjson"),samples).unwrap();
        let doc=ProjectDocument{schema_version:3,project_id:"fixture".into(),revision:0,frame_style:Default::default(),
            video:VideoAsset{cursor:CursorMode::Separate,path:"screen.mp4".into(),width:bounds.width,height:bounds.height,duration_us},
            cursor:Some(CursorAsset{index_path:"cursor/index.json".into()}),cursor_settings:CursorSettings{enabled:true,style:CursorStyle::Native,..Default::default()},
            microphone:None,timing:Timing{provenance:TimingProvenance::LegacyUnknown,audio_offset_us:None},camera:CameraDefaults{pan_transition_us:0,settle_us:0},
            clips:vec![Clip{id:"clip-0".into(),in_us:0,out_us:duration_us}],zooms:vec![],clicks:vec![],warnings:vec![]};
        fs::write(root.join("project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
        focusflow_core::cursor::prepare_cursor(root,&doc,&std::sync::atomic::AtomicBool::new(false)).unwrap().unwrap()
    }
    #[test]
    fn emitted_pam_is_accepted_by_real_schema_three_consumer() {
        let dir=Dir::new();let (mut assets,_)=Assets::create(&dir.0,source().bounds,Limits::default()).unwrap();
        assets.admit(&image(4,1,0,0,vec![0x00ffffff,0x01010001,0x80402080,0xff123456])).unwrap();
        let prepared=consumer_accept(&dir.0,source().bounds,&assets.shapes);
        assert_eq!(&*prepared.shapes[0].pixels,&[0,0,0,0,255,0,255,1,128,64,255,128,18,52,86,255]);
        assert_eq!(prepared.sample_count(),1);
    }
    #[test]
    fn preexisting_cursor_symlink_and_shape_files_are_not_followed_or_overwritten() {
        let dir=Dir::new();let outside=Dir::new();std::os::unix::fs::symlink(&outside.0,dir.0.join("cursor")).unwrap();
        assert!(Assets::create(&dir.0,source().bounds,Limits::default()).is_err());assert_eq!(fs::read_dir(&outside.0).unwrap().count(),0);
        let dir=Dir::new();let (mut assets,_)=Assets::create(&dir.0,source().bounds,Limits::default()).unwrap();
        fs::write(dir.0.join("cursor/shapes/shape-000.pam"),b"existing").unwrap();
        assert!(assets.admit(&image(1,1,0,0,vec![0xff000000])).is_err());assert!(assets.degraded);assert!(assets.shapes.is_empty());
        assert_eq!(fs::read(dir.0.join("cursor/shapes/shape-000.pam")).unwrap(),b"existing");
    }
    #[test]
    #[ignore = "requires root-owned authenticated DISPLAY=:98 and generated-only evidence directory"]
    fn native_sampler_fixture() {
        use x11rb::protocol::xproto::*;
        assert_eq!(std::env::var("DISPLAY").as_deref(),Ok(":98"));
        assert_eq!(std::env::var("FOCUSFLOW_ISOLATED_X11").as_deref(),Ok("1"));
        let evidence=PathBuf::from(std::env::var_os("FOCUSFLOW_NATIVE_EVIDENCE").expect("owned evidence directory"));
        let session=evidence.join("sampler-session");fs::create_dir(&session).unwrap();
        let (c,screen)=x11rb::connect(None).unwrap();let root=c.setup().roots[screen].root;
        let window=c.generate_id().unwrap();
        c.create_window(x11rb::COPY_DEPTH_FROM_PARENT,window,root,120,100,160,120,0,WindowClass::INPUT_OUTPUT,0,&CreateWindowAux::new().background_pixel(0x446688)).unwrap().check().unwrap();
        c.map_window(window).unwrap().check().unwrap();c.flush().unwrap();
        let source=Source{bounds:CaptureBounds{x:120,y:100,width:160,height:120},window:Some(window)};
        let mut sampler=Sampler::prepare(&session,source).unwrap();sampler.start(Instant::now()).unwrap();
        for (x,y) in [(10,10),(40,30),(100,70)] {
            c.warp_pointer(0u32,window,0,0,0,0,x,y).unwrap().check().unwrap();c.flush().unwrap();thread::sleep(Duration::from_millis(70));
        }
        c.configure_window(window,&ConfigureWindowAux::new().x(180).y(130).width(200).height(160)).unwrap().check().unwrap();c.flush().unwrap();thread::sleep(Duration::from_millis(70));
        c.warp_pointer(0u32,window,0,0,0,0,30,30).unwrap().check().unwrap();c.flush().unwrap();thread::sleep(Duration::from_millis(70));
        c.configure_window(window,&ConfigureWindowAux::new().width(80).height(60)).unwrap().check().unwrap();c.flush().unwrap();thread::sleep(Duration::from_millis(70));
        c.unmap_window(window).unwrap().check().unwrap();c.flush().unwrap();thread::sleep(Duration::from_millis(70));
        c.destroy_window(window).unwrap().check().unwrap();c.flush().unwrap();thread::sleep(Duration::from_millis(70));
        let outcome=sampler.finish();assert!(outcome.trustworthy_prefix);assert!(outcome.records>5);assert!(!outcome.shapes.is_empty());
        let raw=fs::read(&outcome.raw_path).unwrap();assert_eq!(raw.len() as u64,outcome.committed_bytes);
        let rows=raw.split(|b|*b==b'\n').filter(|b|!b.is_empty()).map(|b|serde_json::from_slice::<RawRecord>(b).unwrap()).collect::<Vec<_>>();
        assert_eq!(rows.len(),outcome.records);assert!(rows.iter().any(|r|matches!(r.state,RawState::Barrier {})));
        assert!(rows.iter().any(|r|matches!(r.state,RawState::Observed{origin_x:180,discontinuity:true,..})));
        assert!(rows.iter().filter(|r|matches!(r.state,RawState::Observed{..})).all(|r|midpoint(r.before_ns,r.after_ns).is_some()));
        assert!(rows.iter().all(|r|r.after_ns<=outcome.cutoff_ns.unwrap()));
        let prepared=consumer_accept(&session,source.bounds,&outcome.shapes);assert_eq!(prepared.shapes.len(),outcome.shapes.len());
        fs::write(evidence.join("sampler-outcome.json"),serde_json::to_vec_pretty(&outcome).unwrap()).unwrap();
    }

    #[test]
    fn local_default_stream_preserves_filesystem_and_abstract_socket_support() {
        use std::os::{linux::net::SocketAddrExt,unix::net::{UnixListener,SocketAddr}};
        for abstract_socket in [false,true] {
            let dir=Dir::new();let path=dir.0.join("owned-peer");
            let listener=if abstract_socket {UnixListener::bind_addr(&SocketAddr::from_abstract_name(path.as_os_str().as_encoded_bytes()).unwrap()).unwrap()} else {UnixListener::bind(&path).unwrap()};
            let (stream,_)=DefaultStream::connect(&ConnectAddress::Socket(path.to_str().unwrap().into())).unwrap();
            let (mut peer,_)=listener.accept().unwrap();
            let shutdown=UnixStream::from(AsFd::as_fd(&stream).try_clone_to_owned().unwrap());shutdown.shutdown(Shutdown::Both).unwrap();
            assert_eq!(peer.read(&mut [0]).unwrap(),0);
        }
    }
    #[test]
    fn raw_exhaustion_closes_telemetry_and_never_resumes_or_moves_cutoff() {
        let shared=Shared::default();shared.lock().epoch=Some(Instant::now());
        let mut limits=Limits::default();limits.rows=1;let mut raw=RawWriter::new(Vec::new(),limits);
        assert!(commit(&shared,&mut raw,&row(0,1)).unwrap());assert!(commit(&shared,&mut raw,&row(1,2)).is_err());
        let cutoff=shared.lock().cutoff_ns;assert!(cutoff.is_some());assert!(shared.lock().stopped);
        assert!(!commit(&shared,&mut raw,&row(2,3)).unwrap());assert_eq!(raw.records,1);assert_eq!(shared.lock().cutoff_ns,cutoff);
    }
    #[test]
    fn raw_strict_tag_barrier_duplicate_and_type_boundaries() {
        for bad in [
            r#"{"beforeNs":"0","beforeNs":"1","afterNs":"1","state":{"kind":"barrier"}}"#,
            r#"{"beforeNs":"0","afterNs":"1","state":{"kind":"barrier","kind":"barrier"}}"#,
            r#"{"beforeNs":"0","afterNs":"1","state":{"kind":"barrier","x":0}}"#,
            r#"{"beforeNs":0,"afterNs":"1","state":{"kind":"barrier"}}"#,
            r#"{"beforeNs":"0","afterNs":"1","extra":true,"state":{"kind":"barrier"}}"#,
        ] {assert!(serde_json::from_str::<RawRecord>(bad).is_err(),"{bad}");}
        assert!(serde_json::to_vec(&row(-1,0)).is_err());
    }

    #[test]
    fn pointer_root_screen_and_slow_reply_failures_emit_only_fresh_barriers() {
        for mode in [Peer::WrongRoot,Peer::OtherScreen,Peer::SlowImage] {
            let dir=Dir::new();let (mut s,p,seen)=sampler(&dir,mode);s.ready(Duration::from_secs(1)).unwrap();s.start(Instant::now()).unwrap();
            seen.recv_timeout(Duration::from_secs(1)).unwrap();thread::sleep(Duration::from_millis(60));let out=s.finish();p.join().unwrap();
            assert!(out.records>0);assert!(out.shapes.is_empty());let data=fs::read(&out.raw_path).unwrap();
            for line in data.split(|b|*b==b'\n').filter(|b|!b.is_empty()) {let row:RawRecord=serde_json::from_slice(line).unwrap();assert_eq!(row.state,RawState::Barrier {});assert!(row.after_ns<=out.cutoff_ns.unwrap());}
        }
    }

    #[test]
    fn stop_discards_successful_reply_arriving_during_flush_grace() {
        let dir=Dir::new();let (mut s,p,seen)=sampler(&dir,Peer::SlowImage);s.ready(Duration::from_secs(1)).unwrap();s.start(Instant::now()).unwrap();
        seen.recv_timeout(Duration::from_secs(1)).unwrap();s.request_stop();let cutoff=s.shared.lock().cutoff_ns;let out=s.finish();p.join().unwrap();
        assert_eq!((out.records,out.committed_bytes),(0,0));assert_eq!(out.cutoff_ns,cutoff);assert!(out.trustworthy_prefix);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FinalizedCursor {
    Separate { degraded: Option<&'static str> },
    Absent { reason: &'static str },
}
#[derive(Clone, Serialize)]
#[serde(rename_all="camelCase")]
struct FinalRow { at_us:u64, x:f64, y:f64, visible:bool, shape_id:Option<String>, discontinuity:bool }
impl FinalRow {
    fn hidden(at_us:u64)->Self {Self{at_us,x:0.0,y:0.0,visible:false,shape_id:None,discontinuity:true}}
    fn bytes(&self)->RecorderResult<Vec<u8>> {
        let mut bytes=serde_json::to_vec(self).map_err(|_|error("Could not serialize final cursor samples"))?;bytes.push(b'\n');Ok(bytes)
    }
}
// One pending microsecond permits last-state consolidation without retaining raw history.
struct FinalWriter<W> {
    writer:W, pending:Option<(FinalRow,bool)>, ordinary_rows:usize, ordinary_bytes:u64,
    rows:usize, bytes:u64, limits:Limits,
}
impl<W:Write> FinalWriter<W> {
    fn new(writer:W,limits:Limits)->Self {Self{writer,pending:None,ordinary_rows:0,ordinary_bytes:0,rows:0,bytes:0,limits}}
    fn admission(&self,row:&FinalRow,ordinary:bool)->RecorderResult<bool> {
        let same=self.pending.as_ref().filter(|(p,_)|p.at_us==row.at_us);
        let mut merged=row.clone();merged.discontinuity|=same.is_some_and(|(p,_)|p.discontinuity);
        let bytes=merged.bytes()?.len() as u64;
        if bytes>self.limits.line as u64 {return Ok(false);}
        let previous=match same {Some((p,true))=>p.bytes()?.len() as u64,_=>0};
        let ordinary=ordinary||same.is_some_and(|(_,o)|*o);
        let count=self.ordinary_rows+usize::from(ordinary&&previous==0);
        let size=self.ordinary_bytes-previous+if ordinary {bytes} else {0};
        Ok(!ordinary||(count<=self.limits.rows.saturating_sub(3)&&size<=self.limits.raw_bytes.saturating_sub(3*self.limits.line as u64)))
    }
    fn push(&mut self,mut row:FinalRow,ordinary:bool)->RecorderResult<()> {
        if let Some((previous,was_ordinary))=&self.pending {
            if previous.at_us==row.at_us {
                row.discontinuity|=previous.discontinuity;
                if *was_ordinary {self.ordinary_rows-=1;self.ordinary_bytes-=previous.bytes()?.len() as u64;}
                let ordinary=ordinary||*was_ordinary;
                self.pending=None;return self.queue(row,ordinary);
            }
        }
        self.flush_pending()?;self.queue(row,ordinary)
    }
    fn queue(&mut self,row:FinalRow,ordinary:bool)->RecorderResult<()> {
        if ordinary {self.ordinary_rows+=1;self.ordinary_bytes+=row.bytes()?.len() as u64;}
        self.pending=Some((row,ordinary));Ok(())
    }
    fn flush_pending(&mut self)->RecorderResult<()> {
        if let Some((row,_))=self.pending.take() {
            let bytes=row.bytes()?;
            if bytes.len()>self.limits.line||self.rows>=self.limits.rows||self.bytes+bytes.len() as u64>self.limits.raw_bytes {return Err(error("Final cursor synthetic reserve is too small"));}
            self.writer.write_all(&bytes).map_err(|_|error("Could not write final cursor samples; retain this session for recovery"))?;
            self.rows+=1;self.bytes+=bytes.len() as u64;
        }
        Ok(())
    }
}
struct Conversion { present:bool, degraded:Option<&'static str> }
struct Endpoint { ns_numerator:u128, denominator:u128, duration_us:u64 }
impl Endpoint {
    fn new(time:focusflow_core::ffmpeg::PacketTime)->Option<Self> {
        if time.ticks==0||time.numerator==0||time.denominator==0 {return None;}
        let numerator=u128::from(time.ticks).checked_mul(time.numerator.into())?;
        let duration_us=u64::try_from(numerator.checked_mul(1_000_000)?/u128::from(time.denominator)).ok()?;
        if duration_us==0||duration_us>focusflow_core::project::MAX_DURATION_US {return None;}
        Some(Self{ns_numerator:numerator.checked_mul(1_000_000_000)?,denominator:time.denominator.into(),duration_us})
    }
    fn at(&self,ns:i128)->Option<u64> {
        let ns=u128::try_from(ns).ok()?;
        if ns.checked_mul(self.denominator)?>=self.ns_numerator {return None;}
        let us=u64::try_from(ns/1000).ok()?;(us<self.duration_us).then_some(us)
    }
}
fn convert<R:std::io::Read,W:Write>(reader:R,output:&mut FinalWriter<W>,source:Source,outcome:&Outcome,zero:i128,end:&Endpoint,raw_limits:Limits)->RecorderResult<Conversion> {
    use std::io::{BufRead,BufReader,Read};
    let mut reader=BufReader::new(reader.take(outcome.committed_bytes.min(raw_limits.raw_bytes)));
    let mut line=Vec::with_capacity(raw_limits.line+1);
    let mut bytes=0u64;let mut records=0usize;let mut last_end=None;let mut root=None;
    let mut predecessor:Option<(i128,FinalRow)>=None;let mut seeded=false;let mut expiry=None;
    let mut present=false;let mut reentry=true;
    let mut degraded=if outcome.degraded {Some(outcome.reason.unwrap_or("Cursor capture was degraded"))} else {None};
    let mut cutoff=outcome.cutoff_ns.and_then(|n|n.checked_sub(zero));
    let mut invalid=false;let mut budget=false;let mut admitted_end=None;
    loop {
        line.clear();
        let count=match (&mut reader).take(raw_limits.line as u64+1).read_until(b'\n',&mut line) {Ok(n)=>n,Err(_)=>{invalid=true;break;}};
        if count==0 {break;}
        if count>raw_limits.line||!line.ends_with(b"\n")||records>=outcome.records||records>=raw_limits.rows {invalid=true;break;}
        let row=match serde_json::from_slice::<RawRecord>(&line) {Ok(row)=>row,Err(_)=>{invalid=true;break;}};
        if row.before_ns<0||row.after_ns<row.before_ns||last_end.is_some_and(|last|row.before_ns<last) {invalid=true;break;}
        let (time,mut final_row,observed,compatible)=match row.state {
            RawState::Observed{x,y,visible,shape_id,root:next_root,origin_x,origin_y,discontinuity}=>{
                let Some(mid)=midpoint(row.before_ns,row.after_ns) else {invalid=true;break;};
                if next_root==0||root.is_some_and(|root|root!=next_root)||
                    !(-2048.0..=source.bounds.width as f64+2048.0).contains(&(x*source.bounds.width as f64))||
                    !(-2048.0..=source.bounds.height as f64+2048.0).contains(&(y*source.bounds.height as f64))||
                    shape_id.as_ref().is_some_and(|id|!outcome.shapes.iter().any(|s|&s.id==id)) {invalid=true;break;}
                let compatible=(origin_x,origin_y)==(source.bounds.x,source.bounds.y);
                if source.window.is_none()&&!compatible {invalid=true;break;}
                root=Some(next_root);
                if visible&&shape_id.is_none() {degraded.get_or_insert("Native cursor image is unavailable; choose Arrow explicitly");}
                (mid,FinalRow{at_us:0,x,y,visible,shape_id,discontinuity},true,compatible)
            },
            RawState::Barrier{}=>(row.before_ns,FinalRow::hidden(0),false,false),
        };
        let Some(ns)=time.checked_sub(zero) else {invalid=true;break;};
        last_end=Some(row.after_ns);records+=1;bytes+=count as u64;
        if cutoff.is_some_and(|cutoff|ns>=cutoff||row.after_ns-zero>cutoff) {continue;}
        if ns<0 {
            predecessor=if observed&&compatible&&ns>=-100_000_000 {Some((ns,final_row))} else {None};continue;
        }
        if !seeded {
            seeded=true;
            if let Some((ns,mut seed))=predecessor.take() {
                seed.at_us=0;seed.discontinuity=true;
                if cutoff.is_none_or(|c|c>0) {output.push(seed,false)?;present=true;expiry=ns.checked_add(250_000_000);reentry=false;}
            }
        }
        let Some(at_us)=end.at(ns) else {continue;};final_row.at_us=at_us;
        final_row.discontinuity|=observed&&(reentry||expiry.is_some_and(|e|ns>e));
        // Test ordinary capacity before emitting a later synthetic expiry, so failure can close at the admitted prefix.
        if !output.admission(&final_row,true)? {budget=true;break;}
        if let Some(e)=expiry {
            if observed&&ns<=e {expiry=None;}
            else if e<=ns {if let Some(us)=end.at(e) {output.push(FinalRow::hidden(us),false)?;}expiry=None;reentry=true;}
        }
        if observed {final_row.discontinuity|=reentry;present=true;reentry=false;} else {degraded.get_or_insert("Cursor observation was interrupted");reentry=true;expiry=None;}
        output.push(final_row,true)?;admitted_end=Some(row.after_ns-zero);
    }
    if !budget&&(bytes!=outcome.committed_bytes||records!=outcome.records) {invalid=true;}
    if invalid||budget {
        degraded.get_or_insert(if budget {"Final cursor budget exhausted"} else {"Cursor committed prefix is invalid or incomplete"});
        let safe=if budget {admitted_end.or_else(||predecessor.as_ref().map(|(ns,_)|*ns))} else {last_end.and_then(|n|n.checked_sub(zero))}.unwrap_or(0);
        cutoff=Some(cutoff.map_or(safe,|n|n.min(safe)));
    }
    if !seeded {
        if let Some((ns,mut seed))=predecessor {
            if cutoff.is_none_or(|c|c>0) {seed.discontinuity=true;output.push(seed,false)?;present=true;expiry=ns.checked_add(250_000_000);}
        }
    }
    if let Some(e)=expiry {if cutoff.is_none_or(|c|e<c) {if let Some(us)=end.at(e) {output.push(FinalRow::hidden(us),false)?;}}}
    if let Some(c)=cutoff {if let Some(us)=end.at(c) {if present {degraded.get_or_insert("Cursor telemetry ended before the saved video");output.push(FinalRow::hidden(us),false)?;}}}
    output.flush_pending()?;
    Ok(Conversion{present,degraded})
}

pub(super) fn finalize(dir:&Path,source:Source,outcome:Outcome,clock:Option<&super::video_clock::AcceptedClock>)->RecorderResult<FinalizedCursor> {
    finalize_with(dir,source,outcome,clock,Limits::default(),Limits::default(),|_,_|Ok(()))
}
fn finalize_with(dir:&Path,source:Source,outcome:Outcome,clock:Option<&super::video_clock::AcceptedClock>,raw_limits:Limits,final_limits:Limits,
    mut hook:impl FnMut(&str,&Path)->std::io::Result<()>)->RecorderResult<FinalizedCursor> {
    let absent=|reason|Ok(FinalizedCursor::Absent{reason});
    let Some(clock)=clock else {return absent("No accepted video clock");};
    let Some(end)=Endpoint::new(clock.saved_endpoint) else {return absent("Video clock endpoint is not representable");};
    if !outcome.trustworthy_prefix||outcome.committed_bytes==0||outcome.records==0 {return absent("No trustworthy cursor observation prefix");}
    if source.bounds.width==0||source.bounds.height==0||clock.frame_zero_ns<0 {return absent("Cursor source or clock is invalid");}
    let raw=dir.join("cursor/raw.ndjson");
    if raw!=outcome.raw_path||dir.canonicalize().ok().as_deref()!=Some(dir)||
        fs::symlink_metadata(dir.join("cursor")).is_err()||
        fs::symlink_metadata(dir.join("cursor")).is_ok_and(|m|!m.is_dir()||m.file_type().is_symlink())||
        fs::symlink_metadata(&raw).is_err()||fs::symlink_metadata(&raw).is_ok_and(|m|!m.is_file()||m.file_type().is_symlink()) {return absent("Cursor telemetry path is missing or not owned");}
    let raw_file=match File::open(&raw) {Ok(file)=>file,Err(_)=>return absent("Cursor telemetry could not be read")};
    let _=raw_file.sync_all();
    let mut decoded=0usize;
    for (i,shape) in outcome.shapes.iter().enumerate() {
        if i>=MAX_SHAPES||shape.id!=format!("shape-{i:03}")||shape.path!=format!("cursor/shapes/{}.pam",shape.id)||
            !(1..=512).contains(&shape.width)||!(1..=512).contains(&shape.height)||shape.hotspot_x>=shape.width||shape.hotspot_y>=shape.height {return absent("Cursor shape descriptors are invalid");}
        decoded+=shape.width as usize*shape.height as usize*4;
        if decoded>MAX_NATIVE_BYTES {return absent("Cursor shape descriptors exceed the native budget");}
    }
    let index=CursorIndex{schema_version:1,width:source.bounds.width,height:source.bounds.height,duration_us:end.duration_us,samples_path:"cursor/samples.ndjson".into(),shapes:outcome.shapes.clone()};
    let index=serde_json::to_vec(&index).map_err(|_|error("Could not serialize final cursor index"))?;
    if index.len() as u64>final_limits.index {return Err(error("Final cursor index exceeds its byte budget; retain this session for recovery"));}
    let samples=dir.join("cursor/samples.ndjson");let index_path=dir.join("cursor/index.json");
    for path in [&samples,&index_path] {if fs::symlink_metadata(path).is_ok() {return Err(error("Final cursor assets already exist; retain this session and reload before recovery"));}}
    let temp=dir.join("cursor/.samples.finalizing.tmp");let index_temp=dir.join("cursor/.index.finalizing.tmp");
    let file=OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|_|error("Could not create owned final cursor sample temporary file"))?;
    let mut owns_index=false;
    let result=(|| {
        hook("samples-write",&temp).map_err(|_|error("Could not write final cursor samples; retain this session for recovery"))?;
        let mut output=FinalWriter::new(file,final_limits);
        let converted=convert(raw_file,&mut output,source,&outcome,clock.frame_zero_ns,&end,raw_limits)?;
        output.writer.flush().map_err(|_|error("Could not flush final cursor samples"))?;drop(output);
        if !converted.present {return absent("No representable cursor observation prefix");}
        hook("samples-rename",&temp).map_err(|_|error("Could not publish final cursor samples; retain this session for recovery"))?;
        if fs::symlink_metadata(&samples).is_ok() {return Err(error("Final cursor samples already exist; reload before recovery"));}
        fs::rename(&temp,&samples).map_err(|_|error("Could not publish final cursor samples; retain this session for recovery"))?;
        let mut file=OpenOptions::new().write(true).create_new(true).open(&index_temp).map_err(|_|error("Could not create owned final cursor index temporary file"))?;owns_index=true;
        file.write_all(&index).and_then(|_|file.flush()).map_err(|_|error("Could not write final cursor index; retain this session for recovery"))?;drop(file);
        hook("index-rename",&index_temp).map_err(|_|error("Could not publish final cursor index; retain this session for recovery"))?;
        if fs::symlink_metadata(&index_path).is_ok() {return Err(error("Final cursor index already exists; reload before recovery"));}
        fs::rename(&index_temp,&index_path).map_err(|_|error("Could not publish final cursor index; retain this session for recovery"))?;
        Ok(FinalizedCursor::Separate{degraded:converted.degraded})
    })();
    let _=fs::remove_file(&temp);if owns_index {let _=fs::remove_file(&index_temp);}result
}

#[cfg(test)]
mod finalize_tests;
