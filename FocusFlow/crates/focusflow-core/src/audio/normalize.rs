use std::{ffi::OsString, fs::{self, File, OpenOptions}, io::{Read,Write,Seek,SeekFrom}, path::{Path,PathBuf}, process::Command, time::{Duration,Instant}};
use crate::{CoreResult,CoreError,ErrorCode,project::{invalid,require},ffmpeg};
use super::{AudioRenderPlan,AudioTrackKind,AudioSourceSpan,OUTPUT_RATE,MAX_AUDIO_SPANS};

pub const RAW_AUDIO: &str = "audio-scratch/final.f32";
const Q:i128=4_000_000;
const WINDOW:u64=20*48_000;
const SLOT:u64=WINDOW;
const BUFFER:usize=65_536;
const STAGE_LIMIT:u64=64*1024*1024;
const SCRATCH_LIMIT:u64=64*1024*1024*1024;
const GRAPH_LIMIT:usize=32*1024*1024;
const STAGE_TIMEOUT:Duration=Duration::from_secs(120);

fn arithmetic()->CoreError {invalid("audio.timing","Audio timing arithmetic exceeds the supported range")}
fn checked(n:Option<i128>)->CoreResult<i128>{n.ok_or_else(arithmetic)}
fn unsigned(n:i128)->CoreResult<u64>{u64::try_from(n).map_err(|_|arithmetic())}
fn bytes(n:u64)->CoreResult<u64>{n.checked_mul(8).ok_or_else(arithmetic)}
#[derive(Clone,Copy,Debug)]
struct Ratio {n:i128,d:i128}
impl Ratio {
    fn new(n:i128,d:i128)->CoreResult<Self>{
        if d<=0{return Err(arithmetic());}let(mut a,mut b)=(n.unsigned_abs(),d as u128);while b!=0{(a,b)=(b,a%b);}let g=i128::try_from(a.max(1)).map_err(|_|arithmetic())?;Ok(Self{n:n/g,d:d/g})
    }
    fn integer(n:i128)->Self{Self{n,d:1}}
    fn add(self,b:Self)->CoreResult<Self>{Self::new(checked(checked(self.n.checked_mul(b.d))?.checked_add(checked(b.n.checked_mul(self.d))?))?,checked(self.d.checked_mul(b.d))?)}
    fn sub(self,b:Self)->CoreResult<Self>{self.add(Self{n:checked(b.n.checked_neg())?,d:b.d})}
    fn mul(self,b:Self)->CoreResult<Self>{Self::new(checked(self.n.checked_mul(b.n))?,checked(self.d.checked_mul(b.d))?)}
    fn div(self,b:Self)->CoreResult<Self>{if b.n<=0{return Err(arithmetic());}self.mul(Self::new(b.d,b.n)?)}
    fn floor(self)->i128{self.n.div_euclid(self.d)}
    fn ceil(self)->i128{self.floor()+i128::from(self.n.rem_euclid(self.d)!=0)}
    fn round(self)->CoreResult<i128>{let a=self.floor();let twice=checked(self.n.rem_euclid(self.d).checked_mul(2))?;Ok(a+i128::from(twice>self.d || (twice==self.d && a%2!=0)))}
    fn abs(self)->CoreResult<Self>{Self::new(self.n.checked_abs().ok_or_else(arithmetic)?,self.d)}
    fn at_most(self,b:Self)->CoreResult<bool>{Ok(checked(self.n.checked_mul(b.d))?<=checked(b.n.checked_mul(self.d))?)}
}
#[derive(Debug,Clone)]
pub(crate) struct Window {
    kind:AudioTrackKind, domain:usize, number:u64,
    first:u64,end:u64,central_first:u64,central_end:u64,
    real_first:u64,real_end:u64,c0:i128,c1:i128,origin:i128,p:u32,rate:u32,channels:u16,
    path:PathBuf,
}
impl Window {
    fn output_bytes(&self)->CoreResult<u64>{bytes(self.end-self.first)}
    fn context_bytes(&self)->CoreResult<u64>{bytes(unsigned(self.c1-self.c0)?)}
}
fn descriptor(audio:&AudioRenderPlan,kind:AudioTrackKind,s:&AudioSourceSpan,absolute_first:u64,number:u64)->CoreResult<Window>{
    let track=audio.tracks.iter().find(|t|t.kind==kind).ok_or_else(||invalid("audio","Missing prepared track"))?;
    let r=Ratio::new(s.step_denominator,s.step_numerator)?;
    let position=Ratio::new(s.position_numerator,s.position_denominator)?;
    let anchor=Ratio::integer(i128::from(absolute_first)).sub(position.mul(r)?)?;
    let p=r.mul(Ratio::integer(Q))?.round()?;
    require((995_000..=24_120_000).contains(&p),"audio.timing","Mapped resampling rate exceeds the accepted range")?;
    let approximate=Ratio::new(p,Q)?;
    let factor=approximate.mul(Ratio::new(97,100)?)?;
    let mut length=if factor.at_most(Ratio::integer(1))?{Ratio::integer(32).div(factor)?.ceil()}else{32};
    if length>1 && length%2!=0{length+=1;}let guard=length+2;
    let central_first=s.domain_first_sample.checked_add(number.checked_mul(WINDOW).ok_or_else(arithmetic)?).ok_or_else(arithmetic)?;
    let central_end=central_first.saturating_add(WINDOW).min(s.domain_end_sample);
    let left=if central_first>s.domain_first_sample{128_u64.min(central_first-s.domain_first_sample).min(s.domain_end_sample-central_first)}else{0};
    let right=if central_end<s.domain_end_sample{128_u64.min(central_end-s.domain_first_sample).min(s.domain_end_sample-central_end)}else{0};
    let first=central_first-left;let end=central_end+right;
    let c0=checked(Ratio::integer(i128::from(first)).sub(anchor)?.div(r)?.floor().checked_sub(guard))?;
    let c1=checked(Ratio::integer(i128::from(end)).sub(anchor)?.div(r)?.ceil().checked_add(guard))?;
    let distance=unsigned(checked(c1.checked_sub(c0))?)?;
    require(distance<=7_680_000,"audio.window","Audio context exceeds 7680000 source frames; request a shorter or simpler range")?;
    let origin=anchor.add(r.mul(Ratio::integer(c0))?)?.round()?;
    let error=Ratio::integer(origin).sub(anchor.add(r.mul(Ratio::integer(c0))?)?)?.abs()?.add(approximate.sub(r)?.abs()?.mul(Ratio::integer(i128::from(distance)))?)?;
    require(error.at_most(Ratio::new(146,100)?)?,"audio.window","Audio timing approximation exceeds 1.46 samples; request a shorter range")?;
    let real_first=unsigned(c0.max(i128::from(s.source_start_frame)).min(i128::from(s.source_end_frame)))?;
    let real_end=unsigned(c1.min(i128::from(s.source_end_frame)).max(i128::from(real_first)))?;
    require(c0<=i128::from(real_first) && i128::from(real_end)<=c1 && i128::from(first)>=origin,"audio.window","Invalid bounded reconstruction context")?;
    Ok(Window{kind,domain:s.domain,number,first,end,central_first,central_end,real_first,real_end,c0,c1,origin,p:u32::try_from(p).map_err(|_|arithmetic())?,rate:track.track.sample_rate,channels:track.track.channels,path:track.path.clone()})
}
/// Windows always belong to the full retained continuity domain, even for a tiny preview crop.
pub(crate) fn compile_windows(audio:&mut AudioRenderPlan,preview:bool)->CoreResult<()> {
    let mut total=0_usize;let mut max_context=0;let mut max_output=0;
    for collect in [false,true] {
        let mut previous=None;
        for span in &audio.spans {
            let Some(source)=&span.source else{continue;};
            let first=audio.first_sample.checked_add(span.output_first_sample).ok_or_else(arithmetic)?;
            let end=first.checked_add(span.sample_count).ok_or_else(arithmetic)?;
            let count=(source.domain_end_sample-source.domain_first_sample).div_ceil(WINDOW);
            let low=((first-source.domain_first_sample)/WINDOW).saturating_sub(1);
            let high=((end-1-source.domain_first_sample)/WINDOW).saturating_add(1).min(count-1);
            for number in low..=high {
                let w=descriptor(audio,span.kind,source,first,number)?;
                if w.first>=end || w.end<=first{continue;}
                if previous.is_some_and(|(kind,domain,number)|kind==w.kind && domain==w.domain && number>=w.number){continue;}
                previous=Some((w.kind,w.domain,w.number));
                if collect{audio.windows.push(w);}else{
                    total=total.checked_add(1).filter(|n|n.checked_mul(3).and_then(|v|v.checked_add(audio.spans.len())).is_some_and(|v|v<=MAX_AUDIO_SPANS)).ok_or_else(||invalid("audio.windows","Audio plan exceeds 16384 reconstruction pieces; request a shorter or simpler range"))?;
                    max_context=max_context.max(w.context_bytes()?);max_output=max_output.max(w.output_bytes()?);
                }
            }
        }
        if !collect{
            require(!preview || audio.input_base.checked_add(total).is_some_and(|n|n<=crate::preview::MAX_INPUTS),"preview.inputs","Preview exceeds 32 seeked file inputs; request a shorter or simpler range")?;
            require(total.checked_mul(std::mem::size_of::<Window>()+512).is_some_and(|n|n<=GRAPH_LIMIT),"audio.windows","Audio descriptors exceed 32 MiB; request a shorter range")?;
            let decode=max_context.checked_mul(2).and_then(|n|n.checked_add(BUFFER as u64*4)).ok_or_else(arithmetic)?;
            let normalize=max_context.checked_add(max_output).and_then(|n|n.checked_add(BUFFER as u64*4)).ok_or_else(arithmetic)?;
            require(decode.max(normalize)<=STAGE_LIMIT,"audio.scratch","Audio stage exceeds the 64 MiB PCM payload limit; request a shorter range")?;
            let queued=max_output.checked_mul(4).and_then(|n|n.checked_add(SLOT*8*3+4096)).ok_or_else(arithmetic)?;
            audio.scratch_upper=bytes(audio.total_samples)?.checked_add(decode.max(normalize)).and_then(|n|n.checked_add(queued)).and_then(|n|n.checked_add(GRAPH_LIMIT as u64*2)).ok_or_else(arithmetic)?;
            require(audio.scratch_upper<=SCRATCH_LIMIT,"audio.scratch","Audio scratch exceeds 64 GiB; export a shorter range")?;
            audio.windows.try_reserve_exact(total).map_err(|_|invalid("audio.windows","Could not allocate bounded audio descriptors"))?;
        }
    }
    Ok(())
}

fn stage_error(message:impl Into<String>)->CoreError{CoreError{code:ErrorCode::EncoderFailed,message:message.into(),field:Some("audio".into())}}
fn io(error:std::io::Error)->CoreError{stage_error(format!("Audio scratch I/O failed: {error}; free disk space and retry"))}
fn check_cancel(cancel:&impl Fn()->CoreResult<bool>)->CoreResult<()>{if cancel()?{Err(ffmpeg::cancel_error())}else{Ok(())}}
fn length(path:&Path,expected:u64)->CoreResult<()>{let actual=fs::metadata(path).map_err(io)?.len();if actual!=expected{return Err(stage_error(format!("Audio stage produced {actual} bytes; expected exactly {expected}")));}Ok(())}
fn zeros(output:&mut File,mut count:u64,cancel:&impl Fn()->CoreResult<bool>)->CoreResult<()>{
    let block=[0_u8;BUFFER];while count>0{check_cancel(cancel)?;let n=count.min(BUFFER as u64) as usize;output.write_all(&block[..n]).map_err(io)?;count-=n as u64;}Ok(())
}
fn copy_range(path:&Path,first:u64,count:u64,output:&mut File,cancel:&impl Fn()->CoreResult<bool>)->CoreResult<()>{
    let mut input=File::open(path).map_err(io)?;input.seek(SeekFrom::Start(bytes(first)?)).map_err(io)?;let mut left=bytes(count)?;let mut block=[0_u8;BUFFER];
    while left>0{check_cancel(cancel)?;let n=left.min(BUFFER as u64) as usize;input.read_exact(&mut block[..n]).map_err(io)?;output.write_all(&block[..n]).map_err(io)?;left-=n as u64;}Ok(())
}
fn new_file(path:&Path)->CoreResult<File>{OpenOptions::new().write(true).create_new(true).open(path).map_err(io)}
fn remove(path:&Path)->CoreResult<()>{match fs::remove_file(path){Ok(())=>Ok(()),Err(e) if e.kind()==std::io::ErrorKind::NotFound=>Ok(()),Err(e)=>Err(io(e))}}
fn base()->Vec<OsString>{["-hide_banner","-loglevel","error","-nostdin","-y","-nostats","-progress","pipe:1","-threads","1","-filter_complex_threads","1"].map(Into::into).into()}
fn raw_input(args:&mut Vec<OsString>,path:&Path,rate:u32){args.extend(["-protocol_whitelist".into(),"file,pipe".into(),"-f".into(),"f32le".into(),"-ar".into(),rate.to_string().into(),"-ac".into(),"2".into(),"-i".into(),path.as_os_str().into()]);}
fn output(args:&mut Vec<OsString>,path:&Path){args.extend(["-map".into(),"[out]".into(),"-c:a".into(),"pcm_f32le".into(),"-f".into(),"f32le".into(),path.as_os_str().into()]);}
fn seconds(us:u64)->String{format!("{}.{:06}",us/1_000_000,us%1_000_000)}
fn run_stage(dir:&Path,mut args:Vec<OsString>,graph:&str,dest:&Path,expected:u64,inputs:u64,cancel:&impl Fn()->CoreResult<bool>,launch:&impl Fn(&Path,Vec<OsString>)->Command)->CoreResult<()>{
    check_cancel(cancel)?;
    require(graph.len()<=GRAPH_LIMIT,"audio.graph","Audio graph exceeds 32 MiB; request a simpler range")?;
    require(inputs.checked_add(expected).and_then(|n|n.checked_add(BUFFER as u64*4)).is_some_and(|n|n<=STAGE_LIMIT),"audio.scratch","Active audio stage exceeds 64 MiB of PCM payload")?;
    remove(dest)?;
    let script=dir.join("stage.filter");crate::store::atomic_write(dir,"stage.filter",graph.as_bytes())?;
    args.extend(["-filter_complex_script".into(),script.into_os_string()]);output(&mut args,dest);
    let start=Instant::now();
    ffmpeg::run_encoder(launch(dir,args),cancel,|_|{if fs::metadata(dest).is_ok_and(|m|m.len()>expected){return Err(stage_error("Audio stage exceeded its exact output byte budget"));}if start.elapsed()>STAGE_TIMEOUT{Err(stage_error("Audio stage timed out after 120 seconds; retry a shorter range"))}else{Ok(())}})?;
    check_cancel(cancel)?;length(dest,expected)
}

/// The private directory owns every intermediate, including the final streamed raw input.
pub struct PreparedAudio{dir:PathBuf}
impl PreparedAudio{pub fn path(&self)->PathBuf{self.dir.join("final.f32")}}
impl Drop for PreparedAudio{fn drop(&mut self){let _=fs::remove_dir_all(&self.dir);}}
#[derive(Default)]
struct TrackCache{windows:std::collections::VecDeque<(usize,PathBuf)>,seam:Option<(usize,PathBuf)>}
struct Session<'a,C,L>{audio:&'a AudioRenderPlan,dir:&'a Path,cancel:&'a C,launch:&'a L,caches:[TrackCache;2]}
fn track_number(kind:AudioTrackKind)->usize{match kind{AudioTrackKind::Microphone=>0,AudioTrackKind::System=>1}}
impl<C:Fn()->CoreResult<bool>,L:Fn(&Path,Vec<OsString>)->Command> Session<'_,C,L>{
    fn window(&mut self,index:usize)->CoreResult<PathBuf>{
        let w=&self.audio.windows[index];let track=track_number(w.kind);
        if let Some((_,path))=self.caches[track].windows.iter().find(|(i,_)|*i==index){return Ok(path.clone());}
        if self.caches[track].windows.len()==2{let (_,old)=self.caches[track].windows.pop_front().unwrap();remove(&old)?;}
        let slot=(0..2).find(|slot|!self.caches[track].windows.iter().any(|(_,path)|path==&self.dir.join(format!("window-{track}-{slot}.f32")))).ok_or_else(||invalid("audio.windows","Audio window queue exceeds two entries"))?;
        let dest=self.dir.join(format!("window-{track}-{slot}.f32"));
        let decoded=self.dir.join("decoded.f32");let context=self.dir.join("context.f32");remove(&decoded)?;remove(&context)?;
        if w.real_first<w.real_end{
            let canonical=w.path.canonicalize().map_err(|e|CoreError{code:ErrorCode::InvalidPath,message:format!("Prepared audio asset no longer resolves to its contained path: {e}"),field:Some("audioTracks".into())})?;
            if canonical!=w.path || !canonical.is_file(){return Err(CoreError{code:ErrorCode::InvalidPath,message:"Audio asset changed its contained path during rendering".into(),field:Some("audioTracks".into())});}
            let seek_us=(w.real_first.checked_mul(1_000_000).ok_or_else(arithmetic)?/u64::from(w.rate)).saturating_sub(2000);
            let end_us=(w.real_end.checked_mul(1_000_000).ok_or_else(arithmetic)?).div_ceil(u64::from(w.rate));let duration=end_us.checked_sub(seek_us).and_then(|n|n.checked_add(2000)).ok_or_else(arithmetic)?;
            let mut args=base();args.extend(["-copyts".into(),"-analyzeduration".into(),"0".into(),"-probesize".into(),"32".into(),"-ss".into(),seconds(seek_us).into(),"-noaccurate_seek".into(),"-t".into(),seconds(duration).into(),"-max_size".into(),"4096".into(),"-protocol_whitelist".into(),"file,pipe".into(),"-f".into(),"wav".into(),"-i".into(),w.path.as_os_str().into()]);
            let graph=format!("[0:a]asettb=1/{},atrim=start_pts={}:end_pts={},asetpts=N/SR/TB[out]",w.rate,w.real_first,w.real_end);
            let decoded_bytes=(w.real_end-w.real_first).checked_mul(u64::from(w.channels)*4).ok_or_else(arithmetic)?;
            run_stage(self.dir,args,&graph,&decoded,decoded_bytes,0,self.cancel,self.launch)?;
        }
        let context_bytes=w.context_bytes()?;
        let decoded_bytes=if w.real_first<w.real_end{fs::metadata(&decoded).map_err(io)?.len()}else{0};
        require(decoded_bytes.checked_add(context_bytes).and_then(|n|n.checked_add(BUFFER as u64*4)).is_some_and(|n|n<=STAGE_LIMIT),"audio.scratch","Canonical audio context exceeds the 64 MiB active PCM limit")?;
        let mut canonical=new_file(&context)?;
        zeros(&mut canonical,bytes(unsigned(i128::from(w.real_first)-w.c0)?)?,self.cancel)?;
        if w.real_first<w.real_end{
            let mut source=File::open(&decoded).map_err(io)?;
            if w.channels==1{
                let mut input=[0_u8;BUFFER/4];let mut stereo=[0_u8;BUFFER/2];let mut left=decoded_bytes;
                while left>0{check_cancel(self.cancel)?;let n=left.min(input.len() as u64) as usize;source.read_exact(&mut input[..n]).map_err(io)?;require(n%4==0,"audio.context","Decoded mono samples are incomplete")?;
                    for (sample,pair) in input[..n].chunks_exact(4).zip(stereo[..n*2].chunks_exact_mut(8)){pair[..4].copy_from_slice(sample);pair[4..].copy_from_slice(sample);}canonical.write_all(&stereo[..n*2]).map_err(io)?;left-=n as u64;
                }
            }else{copy_range(&decoded,0,w.real_end-w.real_first,&mut canonical,self.cancel)?;}
        }
        zeros(&mut canonical,bytes(unsigned(w.c1-i128::from(w.real_end))?)?,self.cancel)?;canonical.flush().map_err(io)?;drop(canonical);length(&context,context_bytes)?;remove(&decoded)?;
        let mut args=base();raw_input(&mut args,&context,w.rate);
        let graph=format!("[0:a]asetrate={Q},aresample={}:phase_shift=8:filter_size=32:cutoff=0.97:linear_interp=1:exact_rational=1:resampler=swr:filter_type=kaiser:kaiser_beta=9:ochl=stereo,asetrate=48000,atrim=start_sample={}:end_sample={},asetpts=N/SR/TB[out]",w.p,i128::from(w.first)-w.origin,i128::from(w.end)-w.origin);
        run_stage(self.dir,args,&graph,&dest,w.output_bytes()?,context_bytes,self.cancel,self.launch)?;remove(&context)?;
        self.caches[track].windows.push_back((index,dest.clone()));Ok(dest)
    }
    fn seam(&mut self,left:usize,right:usize)->CoreResult<PathBuf>{
        let track=track_number(self.audio.windows[left].kind);
        if let Some((id,path))=&self.caches[track].seam{if *id==left{return Ok(path.clone());}}
        if let Some((_,old))=self.caches[track].seam.take(){remove(&old)?;}
        let a=self.window(left)?;let b=self.window(right)?;let wa=&self.audio.windows[left];let wb=&self.audio.windows[right];
        require(wa.kind==wb.kind && wa.domain==wb.domain && wa.number+1==wb.number,"audio.seam","Audio overlap must stay within one continuous affine domain")?;
        let first=wb.first;let end=wa.end;let count=end.checked_sub(first).ok_or_else(arithmetic)?;require((1..=256).contains(&count),"audio.seam","Invalid artificial overlap length")?;
        let graph=format!("[0:a]atrim=start_sample={}:end_sample={},asetpts=N/SR/TB,afade=t=out:ss=0:ns={count}:curve=tri[o];[1:a]atrim=end_sample={count},asetpts=N/SR/TB,afade=t=in:ss=0:ns={count}:curve=tri[i];[o][i]amix=inputs=2:weights='1 1':normalize=0:dropout_transition=0:duration=longest,asetpts=N/SR/TB[out]",first-wa.first,end-wa.first);
        let dest=self.dir.join(format!("seam-{track}.f32"));let mut args=base();raw_input(&mut args,&a,OUTPUT_RATE);raw_input(&mut args,&b,OUTPUT_RATE);
        run_stage(self.dir,args,&graph,&dest,bytes(count)?,wa.output_bytes()?.checked_add(wb.output_bytes()?).ok_or_else(arithmetic)?,self.cancel,self.launch)?;
        self.caches[track].seam=Some((left,dest.clone()));Ok(dest)
    }
    fn track_slot(&mut self,kind:AudioTrackKind,first:u64,end:u64,dest:&Path)->CoreResult<()>{
        remove(dest)?;let mut output=new_file(dest)?;let mut cursor=first;
        for span in self.audio.spans.iter().filter(|s|s.kind==kind){
            let a=(self.audio.first_sample+span.output_first_sample).max(first);let b=(self.audio.first_sample+span.output_first_sample+span.sample_count).min(end);if a>=b{continue;}
            if cursor<a{zeros(&mut output,bytes(a-cursor)?,self.cancel)?;}
            if let Some(source)=&span.source{
                let mut n=a;
                while n<b{
                    check_cancel(self.cancel)?;
                    let number=(n-source.domain_first_sample)/WINDOW;
                    let current=self.audio.windows.iter().position(|w|w.kind==kind && w.domain==source.domain && w.number==number).ok_or_else(||invalid("audio.windows","Missing full-plan audio window"))?;
                    let w=&self.audio.windows[current];
                    let left=if w.central_first>source.domain_first_sample{128_u64.min(w.central_first-source.domain_first_sample).min(source.domain_end_sample-w.central_first)}else{0};
                    let right=if w.central_end<source.domain_end_sample{128_u64.min(w.central_end-source.domain_first_sample).min(source.domain_end_sample-w.central_end)}else{0};
                    let (path,start,stop)=if left>0 && n<w.central_first+left{
                        let peer=self.audio.windows.iter().position(|other|other.kind==kind && other.domain==source.domain && other.number+1==number).ok_or_else(||invalid("audio.windows","Missing preview overlap peer"))?;
                        let first=w.central_first-left;let stop=(w.central_first+left).min(b);(self.seam(peer,current)?,n-first,stop)
                    }else if right>0 && n>=w.central_end-right{
                        let peer=self.audio.windows.iter().position(|other|other.kind==kind && other.domain==source.domain && other.number==number+1).ok_or_else(||invalid("audio.windows","Missing preview overlap peer"))?;
                        let first=w.central_end-right;let stop=(w.central_end+right).min(b);(self.seam(current,peer)?,n-first,stop)
                    }else{let start=n-w.first;let stop=(w.central_end-right).min(b);(self.window(current)?,start,stop)};
                    require(stop>n,"audio.windows","Audio reconstruction did not advance")?;copy_range(&path,start,stop-n,&mut output,self.cancel)?;n=stop;
                }
            }else{zeros(&mut output,bytes(b-a)?,self.cancel)?;}
            cursor=b;
        }
        if cursor<end{zeros(&mut output,bytes(end-cursor)?,self.cancel)?;}output.flush().map_err(io)?;drop(output);length(dest,bytes(end-first)?)
    }
}

/// One owned child at a time; only two normalized windows per track remain queued.
pub fn normalize(audio:&AudioRenderPlan,job_dir:&Path,cancel:&impl Fn()->CoreResult<bool>,launch:&impl Fn(&Path,Vec<OsString>)->Command)->CoreResult<Option<PreparedAudio>>{
    check_cancel(cancel)?;if audio.tracks.is_empty(){return Ok(None);}
    require(audio.scratch_upper<=SCRATCH_LIMIT && bytes(audio.total_samples)?<=audio.scratch_upper && audio.first_sample.checked_add(audio.total_samples).is_some_and(|n|n<=crate::project::MAX_DURATION_US*48_000/1_000_000),"audio.scratch","Audio scratch exceeds the compiled 64 GiB budget; export a shorter range")?;
    let dir=job_dir.join("audio-scratch");match fs::create_dir(&dir){Ok(())=>(),Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>return Err(CoreError{code:ErrorCode::InvalidPath,message:"Reserved audio scratch directory already exists; retry a new job".into(),field:Some("audio.scratch".into())}),Err(e)=>return Err(io(e))};
    let prepared=PreparedAudio{dir};let mut final_output=new_file(&prepared.path())?;
    let mut session=Session{audio,dir:&prepared.dir,cancel,launch,caches:[TrackCache::default(),TrackCache::default()]};
    let last=audio.first_sample.checked_add(audio.total_samples).ok_or_else(arithmetic)?;let mut first=audio.first_sample;
    while first<last{
        check_cancel(cancel)?;let end=first.saturating_add(SLOT).min(last);let mut slots=Vec::with_capacity(2);
        for track in &audio.tracks{let path=prepared.dir.join(format!("track-{}.f32",track_number(track.kind)));session.track_slot(track.kind,first,end,&path)?;slots.push(path);}
        let count=end-first;
        if slots.len()==1{copy_range(&slots[0],0,count,&mut final_output,cancel)?;}else{
            let mixed=prepared.dir.join("mixed.f32");let mut args=base();for path in &slots{raw_input(&mut args,path,OUTPUT_RATE);}
            run_stage(&prepared.dir,args,"[0:a][1:a]amix=inputs=2:weights='0.5 0.5':normalize=0:dropout_transition=0:duration=longest,asetpts=N/SR/TB[out]",&mixed,bytes(count)?,bytes(count)?.checked_mul(2).ok_or_else(arithmetic)?,cancel,launch)?;
            copy_range(&mixed,0,count,&mut final_output,cancel)?;remove(&mixed)?;
        }
        for path in slots{remove(&path)?;}first=end;
    }
    final_output.flush().map_err(io)?;length(&prepared.path(),bytes(audio.total_samples)?)?;
    // Only the final stream survives until the owning caller completes AAC mux/publication.
    drop(session);for entry in fs::read_dir(&prepared.dir).map_err(io)?{let path=entry.map_err(io)?.path();if path!=prepared.path(){remove(&path)?;}}
    Ok(Some(prepared))
}
