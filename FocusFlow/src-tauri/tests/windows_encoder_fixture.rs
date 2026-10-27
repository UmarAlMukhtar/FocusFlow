//! Harmless generated fault encoder. Validates the unchanged product argv; delegates profile probe.
use std::{fs, io::{self,Read}, path::PathBuf, process::{Command,Stdio}, time::Duration};
fn run()->Result<(),Box<dyn std::error::Error>> {
 let args:Vec<_>=std::env::args_os().skip(1).collect();
 let config:serde_json::Value=serde_json::from_slice(&fs::read(std::env::current_exe()?.with_extension("json"))?)?;
 let real=PathBuf::from(config["realFfmpeg"].as_str().ok_or("realFfmpeg missing")?);
 let root=PathBuf::from(config["evidence"].as_str().ok_or("evidence missing")?);
 let text:Vec<_>=args.iter().map(|x|x.to_str().ok_or("non UTF8 fixture argv")).collect::<Result<_,_>>()?;
 if text.len()!=37 {return Err(format!("unexpected profile argument count: {}",text.len()).into());}
 assert_eq!(&text[..10],["-hide_banner","-loglevel","error","-nostdin","-n","-f","rawvideo","-pixel_format","bgra","-video_size"]);
 assert_eq!(&text[11..36],["-framerate","30","-i","pipe:0","-map","0:v:0","-an","-c:v","libx264","-preset","veryfast","-crf","23","-pix_fmt","yuv420p","-bf","0","-fps_mode","passthrough","-video_track_timescale","30000","-movflags","+faststart","-f","mp4"]);
 let output=PathBuf::from(&args[36]);let name=output.file_name().ok_or("no output")?;
 if name=="probe.mp4" {
  assert_eq!(text[10],"2x2");
  let mut relay=Command::new(&real).args(&args).stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit()).spawn()?;
  let pid=relay.id();let status=relay.wait()?;
  fs::write(root.join(format!("profile-relay-{}.json",std::process::id())),serde_json::to_vec(&serde_json::json!({"fixture":std::process::id(),"relay":pid,"exit":status.code(),"waited":true}))?)?;
  std::process::exit(status.code().unwrap_or(1));
 }
 if name!="screen.mp4" {return Err("fixture only accepts product probe.mp4 or generated screen.mp4".into());}
 let case=output.parent().ok_or("case missing")?;let mode=config["mode"].as_str().ok_or("mode missing")?;
 fs::write(case.join("encoder-fixture.json"),serde_json::to_vec(&serde_json::json!({"pid":std::process::id(),"mode":mode,"argv":text}))?)?;
 match mode {
  "blocked"|"parent-death"=>{std::thread::sleep(Duration::from_secs(30));return Err("fixture escape reached".into());},
  "partial"=>{let mut prefix=[0u8;65536];io::stdin().read_exact(&mut prefix)?;fs::write(case.join("consumer-prefix.json"),b"{\"consumerBytes\":65536,\"notWriterProgress\":true}")?;std::process::exit(7);},
  "exit7"=>{let mut total=0usize;let mut buf=[0;65536];loop{let n=io::stdin().read(&mut buf)?;if n==0{break}total+=n;}fs::write(case.join("consumer-prefix.json"),format!("{{\"consumerBytes\":{total},\"notWriterProgress\":true,\"eof\":true}}"))?;std::process::exit(7);},
  _=>return Err("Unknown generated encoder mode".into()),
 }
}
fn main(){if let Err(error)=run(){eprintln!("{error}");std::process::exit(1);}}
