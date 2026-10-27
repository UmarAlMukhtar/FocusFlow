//! X11 request-clock evidence. Pipe receipt time is deliberately never an anchor.
use super::InteractionEvents;
use focusflow_core::ffmpeg::{PacketTime, ProbedVideoEvidence};
use serde::Serialize;
use serde_json::{json, Value};
use std::{fs, io::Read, path::Path, process::Command, sync::{mpsc, Mutex, OnceLock}, thread, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

const FORMAT: &str = "{fidx},{sidx},{n},{ni},{tbi},{ptsi},{tb},{pts}";
const LINE_LIMIT: usize = 512;
const EVIDENCE_LIMIT: usize = 64 * 1024;
const FRAME_NS: i128 = 33_333_334;
const BRACKET_NS: i128 = 1_000_000;
const GAP_NS: i128 = 250_000_000;
const COMPLETION: Duration = Duration::from_secs(1);
type Checked<T> = Result<T, String>;

fn rational(text: &str) -> Checked<(i128, i128)> {
    let (a,b)=text.split_once('/').ok_or("Missing rational time base")?;
    let a=a.parse::<i64>().map_err(|_|"Invalid time base numerator")?;
    let b=b.parse::<i64>().map_err(|_|"Invalid time base denominator")?;
    if a<=0 || b<=0 { return Err("Non-positive time base".into()); }
    Ok((a.into(),b.into()))
}
fn timestamp(text: &str, tb: (i128,i128)) -> Checked<i128> {
    let value=text.parse::<i64>().map_err(|_|"Invalid PTS")?;
    if value<0 || value==i64::MAX { return Err("Missing or negative PTS".into()); }
    i128::from(value).checked_mul(tb.0).and_then(|v|v.checked_mul(1_000_000_000)).map(|v|v/tb.1).ok_or("PTS conversion overflow".into())
}
#[derive(Clone, Debug)]
struct Mapping { frame: u64, input: u64, source: i128, output: i128, source_tb: String, source_pts: String, output_tb: String, output_pts: String }
impl Mapping {
    fn parse(line: &[u8]) -> Checked<Self> {
        let text=std::str::from_utf8(line).map_err(|_|"Non-UTF8 statistics")?;
        let fields:Vec<_>=text.trim_end_matches('\r').split(',').collect();
        if fields.len()!=8 || fields[0]!="0" || fields[1]!="0" { return Err("Unexpected statistics format or stream".into()); }
        Ok(Self {frame:fields[2].parse().map_err(|_|"Invalid output index")?,input:fields[3].parse().map_err(|_|"Invalid input index")?,
            source:timestamp(fields[5],rational(fields[4])?)?,output:timestamp(fields[7],rational(fields[6])?)?,
            source_tb:fields[4].into(),source_pts:fields[5].into(),output_tb:fields[6].into(),output_pts:fields[7].into()})
    }
    fn json(&self) -> Value { json!({"outputIndex":self.frame.to_string(),"inputIndex":self.input.to_string(),"sourceNs":self.source.to_string(),"outputNs":self.output.to_string(),"sourceTimeBase":self.source_tb,"sourcePts":self.source_pts,"outputTimeBase":self.output_tb,"outputPts":self.output_pts}) }
}
#[derive(Default)]
struct Stats {
    line: Vec<u8>, oversized: bool, reason: Option<String>, first: Option<Mapping>, last: Option<Mapping>,
    count: u64, duplicates: u64, skipped: u64, residual_low:i128, residual_high:i128, largest_source_gap:i128,
}
impl Stats {
    fn fail(&mut self, reason: impl Into<String>) { if self.reason.is_none() { self.reason=Some(reason.into()); } }
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte==b'\n' {
                if !self.oversized { match Mapping::parse(&self.line) {Ok(row)=>self.push(row),Err(e)=>self.fail(e)} }
                self.line.clear();self.oversized=false;
            } else if self.line.len()<LINE_LIMIT && !self.oversized {self.line.push(byte);}
            else {self.oversized=true;self.fail("Statistics line exceeds 512 bytes");}
        }
    }
    fn push(&mut self, row: Mapping) {
        if let Some(last)=&self.last {
            if row.frame!=last.frame.saturating_add(1) || row.input<last.input || row.source<last.source || row.output<=last.output || (row.input==last.input && row.source!=last.source) || row.source_tb!=last.source_tb || row.output_tb!=last.output_tb {
                self.fail("Statistics progression is inconsistent");return;
            }
            self.largest_source_gap=self.largest_source_gap.max(row.source-last.source);
            if row.input==last.input { self.duplicates=self.duplicates.saturating_add(1); }
            else {self.skipped=self.skipped.saturating_add(row.input-last.input-1);}
        } else if row.frame!=0 || row.output!=0 {self.fail("Missing first encoded frame at PTS zero");return;}
        let first=self.first.get_or_insert_with(||row.clone());
        let residual=(row.output-first.output)-(row.source-first.source);
        self.residual_low=self.residual_low.min(residual); self.residual_high=self.residual_high.max(residual);
        if residual.abs()>FRAME_NS {self.fail("Source/output residual exceeds one 30 fps frame");}
        self.last=Some(row);self.count=self.count.saturating_add(1);
    }
    fn eof(&mut self) {if !self.line.is_empty() || self.oversized {self.fail("Truncated statistics row");}if self.count==0 {self.fail("No frame statistics received");}}
    fn endpoint_frames(&self,saved:&ProbedVideoEvidence)->Checked<u64> {
        if let Some(reason)=&self.reason {return Err(reason.clone());}
        let last=self.last.as_ref().ok_or("Missing encoded endpoint")?;
        // Contiguous indices and strictly increasing integer PTS from zero prove every 1/30 tick.
        if last.output_tb!="1/30" || last.output_pts.parse::<u64>().ok()!=Some(last.frame)
            || last.frame.checked_add(1)!=Some(self.count) {
            return Err("Encoded endpoint requires a complete fixed 30 fps grid".into());
        }
        if !(1..=2_592_000).contains(&self.count) {return Err("Completed frame count exceeds the 24 hour recording limit".into());}
        let cadence=saved.first_packet_duration;
        if cadence.numerator==0 || cadence.denominator==0 || packet_product(cadence,30)?!=u128::from(cadence.denominator) {
            return Err("Saved packet duration does not verify the 30 fps cadence".into());
        }
        let endpoint=saved.endpoint;
        if endpoint.numerator==0 || endpoint.denominator==0 || packet_product(endpoint,30)?!=u128::from(self.count).checked_mul(endpoint.denominator.into()).ok_or("Saved endpoint overflow")? {
            return Err("Exact saved video endpoint does not match completed encoder grid".into());
        }
        Ok(self.count)
    }
    fn json(&self) -> Value {json!({"rows":self.count.to_string(),"first":self.first.as_ref().map(Mapping::json),"last":self.last.as_ref().map(Mapping::json),"duplicatedInputs":self.duplicates.to_string(),"skippedInputs":self.skipped.to_string(),"residualLowNs":self.residual_low.to_string(),"residualHighNs":self.residual_high.to_string(),"largestSourceGapNs":self.largest_source_gap.to_string()})}
}
fn packet_product(time:PacketTime,scale:u128)->Checked<u128> {
    u128::from(time.ticks).checked_mul(time.numerator.into()).and_then(|v|v.checked_mul(scale)).ok_or("Saved packet arithmetic overflow".into())
}
fn read_statistics(mut reader: impl Read) -> Stats {
    let mut stats=Stats::default();let mut buffer=[0;4096];
    loop {match reader.read(&mut buffer) {Ok(0)=>break,Ok(n)=>stats.feed(&buffer[..n]),Err(e) if e.kind()==std::io::ErrorKind::Interrupted=>{},Err(e)=>{stats.fail(format!("Statistics reader failed: {e}"));break;}}}
    stats.eof();stats
}
#[cfg(target_os="linux")]
struct StatisticsReader<R> { reader:R, stop:mpsc::Receiver<()> }
#[cfg(target_os="linux")]
impl<R:Read> Read for StatisticsReader<R> {
    fn read(&mut self, bytes:&mut [u8])->std::io::Result<usize> {
        let interrupted=||std::io::Error::other("Statistics reading interrupted before EOF");
        loop {
            if self.stop.try_recv()!=Err(mpsc::TryRecvError::Empty) {return Err(interrupted());}
            match self.reader.read(bytes) {
                Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=> {
                    if self.stop.recv_timeout(Duration::from_millis(20))!=Err(mpsc::RecvTimeoutError::Timeout) {return Err(interrupted());}
                }
                result=>return result,
            }
        }
    }
}
#[derive(Clone)]
struct Pair {before:i128,wall:i128,after:i128}
impl Pair {fn json(&self)->Value {json!({"beforeNs":self.before.to_string(),"unixNs":self.wall.to_string(),"afterNs":self.after.to_string()})}}
#[derive(Default)]
struct Bridge {
    low:Option<i128>,high:Option<i128>,count:u64,min_read:Option<i128>,max_read:i128,max_gap:i128,
    first:Option<Pair>,last:Option<Pair>,low_pair:Option<Pair>,high_pair:Option<Pair>,examples:Vec<Pair>,reason:Option<String>,
}
impl Bridge {
    fn fail(&mut self, reason:impl Into<String>) {if self.reason.is_none() {self.reason=Some(reason.into());}}
    fn sample(&mut self,before:i128,wall:i128,after:i128) {
        if before<0 || after<before || wall<0 {self.fail("Invalid clock sample");return;}
        let Some(low)=wall.checked_sub(after) else {self.fail("Clock arithmetic overflow");return};
        let Some(high)=wall.checked_sub(before) else {self.fail("Clock arithmetic overflow");return};
        let pair=Pair{before,wall,after};
        if let Some(last)=&self.last {
            if before<last.after {self.fail("Monotonic clock reversed");return;}
            self.max_gap=self.max_gap.max(before-last.before);
        }
        self.min_read=Some(self.min_read.map_or(after-before,|old|old.min(after-before)));
        self.max_read=self.max_read.max(after-before);
        if self.low.is_none_or(|old|low<old) {self.low=Some(low);self.low_pair=Some(pair.clone());}
        if self.high.is_none_or(|old|high>old) {self.high=Some(high);self.high_pair=Some(pair.clone());}
        self.first.get_or_insert_with(||pair.clone());self.last=Some(pair.clone());self.count=self.count.saturating_add(1);
        if self.examples.len()<60 {self.examples.push(pair);}
        if high.checked_sub(low).is_none() || self.high.zip(self.low).and_then(|(h,l)|h.checked_sub(l)).is_none_or(|width|width>BRACKET_NS) || self.max_read>BRACKET_NS {self.fail("Clock step or read/envelope width exceeds 1 ms policy");}
        if self.max_gap>GAP_NS {self.fail("Clock sampling gap exceeds 250 ms policy");}
    }
    fn observe(&mut self,epoch:Instant) {
        let Ok(before)=i128::try_from(epoch.elapsed().as_nanos()) else {self.fail("Monotonic clock overflow");return;};
        let wall=SystemTime::now().duration_since(UNIX_EPOCH).ok().and_then(|d|i128::try_from(d.as_nanos()).ok());
        let Ok(after)=i128::try_from(epoch.elapsed().as_nanos()) else {self.fail("Monotonic clock overflow");return;};
        if let Some(wall)=wall {self.sample(before,wall,after);} else {self.fail("System clock precedes Unix epoch");}
    }
    fn anchor(&self, source:i128) -> Checked<(i128,i128)> {
        if let Some(reason)=&self.reason {return Err(reason.clone());}
        if self.count<2 {return Err("Insufficient clock samples".into());}
        let low=source.checked_sub(self.high.ok_or("Missing clock envelope")?).ok_or("Anchor overflow")?;
        let high=source.checked_add(1000).and_then(|s|s.checked_sub(self.low?)).ok_or("Anchor overflow")?;
        if low<self.first.as_ref().ok_or("Missing first sample")?.before || high>self.last.as_ref().ok_or("Missing last sample")?.after {return Err("Source anchor outside sampled capture interval".into());}
        Ok((low,high))
    }
    fn json(&self)->Value {json!({"samples":self.count.to_string(),"offsetLowNs":self.low.map(|v|v.to_string()),"offsetHighNs":self.high.map(|v|v.to_string()),"minReadNs":self.min_read.map(|v|v.to_string()),"maxReadNs":self.max_read.to_string(),"maxGapNs":self.max_gap.to_string(),"first":self.first.as_ref().map(Pair::json),"last":self.last.as_ref().map(Pair::json),"lowPair":self.low_pair.as_ref().map(Pair::json),"highPair":self.high_pair.as_ref().map(Pair::json),"examples":self.examples.iter().map(Pair::json).collect::<Vec<_>>()})}
}

#[derive(Clone)]
pub(super) struct Capability { identity:Value, version:String, reason:Option<String> }
impl Capability {
    pub(super) fn probe(path:&Path) -> Self {
        let identity=fs::canonicalize(path).and_then(|path|fs::metadata(&path).map(|m|json!({"path":path.to_string_lossy(),"sizeBytes":m.len().to_string(),"modifiedUnixNs":m.modified().ok().and_then(|t|t.duration_since(UNIX_EPOCH).ok()).map(|d|d.as_nanos().to_string())})));
        let identity=match identity {Ok(v)=>v,Err(e)=>return Self{identity:json!({"path":path.to_string_lossy()}),version:String::new(),reason:Some(e.to_string())}};
        static CACHE:OnceLock<Mutex<Option<Capability>>>=OnceLock::new();
        let cache=CACHE.get_or_init(||Mutex::new(None));
        if let Ok(guard)=cache.lock() {if let Some(c)=guard.as_ref().filter(|c|c.identity==identity) {return c.clone();}}
        let mut capability=Self{identity,version:String::new(),reason:None};
        let result=(|| -> Checked<()> {
            if !cfg!(target_os="linux") {return Err("Backend not verified; video clock enabled only for X11".into());}
            let mut version=Command::new(path);version.arg("-version");
            let (bytes,_)=focusflow_core::ffmpeg::run_probe(version,true).map_err(|e|e.message)?;
            capability.version=String::from_utf8_lossy(&bytes).lines().next().unwrap_or_default().chars().take(512).collect();
            let mut smoke=Command::new(path);
            smoke.args(["-v","error","-f","lavfi","-i","color=size=16x16:rate=30","-frames:v","2","-copyts","-map","0:v:0","-vf","setpts=PTS-STARTPTS","-c:v","libx264","-threads","1","-stats_enc_pre:v:0","pipe:1","-stats_enc_pre_fmt:v:0",FORMAT,"-f","null","-"]);
            let (bytes,_)=focusflow_core::ffmpeg::run_probe(smoke,true).map_err(|e|e.message)?;
            let stats=read_statistics(bytes.as_slice());
            if let Some(reason)=stats.reason {return Err(reason);}
            if stats.count!=2 {return Err("Statistics smoke test did not return two frames".into());}
            Ok(())
        })();
        capability.reason=result.err().map(|s|s.chars().take(1024).collect());
        if let Ok(mut guard)=cache.lock() {*guard=Some(capability.clone());}
        capability
    }
    #[cfg(target_os = "linux")]
    pub(super) fn supported(&self)->bool {self.reason.is_none()}
    pub(super) fn configure(&self,args:&mut Vec<String>) {
        if self.reason.is_some() {return;}
        args.insert(0,"-copyts".into());
        let i=args.len().saturating_sub(1);
        args.splice(i..i,["-map","0:v:0","-vf","setpts=PTS-STARTPTS","-stats_enc_pre:v:0","pipe:1","-stats_enc_pre_fmt:v:0",FORMAT].map(str::to_owned));
    }
}

pub(super) struct Clock {
    capability:Capability,stats:Option<mpsc::Receiver<Stats>>,bridge:Option<mpsc::Receiver<Bridge>>,stop:Option<mpsc::Sender<()>>,
    bridge_owner:Option<thread::JoinHandle<()>>, stats_owner:Option<thread::JoinHandle<()>>, stats_stop:Option<mpsc::SyncSender<()>>,
}
impl Clock {
    pub(super) fn start(capability:Capability,epoch:Instant)->Self {
        let mut clock=Self{capability,stats:None,bridge:None,stop:None,bridge_owner:None,stats_owner:None,stats_stop:None};
        if clock.capability.reason.is_some() {return clock;}
        let (stop,control)=mpsc::channel();let (send,receive)=mpsc::sync_channel(1);
        match thread::Builder::new().name("video-clock-bridge".into()).spawn(move || {
            let mut bridge=Bridge::default();bridge.observe(epoch);
            loop {if control.recv_timeout(Duration::from_millis(20))!=Err(mpsc::RecvTimeoutError::Timeout) {bridge.observe(epoch);break;}bridge.observe(epoch);}
            let _=send.send(bridge);
        }) {Ok(owner)=>{clock.stop=Some(stop);clock.bridge=Some(receive);clock.bridge_owner=Some(owner);},Err(e)=>clock.capability.reason=Some(format!("Clock sampler failed: {e}"))}
        clock
    }
    #[cfg(target_os="linux")]
    pub(super) fn enabled(&self)->bool {self.capability.reason.is_none()}
    #[cfg(target_os="linux")]
    pub(super) fn read(&mut self,reader:impl Read+std::os::fd::AsFd+Send+'static) {
        use rustix::fs::{fcntl_getfl,fcntl_setfl,OFlags};
        // The worker exclusively owns this pipe; preserve its existing descriptor flags.
        if let Err(error)=fcntl_getfl(&reader).and_then(|flags|fcntl_setfl(&reader,flags|OFlags::NONBLOCK)) {
            self.capability.reason=Some(format!("Could not make statistics reading interruptible: {error}"));return;
        }
        let (stop,control)=mpsc::sync_channel(1);let (send,receive)=mpsc::sync_channel(1);
        match thread::Builder::new().name("video-clock-stats".into()).spawn(move || {let stats=read_statistics(StatisticsReader{reader,stop:control});let _=send.send(stats);}) {
            Ok(owner)=>{self.stats=Some(receive);self.stats_owner=Some(owner);self.stats_stop=Some(stop);},Err(e)=>self.capability.reason=Some(format!("Statistics reader failed to start: {e}")),
        }
    }
    pub(super) fn request_stop(&mut self) {
        if let Some(stop)=self.stop.take() {let _=stop.send(());}
    }
    pub(super) fn finish(mut self,encoder_ok:bool,saved:Option<ProbedVideoEvidence>)->Evidence {
        self.request_stop();
        let bridge=self.bridge.take().and_then(|r|r.recv_timeout(COMPLETION).ok());
        let stats=self.stats.take().and_then(|r|r.recv_timeout(COMPLETION).ok());
        let result=(|| -> Checked<AcceptedClock> {
            if let Some(reason)=&self.capability.reason {return Err(reason.clone());}
            if !encoder_ok {return Err("Encoder did not finalize successfully".into());}
            let stats=stats.as_ref().ok_or("Statistics completion timed out or reader unavailable")?;
            if let Some(reason)=&stats.reason {return Err(reason.clone());}
            let first=stats.first.as_ref().ok_or("Missing first mapping")?;
            if first.source_tb!="1/1000000" {return Err("X11 input time base is not Unix microseconds".into());}
            let bridge=bridge.as_ref().ok_or("Clock sampler completion timed out")?;
            let (low,high)=bridge.anchor(first.source)?;
            bridge.anchor(stats.last.as_ref().ok_or("Missing last mapping")?.source)?;
            let saved=saved.as_ref().ok_or("Saved video could not be validated")?;
            let completed_frames=stats.endpoint_frames(saved)?;
            let frame_zero_ns=high.checked_sub(low).filter(|width|*width>0).and_then(|width|low.checked_add(width/2)).ok_or("Invalid source anchor midpoint")?;
            Ok(AcceptedClock{anchor_low_ns:low,anchor_high_exclusive_ns:high,frame_zero_ns,completed_frames,saved_endpoint:saved.endpoint})
        })();
        let reason=result.as_ref().err().map(|s|s.chars().take(1024).collect::<String>());
        let accepted=result.ok();
        let correction=accepted.as_ref().map(|clock|-(clock.frame_zero_ns as f64)/1e9);
        let duration=accepted.as_ref().map(|clock|clock.completed_frames as f64/30.0);
        let frames=accepted.as_ref().map(|clock|clock.completed_frames);
        let saved_packet=accepted.as_ref().and(saved.as_ref()).map(|saved|json!({"timeBase":format!("{}/{}",saved.first_packet_duration.numerator,saved.first_packet_duration.denominator),"duration":saved.first_packet_duration.ticks.to_string(),"provenance":"shared_first_and_tail_probe"}));
        let anchor=accepted.as_ref().map(|clock|json!({"lowNs":clock.anchor_low_ns.to_string(),"highExclusiveNs":clock.anchor_high_exclusive_ns.to_string()}));
        let value=json!({"schema":1,"status":if correction.is_some(){"accepted"}else{"unavailable"},"reason":reason,
            "backend":if cfg!(target_os="linux"){"x11"}else{"unverified"},"executable":self.capability.identity,"executableVersion":self.capability.version,
            "extraction":{"format":FORMAT,"sourceClock":"unix_microseconds_pre_image_copy_request","savedFirstPacket":saved_packet},
            "statistics":stats.as_ref().map(Stats::json),"clockBridge":bridge.as_ref().map(Bridge::json),"anchor":anchor,
            "policy":{"samplePeriodMs":20,"maxReadAndEnvelopeNs":BRACKET_NS.to_string(),"maxSamplingGapNs":GAP_NS.to_string(),"maxResidualNs":FRAME_NS.to_string(),"timestampTruncationNs":"1000","maxExamples":64,"maxLineBytes":LINE_LIMIT,"maxEvidenceBytes":EVIDENCE_LIMIT},
            "actualVideoDurationSeconds":duration,
            "videoEndpoint":frames.map(|frames|json!({"provenance":"completed_encoder_grid_and_saved_packet_cadence","endPts":frames.to_string(),"timeBase":"1/30","interval":"half_open","savedEndpoint":accepted.as_ref().map(|clock|json!({"ticks":clock.saved_endpoint.ticks.to_string(),"numerator":clock.saved_endpoint.numerator,"denominator":clock.saved_endpoint.denominator}))})),
            "appliedCorrectionSeconds":correction,"interactionClock":"delivery_time","audioTiming":"unmeasured","cursor":"embedded",
            "limits":["Unsampled clock changes are not bounded","Image copy and compositor presentation latency are not measured","Input event delivery lag is not measured"]});
        Evidence{value,accepted,applied:false}
    }
}
impl Drop for Clock {
    fn drop(&mut self) {
        self.request_stop();
        // An unconfirmed guardian may still hold stdout open after its stop deadline.
        if let Some(stop)=self.stats_stop.take() {let _=stop.try_send(());}
        for owner in [&mut self.bridge_owner, &mut self.stats_owner] {
            if let Some(owner) = owner.take() { let _ = owner.join(); }
        }
    }
}
#[derive(Debug,Clone,Copy)]
pub(super) struct AcceptedClock {
    pub(super) anchor_low_ns:i128,
    pub(super) anchor_high_exclusive_ns:i128,
    pub(super) frame_zero_ns:i128,
    pub(super) completed_frames:u64,
    pub(super) saved_endpoint:PacketTime,
}
pub(super) struct Evidence {pub(super) value:Value,pub(super) accepted:Option<AcceptedClock>,applied:bool}
impl Evidence {
    pub(super) fn set_separate_intent(&mut self,separate:bool) {self.value["cursor"]=json!(if separate {"separate"} else {"embedded"});}
    pub(super) fn reject(&mut self,reason:&str) {
        self.accepted=None;self.value["status"]=json!("unavailable");self.value["reason"]=json!(reason);
        for field in ["anchor","appliedCorrectionSeconds","actualVideoDurationSeconds","videoEndpoint"] {self.value[field]=Value::Null;}
        self.value["extraction"]["savedFirstPacket"]=Value::Null;
    }
    pub(super) fn apply(&mut self,events:&mut InteractionEvents) {
        if self.applied {return;}
        self.applied=true;
        let dropped=match self.accepted.as_ref() {
            Some(clock)=>rebase(events,Some(-(clock.frame_zero_ns as f64)/1e9),clock.completed_frames as f64/30.0),
            None=>Dropped::default(),
        };
        self.value["droppedEvents"]=serde_json::to_value(dropped).unwrap_or(Value::Null);
    }
    pub(super) fn bytes(&self)->Checked<Vec<u8>> {let bytes=serde_json::to_vec_pretty(&self.value).map_err(|e|e.to_string())?;if bytes.len()>EVIDENCE_LIMIT {return Err("Video clock evidence exceeds 64 KiB".into());}Ok(bytes)}
}
#[derive(Default,Debug,PartialEq,Serialize)]
struct Dropped {clicks:usize,points:usize,drags:usize}
fn rebase(events:&mut InteractionEvents,correction:Option<f64>,duration:f64)->Dropped {
    let Some(offset)=correction else{return Dropped::default()};
    let mut dropped=Dropped::default();
    let within=|t:f64|t.is_finite() && t>=0.0 && t<duration;
    events.clicks.retain_mut(|c|{c.timestamp+=offset;let keep=within(c.timestamp);if !keep{dropped.clicks+=1;}keep});
    events.drags.retain_mut(|d|{
        d.points.retain_mut(|p|{p.timestamp+=offset;let keep=within(p.timestamp);if !keep{dropped.points+=1;}keep});
        if d.points.len()<2 || d.points.last().is_none_or(|last|last.timestamp<=d.points[0].timestamp) || d.points.iter().all(|p|p.x==d.points[0].x && p.y==d.points[0].y) {dropped.drags+=1;return false;}
        d.start=d.points[0].timestamp;d.end=d.points[d.points.len()-1].timestamp;true
    });dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(n: u64, ni: u64, source: i64, pts: i64) -> String { format!("0,0,{n},{ni},1/1000000,{source},1/30,{pts}\n") }
    fn saved(frames:u64)->ProbedVideoEvidence {
        ProbedVideoEvidence{video:focusflow_core::ffmpeg::ProbedVideo{width:320,height:240,duration_us:frames*1_000_000/30},endpoint:PacketTime{ticks:frames,numerator:1,denominator:30},first_packet_duration:PacketTime{ticks:512,numerator:1,denominator:15360}}
    }
    fn accepted(frames:u64,frame_zero_ns:i128)->AcceptedClock {
        AcceptedClock{anchor_low_ns:frame_zero_ns,anchor_high_exclusive_ns:frame_zero_ns+1,frame_zero_ns,completed_frames:frames,saved_endpoint:saved(frames).endpoint}
    }
    fn completed_clock(frames:u64)->Clock {
        let mut stats=Stats::default();
        for i in 0..frames {stats.feed(row(i,i,1_100_000+(i*1_000_000/30) as i64,i as i64).as_bytes());}
        stats.eof();
        let mut bridge=Bridge::default();
        for i in 0..=200 {let before=i*20_000_000;bridge.sample(before,1_000_000_000+before,before+100);}
        let (stats_send,stats_receive)=mpsc::sync_channel(1);stats_send.send(stats).unwrap();
        let (bridge_send,bridge_receive)=mpsc::sync_channel(1);bridge_send.send(bridge).unwrap();
        Clock{capability:Capability{identity:json!({"fixture":true}),version:"fixture".into(),reason:None},stats:Some(stats_receive),bridge:Some(bridge_receive),stop:None,bridge_owner:None,stats_owner:None,stats_stop:None}
    }
    #[cfg(target_os="linux")]
    #[test]
    fn dropping_clock_joins_its_statistics_reader() {
        use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
        struct FinishingReader(Arc<AtomicBool>,std::process::ChildStdout);
        impl std::os::fd::AsFd for FinishingReader {fn as_fd(&self)->std::os::fd::BorrowedFd<'_> {self.1.as_fd()}}
        impl Read for FinishingReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                thread::sleep(Duration::from_millis(100));
                Ok(0)
            }
        }
        impl Drop for FinishingReader {
            fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); }
        }
        let finished = Arc::new(AtomicBool::new(false));
        let mut clock = Clock::start(Capability { identity:json!({}), version:String::new(), reason:None }, Instant::now());
        let mut child=Command::new("/usr/bin/true").stdout(std::process::Stdio::piped()).spawn().unwrap();
        let pipe=child.stdout.take().unwrap();child.wait().unwrap();
        clock.read(FinishingReader(finished.clone(),pipe));
        let bridge = clock.bridge.take().unwrap();
        drop(clock);
        assert!(bridge.try_recv().is_ok(), "clock returned before its bridge finished");
        assert!(finished.load(Ordering::SeqCst), "clock returned before its owned reader finished");
    }

    #[cfg(target_os="linux")]
    #[test]
    fn clock_cleanup_interrupts_statistics_while_writer_keeps_pipe_open() {
        use std::process::Stdio;
        for (finish,writing) in [(false,false),(true,false),(false,true),(true,true)] {
            let script=if writing {"import os;os.write(1,b'R')\nwhile True: os.write(1,b'0,0,0,0,1/1000000,1100000,1/30,0\\n'*64)"}
                else {"import sys,time;sys.stdout.write('R');sys.stdout.flush();time.sleep(10)"};
            let mut child=Command::new("/usr/bin/python3").args(["-c",script])
                .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
            let mut reader=child.stdout.take().unwrap();let mut ready=[0];reader.read_exact(&mut ready).unwrap();assert_eq!(ready,[b'R']);
            let (release,wait)=mpsc::sync_channel::<()>(1);
            // Bound the RED run without relying on voluntary EOF as the expected cleanup.
            let cleanup=thread::spawn(move || {let timed_out=wait.recv_timeout(Duration::from_secs(4)).is_err();let _=child.kill();child.wait().unwrap();timed_out});
            let mut clock=Clock::start(Capability {identity:json!({}),version:String::new(),reason:None},Instant::now());clock.read(reader);
            let started=Instant::now();
            if finish {assert!(clock.finish(false,None).accepted.is_none());} else {drop(clock);}
            let elapsed=started.elapsed();let _=release.send(());let forced_eof=cleanup.join().unwrap();
            assert!(!forced_eof,"clock waited for forced writer EOF instead of interrupting its owned reader (finish={finish})");
            assert!(elapsed<Duration::from_secs(2),"clock cleanup blocked for {elapsed:?}");
        }
    }

    #[cfg(target_os="linux")]
    #[test]
    fn bridge_stop_preserves_late_statistics_tail_until_normal_eof() {
        use std::process::Stdio;
        let mut child=Command::new("/usr/bin/python3").args(["-c", "import sys,time;print('0,0,0,0,1/1000000,1100000,1/30,0',flush=True);time.sleep(.15);print('0,0,1,1,1/1000000,1133333,1/30,1',flush=True)"])
            .stdout(Stdio::piped()).spawn().unwrap();
        let mut clock=Clock::start(Capability {identity:json!({}),version:String::new(),reason:None},Instant::now());clock.read(child.stdout.take().unwrap());
        clock.request_stop();
        let evidence=clock.finish(true,Some(saved(2)));assert!(child.wait().unwrap().success());
        assert_eq!(evidence.value["statistics"]["rows"],"2");assert_eq!(evidence.value["statistics"]["last"]["outputIndex"],"1");
    }

    #[test]
    fn optional_clock_sidecar_failure_keeps_native_correction_but_embedded_fallback() {
        use super::super::{persist_clock_interactions,ClickButton,ClickEvent,InteractionEvents};
        let dir=std::env::temp_dir().join(format!("focusflow-clock-persist-{}",std::process::id()));
        fs::create_dir_all(dir.join("video-clock.json")).unwrap();
        for separate in [false,true] {
            let mut evidence=completed_clock(90).finish(true,Some(saved(90)));
            let mut events=InteractionEvents {clicks:vec![ClickEvent {timestamp:1.0,x:10,y:20,button:ClickButton::Left}],drags:vec![]};
            persist_clock_interactions(&dir.join("screen.mp4"),Some(&mut evidence),separate,&mut events);
            let expected=if separate {0.89999945} else {1.0};
            assert!((events.clicks[0].timestamp-expected).abs()<1e-6,"{}",events.clicks[0].timestamp);
            assert_eq!(evidence.value["cursor"],if separate {"separate"} else {"embedded"});
            let before=events.clicks[0].timestamp;evidence.apply(&mut events);assert_eq!(events.clicks[0].timestamp,before);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn typed_saved_endpoint_controls_acceptance_and_diagnostics_cannot_control_rebase() {
        use super::super::{ClickButton,ClickEvent,InteractionEvents};
        for frames in [91,92] {
            let mut saved=saved(frames);saved.endpoint=PacketTime{ticks:frames*512,numerator:2,denominator:30720};
            let mut evidence=completed_clock(frames).finish(true,Some(saved));
            let clock=evidence.accepted.unwrap();
            assert_eq!(clock.frame_zero_ns,100_000_550);
            assert_eq!((clock.anchor_low_ns,clock.anchor_high_exclusive_ns),(100_000_000,100_001_100));
            assert_eq!(clock.completed_frames,frames);assert_eq!(clock.saved_endpoint,saved.endpoint);
            evidence.value["status"]=json!("unavailable");evidence.value["appliedCorrectionSeconds"]=json!(100);evidence.value["actualVideoDurationSeconds"]=json!(0);
            let mut events=InteractionEvents{clicks:vec![ClickEvent{timestamp:0.5,x:7,y:9,button:ClickButton::Left}],drags:vec![]};
            evidence.apply(&mut events);assert!((events.clicks[0].timestamp-0.39999945).abs()<1e-12);
            evidence.reject("fixture rejection");evidence.apply(&mut events);
            assert!(evidence.accepted.is_none());assert!((events.clicks[0].timestamp-0.39999945).abs()<1e-12);
            for field in ["anchor","videoEndpoint","appliedCorrectionSeconds","actualVideoDurationSeconds"] {assert!(evidence.value[field].is_null(),"{field}");}
            assert!(evidence.value["extraction"]["savedFirstPacket"].is_null());
        }
        let unequal=ProbedVideoEvidence{endpoint:PacketTime{ticks:91000001,numerator:1,denominator:30000000},..saved(91)};
        assert!(completed_clock(91).finish(true,Some(unequal)).accepted.is_none());
    }
    #[test]
    fn clock_rejects_bad_encoder_stats_bridge_missing_media_and_cadence() {
        for failure in ["encoder","stats","bridge","saved","cadence","zero"] {
            let mut clock=completed_clock(if failure=="zero" {0}else{91});let mut saved=Some(saved(91));
            if failure=="stats" {let (tx,rx)=mpsc::sync_channel(1);let mut stats=Stats::default();stats.fail("fixture stats error");tx.send(stats).unwrap();clock.stats=Some(rx);}
            if failure=="bridge" {let (tx,rx)=mpsc::sync_channel(1);let mut bridge=Bridge::default();bridge.fail("fixture bridge error");tx.send(bridge).unwrap();clock.bridge=Some(rx);}
            if failure=="saved" {saved=None;}
            if failure=="cadence" {saved.as_mut().unwrap().first_packet_duration.ticks=511;}
            let evidence=clock.finish(failure!="encoder",saved);
            assert!(evidence.accepted.is_none(),"{failure}");assert_eq!(evidence.value["status"],"unavailable");
            assert!(evidence.value["anchor"].is_null());assert!(evidence.value["videoEndpoint"].is_null());
        }
        let mut stats=read_statistics(row(0,0,1_100_000,0).as_bytes());
        stats.count=2_592_001;let last=stats.last.as_mut().unwrap();last.frame=2_592_000;last.output_pts="2592000".into();
        assert!(stats.endpoint_frames(&saved(2_592_001)).is_err());
        stats.count=2_592_000;let last=stats.last.as_mut().unwrap();last.frame=2_591_999;last.output_pts="2591999".into();
        assert_eq!(stats.endpoint_frames(&saved(2_592_000)).unwrap(),2_592_000);
    }
    #[test]
    fn stop_request_completes_bridge_before_saved_media_is_available() {
        let capability=Capability{identity:json!({"fixture":true}),version:"fixture".into(),reason:None};
        let mut clock=Clock::start(capability,Instant::now());clock.request_stop();clock.request_stop();
        let bridge=clock.bridge.take().unwrap().recv_timeout(COMPLETION).expect("bridge kept sampling after stop request");
        assert!(bridge.count>=2);
        assert!(clock.finish(true,None).accepted.is_none());
    }
    #[test]
    fn split_rows_duplicate_and_dropped_input_are_not_output_indices() {
        let text = row(0,0,1_000_000,0)+&row(1,0,1_000_000,1)+&row(2,2,1_066_666,2);
        let mut parser=Stats::default(); for bytes in text.as_bytes().chunks(3) { parser.feed(bytes); }
        parser.eof(); assert!(parser.reason.is_none(), "{:?}",parser.reason); assert_eq!(parser.count,3);
        assert_eq!(parser.residual_high,33_333_333); assert_eq!(parser.last.as_ref().unwrap().input,2);
    }
    #[test]
    fn malformed_sentinel_negative_overflow_and_unterminated_rows_fail_closed() {
        for text in ["0,0,0,0,1/1000000,9223372036854775807,1/30,0\n", "0,0,0,-1,1/1000000,1,1/30,0\n", "0,0,0,0,-1/1,1,1/30,0\n", "0,0,0,0,999999999999999999999999999999/1,99,1/30,0\n", "0,0,0,0,1/1000000,-1,1/30,0\n", "0,0,0,0,1/1000000,1,1/30,0", "0,0,0,0,0/1,1,1/30,0\n", "0,0,0,0,N/A,1,1/30,0\n", "0,0,0,0,9223372036854775807/1,9223372036854775806,1/30,0\n"] {
            let mut stats=Stats::default(); stats.feed(text.as_bytes());stats.eof();assert!(stats.reason.is_some(),"{text}");
        }
        let mut stats=Stats::default();stats.feed(&vec![b'1';4096]);assert!(stats.line.len()<=512);stats.feed(b"\n");stats.feed(row(0,0,100,0).as_bytes());stats.eof();assert_eq!(stats.count,1);assert!(stats.reason.is_some());
    }
    #[test]
    fn excessive_residual_and_reverse_source_are_unavailable() {
        for next in [row(1,1,900_000,1),row(1,1,1_200_000,1),row(1,1,1_033_333,-1),row(0,1,1_033_333,1)] {
            let mut stats=Stats::default();stats.feed((row(0,0,1_000_000,0)+&next).as_bytes());stats.eof();assert!(stats.reason.is_some());
        }
    }
    #[test]
    fn clock_bridge_rejects_steps_read_width_and_sampling_gaps() {
        let mut bridge=Bridge::default();bridge.sample(0,1_000_000_000,100);bridge.sample(20_000_000,1_020_000_000,20_000_100);assert!(bridge.reason.is_none());
        assert_eq!(bridge.anchor(1_010_000_000).unwrap(),(10_000_000,10_001_100));
        for (before,wall,after) in [(20_000_000,1_022_000_000,20_000_100),(300_000_000,1_300_000_000,300_000_100),(20_000_000,1_020_000_000,22_000_000)] {
            let mut b=Bridge::default();b.sample(0,1_000_000_000,100);b.sample(before,wall,after);assert!(b.reason.is_some());
        }
        for i in 2..1000 {let m=i*20_000_000;bridge.sample(m,1_000_000_000+m,m+100);}
        assert!(bridge.examples.len()<=64);
    }
    #[test]
    fn rebasing_discards_negative_and_end_boundary_and_recomputes_drags() {
        use super::super::{ClickButton,ClickEvent,DragInteraction,DragPoint,InteractionEvents};
        let points=vec![0.1,0.5,1.0,1.5].into_iter().map(|timestamp|DragPoint{timestamp,x:(timestamp*10.0) as i32,y:8}).collect();
        let mut events=InteractionEvents {clicks:vec![0.1,0.5,1.0,1.5].into_iter().map(|timestamp|ClickEvent{timestamp,x:4,y:8,button:ClickButton::Left}).collect(),drags:vec![DragInteraction{start:0.1,end:1.5,button:ClickButton::Right,points}]};
        let before=serde_json::to_value(&events.clicks).unwrap();assert_eq!(rebase(&mut events,None,1.0),Dropped::default());assert_eq!(serde_json::to_value(&events.clicks).unwrap(),before);
        let dropped=rebase(&mut events,Some(-0.5),1.0);assert_eq!(dropped.clicks,2);assert_eq!(dropped.points,2);assert_eq!(events.clicks.iter().map(|c|c.timestamp).collect::<Vec<_>>(),vec![0.0,0.5]);assert_eq!((events.drags[0].start,events.drags[0].end),(0.0,0.5));
        assert_eq!(rebase(&mut events,Some(-0.25),1.0).drags,1);
    }
    #[test]
    fn non_centisecond_endpoint_clips_clicks_and_partial_drags_precisely() {
        use super::super::{ClickButton,ClickEvent,DragInteraction,DragPoint,InteractionEvents};
        for (frames,rounded) in [(92_u64,3.07),(91,3.03)] {
            let actual=frames as f64/30.0;
            let times=[actual-0.01,actual-0.000001,actual,actual+0.000001];
            let mut events=InteractionEvents {
                clicks:times.iter().map(|&timestamp|ClickEvent{timestamp,x:4,y:8,button:ClickButton::Left}).collect(),
                drags:vec![DragInteraction{start:times[0],end:times[3],button:ClickButton::Right,
                    points:times.iter().enumerate().map(|(i,&timestamp)|DragPoint{timestamp,x:i as i32*10,y:8}).collect()}],
            };
            let mut evidence=Evidence{value:json!({"actualVideoDurationSeconds":actual}),accepted:Some(accepted(frames,0)),applied:false};
            evidence.apply(&mut events);
            assert_eq!(events.clicks.iter().map(|c|c.timestamp).collect::<Vec<_>>(),times[..2],"{frames} frames, coarse duration {rounded}");
            assert_eq!(events.drags[0].points.len(),2);
            assert_eq!((events.drags[0].start,events.drags[0].end),(times[0],times[1]));
            assert_eq!(evidence.value["actualVideoDurationSeconds"],actual);
            assert_eq!(evidence.value["droppedEvents"]["clicks"],2);
            assert_eq!(evidence.value["droppedEvents"]["points"],2);
        }
    }
    #[test]
    fn accepted_correction_is_applied_once_and_preserves_window_coordinates() {
        use super::super::{ClickButton,ClickEvent,InteractionEvents};
        let mut events=InteractionEvents{clicks:vec![ClickEvent{timestamp:0.75,x:12,y:18,button:ClickButton::Left}],drags:vec![]};
        let mut evidence=Evidence{value:json!({}),accepted:Some(accepted(30,500_000_000)),applied:false};
        evidence.apply(&mut events);evidence.apply(&mut events);
        assert_eq!(events.clicks.len(),1,"second apply discarded an already corrected click");
        assert_eq!((events.clicks[0].timestamp,events.clicks[0].x,events.clicks[0].y),(0.25,12,18));
    }
    #[test]
    fn endpoint_requires_complete_fixed_grid_and_matching_saved_packet_duration() {
        let packet=saved(92);
        let mut stats=Stats::default();
        for i in 0..92 {stats.feed(row(i,i,1_000_000+(i*1_000_000/30) as i64,i as i64).as_bytes());}
        stats.eof();assert_eq!(stats.endpoint_frames(&packet).unwrap(),92);
        assert!(stats.endpoint_frames(&ProbedVideoEvidence{first_packet_duration:PacketTime{ticks:511,numerator:1,denominator:15360},..packet}).is_err());
        assert!(stats.endpoint_frames(&ProbedVideoEvidence{first_packet_duration:PacketTime{ticks:u64::MAX,numerator:u32::MAX,denominator:30},..packet}).is_err());
        let text=row(0,0,1_000_000,0)+&row(1,1,1_033_334,2);
        let mut gap=read_statistics(text.as_bytes());assert!(gap.reason.is_none());
        assert!(gap.endpoint_frames(&packet).unwrap_err().contains("fixed 30 fps"));
        gap.last.as_mut().unwrap().output_tb="1/60".into();
        assert!(gap.endpoint_frames(&packet).is_err());
        let truncated=read_statistics((row(0,0,1_000_000,0)+"0,0,").as_bytes());
        assert!(truncated.endpoint_frames(&packet).is_err());
    }
    #[test]
    fn matching_cadence_cannot_accept_unequal_exact_saved_endpoint() {
        let mut stats=Stats::default();
        for i in 0..91 {stats.feed(row(i,i,1_000_000+(i*1_000_000/30) as i64,i as i64).as_bytes());}
        stats.eof();
        let mut exact=saved(91);exact.endpoint=PacketTime{ticks:46592,numerator:2,denominator:30720};exact.first_packet_duration=PacketTime{ticks:512,numerator:2,denominator:30720};
        assert_eq!(stats.endpoint_frames(&exact).unwrap(),91);
        // 91/30 and 91000001/30000000 both floor to 3033333 microseconds.
        let unequal=ProbedVideoEvidence{endpoint:PacketTime{ticks:91000001,numerator:1,denominator:30000000},..saved(91)};
        assert!(stats.endpoint_frames(&unequal).is_err(),"cadence alone accepted unequal exact endpoint");
    }
    #[test]
    fn reader_errors_completion_timeout_and_evidence_cap_are_explicit() {
        struct Broken;
        impl Read for Broken {fn read(&mut self,_:&mut[u8])->std::io::Result<usize>{Err(std::io::Error::other("fixture read failure"))}}
        assert!(read_statistics(Broken).reason.unwrap().contains("reader failed"));
        let capability=Capability{identity:json!({"fixture":true}),version:"fixture".into(),reason:None};
        let (send,receive)=mpsc::channel();
        let mut clock=Clock::start(capability,Instant::now());clock.stats=Some(receive);
        let started=Instant::now();let evidence=clock.finish(true,None);
        assert!(started.elapsed()<Duration::from_secs(2));assert_eq!(evidence.value["status"],"unavailable");assert!(evidence.value["reason"].as_str().unwrap().contains("completion"));drop(send);
        assert!(evidence.bytes().unwrap().len()<EVIDENCE_LIMIT);
        let mut oversized=evidence;oversized.value["fixture"]=json!("x".repeat(EVIDENCE_LIMIT));assert!(oversized.bytes().is_err());
    }
    #[cfg(unix)]
    #[test]
    fn unsupported_capability_preserves_capture_arguments() {
        let capability=Capability::probe(Path::new("/usr/bin/false"));assert!(capability.reason.is_some());
        let mut args=vec!["-i".into(),"display".into(),"output.mp4".into()];let original=args.clone();capability.configure(&mut args);assert_eq!(args,original);
    }

    #[cfg(target_os="linux")]
    #[test]
    #[ignore="requires test-owned DISPLAY=:98, generated content and the actual built guardian"]
    fn native_clock_delayed_delivery_and_shared_finalizer() {
        use super::super::*;
        use std::sync::Arc;
        use x11rb::{connection::Connection, protocol::xproto::*};
        assert_eq!(std::env::var("DISPLAY").as_deref(),Ok(":98"));
        assert_eq!(std::env::var("FOCUSFLOW_ISOLATED_X11").as_deref(),Ok("1"));
        let ffmpeg=std::path::PathBuf::from(std::env::var("FOCUSFLOW_FFMPEG").unwrap());
        let guardian=std::path::PathBuf::from(std::env::var("FOCUSFLOW_NATIVE_GUARDIAN").unwrap());
        let root=std::path::PathBuf::from(std::env::var("FOCUSFLOW_NATIVE_EVIDENCE").unwrap());
        let (c,screen)=x11::connect().unwrap();let root_window=c.setup().roots[screen].root;
        let window=c.generate_id().unwrap();
        c.create_window(x11rb::COPY_DEPTH_FROM_PARENT,window,root_window,0,0,320,240,0,WindowClass::INPUT_OUTPUT,0,&CreateWindowAux::new().background_pixel(0x22aa66)).unwrap().check().unwrap();
        c.map_window(window).unwrap().check().unwrap();c.flush().unwrap();
        thread::sleep(Duration::from_millis(100));
        for (name,delay,unsupported,frame_cap,diagnostic_failure) in [
            ("delivery-0",0,false,None,false),("delivery-800",800,false,None,false),("unavailable",0,true,None,false),
            ("frames-91",0,false,Some(91_u64),false),("frames-92",0,false,Some(92),false),("diagnostic-failure",0,false,None,true),
        ] {
            let directory=root.join(name);fs::create_dir_all(&directory).unwrap();
            let ffmpeg=if unsupported {
                use std::os::unix::fs::PermissionsExt;
                let wrapper=directory.join("unsupported-stats-ffmpeg");
                fs::write(&wrapper,format!("#!/usr/bin/python3\nimport os,sys\nif '-stats_enc_pre:v:0' in sys.argv:\n print('Unsupported statistics fixture',file=sys.stderr)\n sys.exit(1)\nos.execv({0:?},[{0:?}]+sys.argv[1:])\n",ffmpeg.to_str().unwrap())).unwrap();
                fs::set_permissions(&wrapper,fs::Permissions::from_mode(0o700)).unwrap();wrapper
            } else {ffmpeg.clone()};
            let video=directory.join("screen.mp4");
            let bounds=CaptureBounds{x:0,y:0,width:320,height:240};
            let capability=Capability::probe(&ffmpeg);assert_eq!(capability.reason.is_some(),unsupported,"{:?}",capability.reason);
            let mut args=x11::args(bounds,None,&video).unwrap();
            if let Some(frames)=frame_cap {let i=args.len()-1;args.splice(i..i,["-frames:v".into(),frames.to_string()]);}
            capability.configure(&mut args);
            assert_eq!(args.iter().any(|a|a=="-copyts"),!unsupported);assert!(!args.iter().any(|a|a=="-start_at_zero"));
            let epoch=Instant::now();let mut clock=Clock::start(capability,epoch);
            let mut command=Command::new(&ffmpeg);command.args(&args).current_dir(&directory);
            let mut launch=crate::capture_guard::Launch::spawn(&command,&guardian).unwrap();
            let log=Arc::new(Mutex::new(ProcessLog::default()));
            spawn_output_reader(launch.child_mut().unwrap().stderr.take().unwrap(),log.clone(),ProcessStream::Stderr);
            launch.wait_ready().unwrap();let reader=launch.stdout.take().unwrap();
            struct Delayed {reader:std::process::ChildStdout,delay:u64,epoch:Instant,delivery:Arc<Mutex<Option<f64>>>}
            impl std::os::fd::AsFd for Delayed {fn as_fd(&self)->std::os::fd::BorrowedFd<'_> {self.reader.as_fd()}}
            impl Read for Delayed {fn read(&mut self,bytes:&mut[u8])->std::io::Result<usize>{let n=self.reader.read(bytes)?;if n>0 && self.delivery.lock().unwrap().is_none(){thread::sleep(Duration::from_millis(self.delay));*self.delivery.lock().unwrap()=Some(self.epoch.elapsed().as_secs_f64());}Ok(n)}}
            let delivery=Arc::new(Mutex::new(None));
            if clock.enabled(){clock.read(Delayed{reader,delay,epoch,delivery:delivery.clone()});}
            else {spawn_output_reader(reader,log.clone(),ProcessStream::Stdout);}
            let pid=launch.encoder_pid;let mut owner=launch.publish().unwrap();
            let tracker=ClickTracker::start(epoch,ClickFilter::All);
            tracker.clicks.lock().unwrap().push(ClickEvent{timestamp:0.0,x:10,y:10,button:ClickButton::Left});
            thread::sleep(Duration::from_millis(650));
            assert!(Command::new("xdotool").args(["mousemove","80","80","mousedown","1"]).status().unwrap().success());
            thread::sleep(Duration::from_millis(150));
            assert!(Command::new("xdotool").args(["mousemove","140","130"]).status().unwrap().success());
            c.change_window_attributes(window,&ChangeWindowAttributesAux::new().background_pixel(0xaa2233)).unwrap().check().unwrap();
            c.clear_area(false,window,0,0,0,0).unwrap().check().unwrap();c.flush().unwrap();
            thread::sleep(Duration::from_millis(150));
            assert!(Command::new("xdotool").args(["mouseup","1"]).status().unwrap().success());
            thread::sleep(Duration::from_millis(900));
            if frame_cap.is_some() {
                let deadline=Instant::now()+Duration::from_secs(8);
                loop {
                    if let Some(status)=owner.try_wait().unwrap() {assert!(status.success(),"{name}: encoder did not reach successful natural EOF");break;}
                    if Instant::now()>=deadline {crate::capture_guard::close_and_reap(owner);panic!("{name}: encoder did not reach the frame cap");}
                    thread::sleep(Duration::from_millis(20));
                }
            }
            let raw_clicks=tracker.clicks.lock().unwrap().clone();let raw_drags=tracker.drags.lock().unwrap().clone();
            fs::write(directory.join("delivery-clicks.json"),serde_json::to_vec_pretty(&raw_clicks).unwrap()).unwrap();
            fs::write(directory.join("delivery-drags.json"),serde_json::to_vec_pretty(&raw_drags).unwrap()).unwrap();
            if diagnostic_failure {fs::create_dir(directory.join("video-clock.json")).unwrap();}
            let state=RecorderState::new();state.lock_runtime().unwrap().active=Some(ActiveRecording{
                native_capture:None,notice:None,
                capture:ActiveCapture::Ffmpeg{child:owner,encoder_pid:pid},video_clock:clock,audio_recorder:None,mic_device_name:None,click_tracker:tracker,output_path:video.clone(),started_at:epoch,
                monitor:MonitorInfo{name:Some("Generated clock fixture".into()),x:0,y:0,width:320,height:240,scale_factor:1.0},capture_bounds:bounds,source:RecordingSource::Screen,ffmpeg_path:ffmpeg.clone(),log,
            });
            let completed=tauri::async_runtime::block_on(stop_active_recording(&state)).unwrap();
            assert!(matches!(completed.phase,RecordingPhase::Completed));
            assert!(!Path::new(&format!("/proc/{pid}")).exists());
            let saved=focusflow_core::ffmpeg::probe_video_evidence(&ffmpeg,&video).unwrap();
            assert!(saved.video.duration_us>0);
            if diagnostic_failure {
                assert!(directory.join("video-clock.json").is_dir());
                let attempted:Value=serde_json::from_slice(&fs::read(directory.join("video-clock.json.tmp")).unwrap()).unwrap();
                assert_eq!(attempted["status"],"accepted","{attempted:#}");assert_eq!(attempted["droppedEvents"]["clicks"],1);
                assert_eq!(raw_clicks.first().unwrap().timestamp,0.0);
                assert_eq!(serde_json::from_slice::<Value>(&fs::read(directory.join("clicks.json")).unwrap()).unwrap(),serde_json::to_value(&raw_clicks).unwrap());
                assert_eq!(serde_json::from_slice::<Value>(&fs::read(directory.join("drags.json")).unwrap()).unwrap(),serde_json::to_value(&raw_drags).unwrap());
                assert!(!raw_drags.is_empty());
                continue;
            }
            let evidence:Value=serde_json::from_slice(&fs::read(directory.join("video-clock.json")).unwrap()).unwrap();
            if unsupported {
                assert_eq!(evidence["status"],"unavailable");assert!(evidence["actualVideoDurationSeconds"].is_null());assert!(evidence["videoEndpoint"].is_null());assert!(evidence["reason"].as_str().unwrap().contains("Unsupported statistics fixture"));
                assert_eq!(serde_json::from_slice::<Value>(&fs::read(directory.join("clicks.json")).unwrap()).unwrap(),serde_json::to_value(raw_clicks).unwrap());
                assert_eq!(serde_json::from_slice::<Value>(&fs::read(directory.join("drags.json")).unwrap()).unwrap(),serde_json::to_value(raw_drags).unwrap());
                continue;
            }
            assert_eq!(evidence["status"],"accepted","{evidence:#}");assert_eq!(evidence["droppedEvents"]["clicks"],1);
            let correction=evidence["appliedCorrectionSeconds"].as_f64().unwrap();assert!(correction<0.0);
            let frames=evidence["statistics"]["rows"].as_str().unwrap().parse::<u64>().unwrap();
            if let Some(expected)=frame_cap {
                assert_eq!(frames,expected,"{name}: completed encoder frame count");
                assert_eq!(u128::from(saved.endpoint.ticks)*u128::from(saved.endpoint.numerator)*30,u128::from(expected)*u128::from(saved.endpoint.denominator),"{name}: actual saved endpoint");
                assert_eq!(u128::from(saved.first_packet_duration.ticks)*u128::from(saved.first_packet_duration.numerator)*30,u128::from(saved.first_packet_duration.denominator));
                assert_eq!(saved.video.duration_us,if expected==91 {3_033_333}else{3_066_666});
            }
            assert_eq!(evidence["actualVideoDurationSeconds"].as_f64().unwrap(),frames as f64/30.0);
            assert_eq!(evidence["videoEndpoint"]["endPts"],frames.to_string());
            assert_eq!(evidence["videoEndpoint"]["timeBase"],"1/30");
            assert_eq!(evidence["videoEndpoint"]["provenance"],"completed_encoder_grid_and_saved_packet_cadence");
            let receipt=delivery.lock().unwrap().unwrap();if delay>0 {assert!(receipt+correction>0.7,"receipt={receipt},correction={correction}");}
            fs::write(directory.join("delivery-proof.json"),serde_json::to_vec_pretty(&json!({"injectedDelayMs":delay,"firstStatisticsDeliverySeconds":receipt,"sourceAnchorSeconds":-correction,"deliveryMinusSourceSeconds":receipt+correction})).unwrap()).unwrap();
            let clicks:Vec<ClickEvent>=serde_json::from_slice(&fs::read(directory.join("clicks.json")).unwrap()).unwrap();assert_eq!(clicks.len(),1);assert!((clicks[0].timestamp-(raw_clicks[1].timestamp+correction)).abs()<1e-9);
            let drags:Vec<DragInteraction>=serde_json::from_slice(&fs::read(directory.join("drags.json")).unwrap()).unwrap();assert_eq!(drags.len(),1);assert!((drags[0].start-(raw_drags[0].start+correction)).abs()<1e-9);
            let probed=saved.video;
            let document=focusflow_core::legacy::read_legacy(&directory,"clock-fixture",probed,focusflow_core::legacy::LegacyDefaults{scale:2.0,intro_us:200_000,hold_us:300_000,outro_us:200_000,pan_transition_us:200_000,settle_us:100_000}).unwrap();
            assert_eq!(document.timing.provenance,focusflow_core::project::TimingProvenance::LegacyUnknown);assert_eq!(document.timing.audio_offset_us,None);
            assert!(document.warnings.contains(&focusflow_core::project::ProjectWarning::EmbeddedCursor));
            assert_eq!(document.clicks[0].at_us,focusflow_core::legacy::seconds_to_us(clicks[0].timestamp).unwrap());
            let snapshot=focusflow_core::store::RenderSnapshot{project_dir:directory.clone(),document,prepared_focus:None,prepared_cursor:None};
            let plan=focusflow_core::render::compile(&snapshot.document,focusflow_core::render::ExportPreset::Balanced).unwrap();
            assert_eq!(plan.clicks[0].at_us,snapshot.document.clicks[0].at_us);
            assert_eq!((plan.clicks[0].x,plan.clicks[0].y),(clicks[0].x as f64/320.0,clicks[0].y as f64/240.0));
            fs::write(directory.join(focusflow_core::ffmpeg::FILTER_SCRIPT),focusflow_core::render::filter_graph(&snapshot,&plan,false).unwrap()).unwrap();
            let mut render=Command::new(&ffmpeg);render.args(focusflow_core::ffmpeg::render_args(&snapshot,&plan,&directory,false).unwrap());
            focusflow_core::ffmpeg::run_probe(render,false).unwrap();
            assert!(focusflow_core::ffmpeg::probe_video(&ffmpeg,&directory.join(focusflow_core::ffmpeg::RENDER_OUTPUT)).unwrap().duration_us>0);
            let selection=focusflow_core::preview::select(&plan,&focusflow_core::preview::PreviewRequest::Still{at_us:plan.clicks[0].at_us}).unwrap();
            assert_eq!(selection.first_frame,plan.clicks[0].at_us*30/1_000_000);
            fs::write(directory.join(focusflow_core::ffmpeg::FILTER_SCRIPT),focusflow_core::render::preview_filter_graph(&snapshot,&plan,&selection,false).unwrap()).unwrap();
            let mut preview=Command::new(&ffmpeg);preview.args(focusflow_core::ffmpeg::preview_args(&snapshot,&plan,&selection,&directory,false,true).unwrap());
            focusflow_core::ffmpeg::run_probe(preview,false).unwrap();
            assert!(fs::read(directory.join("output.partial.png")).unwrap().starts_with(b"\x89PNG"));
            println!("delay={delay}ms sourceAnchor={}s firstDelivery={receipt}s correctedClick={}s importedClick={}us",-correction,clicks[0].timestamp,snapshot.document.clicks[0].at_us);
        }
        c.destroy_window(window).unwrap().check().unwrap();c.flush().unwrap();
    }

}
