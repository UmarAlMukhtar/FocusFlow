//! Independent hound decoding and integer-recipe content oracle. No generator calls.
//! A fitted content offset is diagnostic matching only, never source timing authority.
use crate::Result;
const RATE:usize=48000;
const BURSTS:[(usize,usize);6]=[(300,757),(1100,1831),(2345,3602),(4371,5124),(6237,8116),(9031,10893)];
#[derive(Debug,serde::Serialize)]
pub struct Analysis { pub status: &'static str, pub frames: u64, pub diagnostic_offset_frames: Option<usize>, pub reason: String, pub correlations: Vec<[f64;2]>, pub gains: Vec<[f64;2]>, pub silence_rms: Option<f64> }
fn result(status:&'static str,frames:u64,reason:&str)->Analysis{Analysis{status,frames,diagnostic_offset_frames:None,reason:reason.into(),correlations:vec![],gains:vec![],silence_rms:None}}
fn expected(frame:usize,channel:usize)->f64 {
    let mut state=1u32 ^ (frame as u32).wrapping_mul(2654435769) ^ (channel as u32).wrapping_mul(2246822507);
    state ^= state.wrapping_shl(13);state ^= state.wrapping_shr(17);state ^= state.wrapping_shl(5);
    let signed=((state/65536) as i32-32768)/4;
    f64::from(signed)/32768.0
}
fn correlation(actual:impl Iterator<Item=f64>,reference:impl Iterator<Item=f64>)->(f64,f64){
    let(mut dot,mut xx,mut yy)=(0.0,0.0,0.0);
    for(y,x)in actual.zip(reference){dot+=x*y;xx+=x*x;yy+=y*y;}
    if xx<=0.0||yy<=0.0{return(0.0,0.0);}(dot/(xx*yy).sqrt(),dot/xx)
}
pub fn inspect(path:&std::path::Path,expected_frames:u64)->Result<Analysis>{
    let mut reader=hound::WavReader::open(path).map_err(|e|format!("Captured WAV is unreadable: {e}"))?;
    let spec=reader.spec();let frames=u64::from(reader.duration());
    if frames==0||frames!=expected_frames{return Err("Independent WAV frame count does not match the reconciled worker inventory".into());}
    if spec.sample_rate!=48000||spec.channels!=2{return Ok(result("Unavailable",frames,"This minimal content oracle supports existing 48kHz stereo routes only; do not change device settings for this test"));}
    if frames<12*RATE as u64||frames>60*RATE as u64{return Err("Captured WAV is outside the bounded 12..60 second content-analysis window".into());}
    let mut samples=Vec::with_capacity(frames as usize*2);
    match(spec.sample_format,spec.bits_per_sample){
        (hound::SampleFormat::Int,bits @ (16|24|32))=>{let scale=2f64.powi(i32::from(bits)-1);for sample in reader.samples::<i32>(){samples.push((f64::from(sample.map_err(|e|e.to_string())?)/scale)as f32);}}
        (hound::SampleFormat::Float,32)=>{for sample in reader.samples::<f32>(){samples.push(sample.map_err(|e|e.to_string())?);}}
        _=>return Err("Captured WAV encoding is unsupported".into()),
    }
    if samples.len()!=frames as usize*2||samples.iter().any(|x|!x.is_finite()||x.abs()>1.01){return Err("Captured WAV samples are incomplete, non-finite or outside the supported range".into());}
    // Search one sparse nonperiodic left-channel signature, then independently verify every burst/channel.
    let anchor=400*RATE/1000;let reference:Vec<_>=(0..32).map(|i|expected(anchor+i*61,0)).collect();
    let(mut best,mut offset)=(0.0,0);
    for candidate in 0..=frames as usize-12*RATE {
        let(score,_)=correlation((0..32).map(|i|f64::from(samples[(candidate+anchor+i*61)*2])),reference.iter().copied());
        if score>best{best=score;offset=candidate;}
    }
    if best<0.98{return Ok(result("Failed",frames,"Known playback signature was not found; playback route, format, effects, volume or other sounds may differ"));}
    let mut report=result("Pass",frames,"Known six-burst per-channel source-content signature matched. This does not prove physical playback route, exact continuity, or A/V timing.");
    report.diagnostic_offset_frames=Some(offset);
    for(start,end)in BURSTS{
        let range=start*RATE/1000..end*RATE/1000;let(mut scores,mut gains)=([0.0;2],[0.0;2]);
        for channel in 0..2{
            let(score,gain)=correlation(range.clone().map(|frame|f64::from(samples[(offset+frame)*2+channel])),range.clone().map(|frame|expected(frame,channel)));
            scores[channel]=score;gains[channel]=gain;
            if score<0.99||!(0.001..=4.0).contains(&gain){report.status="Failed";report.reason="At least one known burst/channel did not match the fixed content threshold".into();}
        }
        report.correlations.push(scores);report.gains.push(gains);
    }
    let(mut silence,mut count)=(0.0,0u64);
    for frame in 0..12*RATE{
        if BURSTS.iter().any(|(start,end)|frame>=start*RATE/1000&&frame<end*RATE/1000){continue;}
        for channel in 0..2{let sample=f64::from(samples[(offset+frame)*2+channel]);silence+=sample*sample;count+=1;}
    }
    let silence=(silence/count as f64).sqrt();report.silence_rms=Some(silence);
    if silence>0.002 {report.status="Failed";report.reason="Unexpected sound in the reference's silent intervals".into();}
    Ok(report)
}

pub fn inspect_inventory(path:&std::path::Path,inventory:&focusflow_windows_audio_worker::protocol::WavInventory)->Result<Analysis>{
    let reader=hound::WavReader::open(path).map_err(|e|e.to_string())?;let spec=reader.spec();
    let encoding=match spec.sample_format{hound::SampleFormat::Int=>crate::format::Encoding::Pcm,hound::SampleFormat::Float=>crate::format::Encoding::Float};
    if spec.sample_rate!=inventory.rate||spec.channels!=inventory.channels||spec.bits_per_sample!=inventory.bits||encoding!=inventory.encoding||std::fs::metadata(path).map_err(|e|e.to_string())?.len()!=inventory.file_bytes {
        return Err("Independent WAV header/size does not match the reconciled worker inventory".into());
    }
    drop(reader);inspect(path,inventory.frames)
}
