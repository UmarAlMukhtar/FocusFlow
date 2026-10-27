use super::*;
use super::super::video_clock::AcceptedClock;
use focusflow_core::{cursor, ffmpeg::PacketTime, project::ProjectDocument};
use std::sync::atomic::{AtomicU64, Ordering, AtomicBool};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("focusflow-finalize-{}-{}", std::process::id(), NEXT.fetch_add(1,Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();fs::create_dir(path.join("cursor")).unwrap();fs::create_dir(path.join("cursor/shapes")).unwrap();
        for name in ["screen.mp4","capture.json",".unrelated.tmp"] { fs::write(path.join(name),name).unwrap(); }
        Self(path)
    }
    fn raw(&self, rows:&[RawRecord]) -> Outcome {
        let bytes:Vec<_>=rows.iter().flat_map(|r|{let mut b=serde_json::to_vec(r).unwrap();b.push(b'\n');b}).collect();
        fs::write(self.0.join("cursor/raw.ndjson"),&bytes).unwrap();
        Outcome{raw_path:self.0.join("cursor/raw.ndjson"),committed_bytes:bytes.len() as u64,records:rows.len(),shapes:vec![],cutoff_ns:None,trustworthy_prefix:true,degraded:false,reason:None}
    }
    fn rows(&self)->Vec<serde_json::Value> {
        fs::read_to_string(self.0.join("cursor/samples.ndjson")).unwrap().lines().map(|l|serde_json::from_str(l).unwrap()).collect()
    }
}
impl Drop for Fixture {fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
fn source()->Source {Source{bounds:CaptureBounds{x:10,y:20,width:100,height:50},window:None}}
fn clock()->AcceptedClock {AcceptedClock{frame_zero_ns:1_000_000_000,anchor_low_ns:999_999_999,anchor_high_exclusive_ns:1_000_000_002,completed_frames:30,saved_endpoint:PacketTime{ticks:30,numerator:1,denominator:30}}}
fn observed(ns:i128,x:f64)->RawRecord {RawRecord{before_ns:1_000_000_000+ns,after_ns:1_000_000_000+ns,state:RawState::Observed{x,y:0.5,visible:true,shape_id:None,root:1,origin_x:10,origin_y:20,discontinuity:false}}}
fn barrier(ns:i128)->RawRecord {RawRecord{state:RawState::Barrier{},..observed(ns,0.0)}}
fn run(rows:&[RawRecord],cutoff:Option<i128>)->(Fixture,FinalizedCursor) {
    let f=Fixture::new();let mut o=f.raw(rows);o.cutoff_ns=cutoff.map(|n|n+1_000_000_000);
    let result=finalize(&f.0,source(),o,Some(&clock())).unwrap();(f,result)
}
#[test]
fn held_predecessor_expires_at_original_age_and_real_zero_wins() {
    let (f,result)=run(&[observed(-100_000_000,0.25)],None);
    assert!(matches!(result,FinalizedCursor::Separate{..}));
    let rows=f.rows();assert_eq!(rows.len(),2);assert_eq!(rows[0]["atUs"],0);assert_eq!(rows[0]["x"],0.25);assert_eq!(rows[0]["discontinuity"],true);
    assert_eq!(rows[1]["atUs"],150_000);assert_eq!(rows[1]["visible"],false);
    let (f,_)=run(&[observed(-100_000_000,0.25),observed(0,0.75)],None);
    let rows=f.rows();assert_eq!(rows.len(),1);assert_eq!(rows[0]["x"],0.75);
}
fn doc(bounds:CaptureBounds,duration_us:u64)->ProjectDocument {
    use focusflow_core::project::*;
    ProjectDocument{schema_version:3,project_id:"fixture".into(),revision:0,frame_style:Default::default(),
        video:VideoAsset{cursor:CursorMode::Separate,path:"screen.mp4".into(),width:bounds.width,height:bounds.height,duration_us},
        cursor:Some(CursorAsset{index_path:"cursor/index.json".into()}),cursor_settings:CursorSettings{enabled:true,style:CursorStyle::Arrow,..Default::default()},
        microphone:None,timing:Timing{provenance:TimingProvenance::LegacyUnknown,audio_offset_us:None},camera:CameraDefaults{pan_transition_us:0,settle_us:0},
        clips:vec![Clip{id:"clip-0".into(),in_us:0,out_us:duration_us}],zooms:vec![],clicks:vec![],warnings:vec![]}
}
fn consumer(f:&Fixture)->std::sync::Arc<cursor::PreparedCursor> {
    let index:CursorIndex=serde_json::from_slice(&fs::read(f.0.join("cursor/index.json")).unwrap()).unwrap();
    cursor::prepare_cursor(&f.0,&doc(source().bounds,index.duration_us),&AtomicBool::new(false)).unwrap().unwrap()
}
#[test]
fn core_evaluator_observes_original_age_at_zero_and_thirty_sixty_fps() {
    let (f,_)=run(&[observed(-100_000_000,0.25)],None);let p=consumer(&f);
    assert_eq!(p.sample_at(0,1).unwrap().x,0.25);
    for (fps,last_visible,first_hidden) in [(30,4,5),(60,8,9)] {
        assert!(p.sample_at(last_visible*1_000_000,fps).is_some());assert!(p.sample_at(first_hidden*1_000_000,fps).is_none());
    }
    assert!(p.sample_at(149_999,1).is_some());assert!(p.sample_at(150_000,1).is_none());
}
#[test]
fn age_boundary_hidden_predecessor_barrier_and_moved_origin_never_resurrect() {
    let (_,result)=run(&[observed(-100_000_001,0.25)],None);assert!(matches!(result,FinalizedCursor::Absent{..}));
    let mut hidden=observed(-100_000_000,0.25);if let RawState::Observed{visible,..}=&mut hidden.state {*visible=false;}
    let (f,_)=run(&[hidden],None);assert_eq!(f.rows()[0]["visible"],false);assert!(consumer(&f).sample_at(0,1).is_none());
    let (f,_)=run(&[observed(-90_000_000,0.25),barrier(-10_000_000),observed(20_000_000,0.75)],None);
    assert_eq!(f.rows().len(),1);assert_eq!(f.rows()[0]["atUs"],20_000);assert!(consumer(&f).sample_at(0,1).is_none());
    let f=Fixture::new();let mut moved=observed(-10_000_000,0.5);if let RawState::Observed{origin_x,..}=&mut moved.state {*origin_x=30;}
    let mut fresh=moved.clone();fresh.before_ns=1_020_000_000;fresh.after_ns=fresh.before_ns;
    let o=f.raw(&[observed(-90_000_000,0.25),moved,fresh]);let mut s=source();s.window=Some(7);
    assert!(matches!(finalize(&f.0,s,o,Some(&clock())).unwrap(),FinalizedCursor::Separate{..}));
    assert_eq!(f.rows().len(),1);assert_eq!(f.rows()[0]["atUs"],20_000);
}
#[test]
fn freshness_wins_exact_expiry_and_later_same_microsecond_keeps_discontinuity() {
    for (ns,discontinuity) in [(149_999_999,false),(150_000_000,false),(150_000_001,true)] {
        let (f,_)=run(&[observed(-100_000_000,0.25),observed(ns,0.75)],None);
        let rows=f.rows();assert_eq!(rows.len(),2);assert_eq!(rows[1]["visible"],true);assert_eq!(rows[1]["discontinuity"],discontinuity);
        assert_eq!(consumer(&f).sample_at(150_001,1).unwrap().x,0.75);
    }
    let (f,_)=run(&[observed(-100_000_000,0.25),observed(150_001_000,0.75)],None);
    let rows=f.rows();assert_eq!(rows.len(),3);assert_eq!(rows[1]["visible"],false);assert_eq!(rows[2]["discontinuity"],true);
}
#[test]
fn cutoff_wins_ties_forbids_post_cutoff_resumption_and_never_clamps() {
    for cutoff in [-1,0] {let (_,r)=run(&[observed(-100_000_000,0.25),observed(0,0.75)],Some(cutoff));assert!(matches!(r,FinalizedCursor::Absent{..}));}
    for (cutoff,times) in [(149_999_999,vec![0,149_999]),(150_000_000,vec![0,150_000]),(150_000_001,vec![0,150_000]),(150_001_000,vec![0,150_000,150_001]),(1_000_000_000,vec![0,150_000]),(1_000_000_001,vec![0,150_000])] {
        let (f,_)=run(&[observed(-100_000_000,0.25),observed(cutoff.max(0),0.75),observed(cutoff.max(0)+1,0.9)],Some(cutoff));
        let rows=f.rows();assert_eq!(rows.iter().map(|r|r["atUs"].as_u64().unwrap()).collect::<Vec<_>>(),times,"{cutoff}");assert_eq!(rows.last().unwrap()["visible"],false);
    }
    let (f,_)=run(&[observed(0,0.25),observed(1_000_000_000,0.9)],Some(1_000_000_000));assert_eq!(f.rows().len(),1);
}
#[test]
fn exact_equivalent_rationals_and_fractional_tail_are_half_open() {
    for endpoint in [PacketTime{ticks:3,numerator:1,denominator:2},PacketTime{ticks:30,numerator:1,denominator:20}] {
        let f=Fixture::new();let o=f.raw(&[observed(0,0.1),observed(1_499_999_999,0.2),observed(1_500_000_000,0.3)]);let mut c=clock();c.saved_endpoint=endpoint;
        finalize(&f.0,source(),o,Some(&c)).unwrap();assert_eq!(f.rows().iter().map(|r|r["atUs"].as_u64().unwrap()).collect::<Vec<_>>(),vec![0,1_499_999]);
    }
    // Arbitrary fractions test conversion arithmetic, not encoder-grid acceptance.
    for (ticks,times) in [(150_000_999,vec![0]),(150_001_000,vec![0,150_000])] {
        let f=Fixture::new();let o=f.raw(&[observed(-99_999_999,0.5)]);let mut c=clock();c.saved_endpoint=PacketTime{ticks,numerator:1,denominator:1_000_000_000};
        finalize(&f.0,source(),o,Some(&c)).unwrap();assert_eq!(f.rows().iter().map(|r|r["atUs"].as_u64().unwrap()).collect::<Vec<_>>(),times);
    }
    let f=Fixture::new();let o=f.raw(&[observed(149_999_999,0.1),observed(150_000_000,0.2),observed(150_000_499,0.3),observed(150_000_500,0.4)]);let mut c=clock();c.saved_endpoint=PacketTime{ticks:150_000_500,numerator:1,denominator:1_000_000_000};
    finalize(&f.0,source(),o,Some(&c)).unwrap();assert_eq!(f.rows().len(),1);assert_eq!(f.rows()[0]["atUs"],149_999);
}
#[test]
fn midpoint_equal_times_and_same_microsecond_use_last_state_and_or_discontinuity() {
    let mut first=observed(1000,0.1);first.after_ns+=1000;
    let mut second=observed(1999,0.2);second.before_ns=1_000_002_000;second.after_ns=second.before_ns;
    let mut third=observed(2000,0.3);if let RawState::Observed{discontinuity,..}=&mut third.state {*discontinuity=true;}
    let (f,_)=run(&[first,second,third,observed(2999,0.4)],None);
    assert_eq!(f.rows().len(),2);assert_eq!(f.rows()[0]["atUs"],1);assert_eq!(f.rows()[1]["atUs"],2);assert_eq!(f.rows()[1]["x"],0.4);assert_eq!(f.rows()[1]["discontinuity"],true);
}
#[test]
fn fresh_stationary_rows_are_kept_and_do_not_become_cached_refreshes() {
    let rows:Vec<_>=(0..5).map(|n|observed(n*50_000_000,0.5)).collect();let (f,_)=run(&rows,None);
    assert_eq!(f.rows().len(),5);let p=consumer(&f);assert!(p.sample_at(449_999,1).is_some());assert!(p.sample_at(450_000,1).is_none());
}
#[test]
fn untrusted_empty_missing_and_wrong_raw_path_are_absent_without_asset_writes() {
    for case in 0..6 {
        let f=Fixture::new();let mut o=f.raw(&[observed(0,0.5)]);
        match case {0=>o.trustworthy_prefix=false,1=>o.committed_bytes=0,2=>o.records=0,3=>fs::remove_file(&o.raw_path).unwrap(),4=>o.raw_path=f.0.join("screen.mp4"),_=>{}}
        let c=clock();let r=finalize(&f.0,source(),o,if case==5 {None} else {Some(&c)}).unwrap();assert!(matches!(r,FinalizedCursor::Absent{..}));
        assert!(!f.0.join("cursor/samples.ndjson").exists());assert!(!f.0.join("cursor/index.json").exists());
    }
}
#[test]
fn uncommitted_trailing_bytes_are_ignored_and_preserved() {
    let f=Fixture::new();let o=f.raw(&[observed(0,0.5)]);let mut raw=fs::read(&o.raw_path).unwrap();raw.extend(b"partial { invalid\n");fs::write(&o.raw_path,&raw).unwrap();
    finalize(&f.0,source(),o,Some(&clock())).unwrap();assert_eq!(f.rows().len(),1);assert_eq!(fs::read(f.0.join("cursor/raw.ndjson")).unwrap(),raw);
}
#[test]
fn malformed_committed_middle_terminates_without_resuming() {
    let bads=["{bad}\n".to_string(),serde_json::to_string(&observed(2000,0.5)).unwrap().replace("1000002000","01000002000")+"\n",
        serde_json::to_string(&observed(2000,0.5)).unwrap().replace("\"root\":1","\"root\":0")+"\n",
        serde_json::to_string(&observed(2000,0.5)).unwrap().replace("\"root\":1","\"root\":2")+"\n",
        serde_json::to_string(&observed(2000,0.5)).unwrap().replace("\"shapeId\":null","\"shapeId\":\"shape-000\"")+"\n",
        serde_json::to_string(&observed(2000,50.0)).unwrap()+"\n",
        serde_json::to_string(&observed(2000,0.5)).unwrap().replace("\"originX\":10","\"originX\":11")+"\n",
        serde_json::to_string(&observed(-1,0.5)).unwrap()+"\n"];
    for bad in bads {
        let f=Fixture::new();let mut o=f.raw(&[observed(0,0.1)]);let mut raw=fs::read(&o.raw_path).unwrap();raw.extend(bad.as_bytes());raw.extend(serde_json::to_vec(&observed(5000,0.9)).unwrap());raw.push(b'\n');o.records=3;o.committed_bytes=raw.len() as u64;fs::write(&o.raw_path,&raw).unwrap();
        assert!(matches!(finalize(&f.0,source(),o,Some(&clock())).unwrap(),FinalizedCursor::Separate{degraded:Some(_)}));
        assert_eq!(f.rows().len(),1);assert_eq!(f.rows()[0]["visible"],false);assert_eq!(fs::read(f.0.join("cursor/raw.ndjson")).unwrap(),raw);
    }
}
#[test]
fn committed_metadata_mismatch_and_truncated_line_close_at_last_bracket_end() {
    for case in 0..4 {
        let f=Fixture::new();let mut o=f.raw(&[observed(1000,0.1),observed(2000,0.2)]);
        match case {0=>o.records+=1,1=>o.records-=1,2=>o.committed_bytes+=1,_=>o.committed_bytes-=1}
        assert!(matches!(finalize(&f.0,source(),o,Some(&clock())).unwrap(),FinalizedCursor::Separate{degraded:Some(_)}));
        assert_eq!(f.rows().last().unwrap()["visible"],false);
    }
    let mut too_wide=observed(-50_000_000,0.2);too_wide.after_ns+=20_000_001;
    let (_,r)=run(&[observed(-90_000_000,0.1),too_wide,observed(0,0.3)],None);assert!(matches!(r,FinalizedCursor::Absent{..}));
}
#[test]
fn raw_count_byte_and_line_limits_are_independent_from_final_reserve() {
    for kind in ["rows","bytes","line"] {
        for extra in [false,true] {
            let f=Fixture::new();let mut o=f.raw(&[observed(1000,0.1),observed(2000,0.2)]);let bytes=fs::read(&o.raw_path).unwrap();let first=bytes.iter().position(|b|*b==b'\n').unwrap()+1;
            let mut raw=Limits::default();match kind {"rows"=>raw.rows=if extra {1} else {2},"bytes"=>raw.raw_bytes=if extra {o.committed_bytes-1} else {o.committed_bytes},_=>raw.line=if extra {first-1} else {first}}
            // Shape-free hidden states avoid position-only degradation masking the boundary assertion.
            let rows=bytes.split_inclusive(|b|*b==b'\n').map(|b|String::from_utf8(b.to_vec()).unwrap().replace("true","false")).collect::<String>();fs::write(&o.raw_path,rows.as_bytes()).unwrap();o.committed_bytes=rows.len() as u64;
            if kind=="bytes" {raw.raw_bytes=if extra {o.committed_bytes-1} else {o.committed_bytes};}
            if kind=="line" {raw.line=rows.lines().next().unwrap().len()+1-usize::from(extra);}
            let r=finalize_with(&f.0,source(),o,Some(&clock()),raw,Limits::default(),|_,_|Ok(())).unwrap();
            if extra&&kind=="line" {assert!(matches!(r,FinalizedCursor::Absent{..}));}
            else if extra {assert!(matches!(r,FinalizedCursor::Separate{degraded:Some(_)}));}
            else {assert_eq!(r,FinalizedCursor::Separate{degraded:None});assert_eq!(f.rows().len(),2);}
        }
    }
}
#[test]
fn three_synthetic_reserves_survive_ordinary_row_exhaustion() {
    let f=Fixture::new();let mut o=f.raw(&[observed(-100_000_000,0.1),observed(160_000_000,0.2),observed(170_000_000,0.3)]);o.cutoff_ns=Some(1_180_000_000);
    let mut limits=Limits::default();limits.rows=4;
    let r=finalize_with(&f.0,source(),o,Some(&clock()),Limits::default(),limits,|_,_|Ok(())).unwrap();assert!(matches!(r,FinalizedCursor::Separate{degraded:Some(_)}));
    let rows=f.rows();assert_eq!(rows.iter().map(|r|r["atUs"].as_u64().unwrap()).collect::<Vec<_>>(),vec![0,150_000,160_000]);assert_eq!(rows[2]["visible"],false);
}
#[test]
fn final_row_byte_line_and_index_boundaries_preserve_owned_assets() {
    let row=FinalRow{at_us:1,x:0.5,y:0.5,visible:true,shape_id:None,discontinuity:true};let length=row.bytes().unwrap().len();
    for kind in ["rows","bytes","line","index"] {for extra in [false,true] {
        let f=Fixture::new();let o=f.raw(&[observed(1000,0.5)]);let mut limits=Limits::default();
        let index=serde_json::to_vec(&CursorIndex{schema_version:1,width:100,height:50,duration_us:1_000_000,samples_path:"cursor/samples.ndjson".into(),shapes:vec![]}).unwrap();
        match kind {"rows"=>limits.rows=4-usize::from(extra),"bytes"=>limits.raw_bytes=3*MAX_RECORD_BYTES as u64+length as u64-u64::from(extra),"line"=>limits.line=length-usize::from(extra),_=>limits.index=index.len() as u64-u64::from(extra)}
        let r=finalize_with(&f.0,source(),o,Some(&clock()),Limits::default(),limits,|_,_|Ok(()));
        if extra&&kind=="index" {assert!(r.is_err());}else if extra {assert!(matches!(r.unwrap(),FinalizedCursor::Absent{..}));}else {assert!(matches!(r.unwrap(),FinalizedCursor::Separate{..}),"{kind}");}
        for name in ["screen.mp4","capture.json",".unrelated.tmp"] {assert_eq!(fs::read(f.0.join(name)).unwrap(),name.as_bytes());}
    }}
}
#[test]
fn asset_write_and_rename_failures_remove_only_owned_temps() {
    for failure in ["samples-write","samples-rename","index-rename"] {
        let f=Fixture::new();let o=f.raw(&[observed(0,0.5)]);let raw=fs::read(&o.raw_path).unwrap();fs::write(f.0.join("cursor/.unrelated.tmp"),b"keep").unwrap();
        let r=finalize_with(&f.0,source(),o,Some(&clock()),Limits::default(),Limits::default(),|stage,path|if stage==failure {if stage=="samples-write" {fs::write(path,b"partial")?;}Err(std::io::Error::other("injected"))} else {Ok(())});assert!(r.is_err());
        assert_eq!(f.0.join("cursor/samples.ndjson").exists(),failure=="index-rename");assert!(!f.0.join("cursor/index.json").exists());
        for name in [".samples.finalizing.tmp",".index.finalizing.tmp"] {assert!(!f.0.join("cursor").join(name).exists());}
        assert_eq!(fs::read(f.0.join("cursor/raw.ndjson")).unwrap(),raw);assert_eq!(fs::read(f.0.join("cursor/.unrelated.tmp")).unwrap(),b"keep");
        assert_eq!(fs::read(f.0.join("capture.json")).unwrap(),b"capture.json");
    }
}
#[test]
fn final_assets_and_foreign_temps_are_never_replaced_including_symlinks() {
    for name in ["samples.ndjson","index.json",".samples.finalizing.tmp",".index.finalizing.tmp"] {for alias in [false,true] {
        let f=Fixture::new();let o=f.raw(&[observed(0,0.5)]);let path=f.0.join("cursor").join(name);
        if alias {std::os::unix::fs::symlink(f.0.join("screen.mp4"),&path).unwrap();}else {fs::write(&path,b"existing").unwrap();}
        assert!(finalize(&f.0,source(),o,Some(&clock())).is_err());assert_eq!(fs::read(&path).unwrap(),if alias {b"screen.mp4".as_slice()} else {b"existing".as_slice()});assert_eq!(fs::read(f.0.join("screen.mp4")).unwrap(),b"screen.mp4");
    }}
}
#[test]
fn seeded_terminal_cutoff_is_disabled_even_without_an_owner_error() {
    let mut hidden=observed(-100_000_000,0.5);if let RawState::Observed{visible,..}=&mut hidden.state {*visible=false;}
    let (f,r)=run(&[hidden],Some(140_000_000));
    assert!(matches!(r,FinalizedCursor::Separate{degraded:Some(_)}));assert_eq!(f.rows().last().unwrap()["atUs"],140_000);
}
#[test]
fn resource_failure_does_not_publish_expiry_after_its_earlier_terminal() {
    let f=Fixture::new();let o=f.raw(&[observed(-100_000_000,0.1),observed(160_000_000,0.2)]);let mut limits=Limits::default();limits.rows=3;
    assert!(matches!(finalize_with(&f.0,source(),o,Some(&clock()),Limits::default(),limits,|_,_|Ok(())).unwrap(),FinalizedCursor::Separate{..}));
    let rows=f.rows();assert_eq!(rows.len(),1);assert_eq!(rows[0]["visible"],false);assert_eq!(rows[0]["atUs"],0);
}
#[test]
fn all_three_synthetic_rows_fit_without_spending_ordinary_capacity() {
    let f=Fixture::new();let mut fresh=observed(160_000_000,0.2);fresh.after_ns+=2_000_000;
    let o=f.raw(&[observed(-100_000_000,0.1),fresh,observed(170_000_000,0.3)]);let mut limits=Limits::default();limits.rows=4;
    finalize_with(&f.0,source(),o,Some(&clock()),Limits::default(),limits,|_,_|Ok(())).unwrap();
    let rows=f.rows();assert_eq!(rows.iter().map(|r|r["atUs"].as_u64().unwrap()).collect::<Vec<_>>(),vec![0,150_000,161_000,162_000]);assert_eq!(rows[3]["visible"],false);
}
#[test]
fn terminal_same_microsecond_later_than_fresh_wins_and_shape_descriptors_are_preserved() {
    let f=Fixture::new();let mut o=f.raw(&[observed(0,0.1),observed(1000,0.2)]);o.cutoff_ns=Some(1_000_001_999);
    o.shapes.push(CursorShape{id:"shape-000".into(),path:"cursor/shapes/shape-000.pam".into(),width:1,height:1,hotspot_x:0,hotspot_y:0});
    fs::write(f.0.join("cursor/shapes/shape-000.pam"),b"retained owned provisional bytes").unwrap();
    finalize(&f.0,source(),o,Some(&clock())).unwrap();let rows=f.rows();assert_eq!(rows.len(),2);assert_eq!(rows[1]["atUs"],1);assert_eq!(rows[1]["visible"],false);
    let index:CursorIndex=serde_json::from_slice(&fs::read(f.0.join("cursor/index.json")).unwrap()).unwrap();assert_eq!(index.shapes.len(),1);assert_eq!(index.shapes[0].path,"cursor/shapes/shape-000.pam");assert_eq!(fs::read(f.0.join("cursor/shapes/shape-000.pam")).unwrap(),b"retained owned provisional bytes");
}
fn manifest_for(dir:&Path,s:Source,c:&AcceptedClock,evidence:&focusflow_core::ffmpeg::ProbedVideoEvidence,degraded:Option<&str>)->focusflow_core::capture::CaptureManifestV1 {
    let mut doc=doc(s.bounds,evidence.video.duration_us);doc.project_id=dir.file_name().unwrap().to_str().unwrap().into();
    doc.cursor_settings.style=focusflow_core::project::CursorStyle::Native;doc.cursor_settings.enabled=degraded.is_none();doc.warnings=vec![focusflow_core::project::ProjectWarning::LegacyTimingUnknown];
    if degraded.is_some() {doc.warnings.push(focusflow_core::project::ProjectWarning::CursorCaptureIncomplete);}
    serde_json::from_value(serde_json::json!({"schemaVersion":1,"captureId":doc.project_id,"source":{"backend":"linuxX11","kind":if s.window.is_some(){"window"}else{"region"},"initialBounds":{"x":s.bounds.x,"y":s.bounds.y,"width":s.bounds.width,"height":s.bounds.height},"windowId":s.window.map(|n|n.to_string())},"diagnostic":degraded.map(|message|serde_json::json!({"stage":"cursor","message":message})),
        "state":{"status":"finalized","videoTiming":{"endpoint":{"provenance":"savedPacketFirstAndTail","endPts":evidence.endpoint.ticks,"timeBase":{"numerator":evidence.endpoint.numerator,"denominator":evidence.endpoint.denominator},"interval":"halfOpen"},"clock":{"sourceClock":"unixMicrosecondsPreImageCopyRequest","epoch":"captureLaunchMonotonicNs","anchor":{"lowNs":c.anchor_low_ns.to_string(),"highExclusiveNs":c.anchor_high_exclusive_ns.to_string()},"completedFrames":c.completed_frames,"cadence":{"numerator":1,"denominator":30},"provenance":"completedEncoderGridAndSavedPacketCadence","interactions":"deliveryTimeRebasedOnce","audio":"unmeasured"}},"cursorTiming":{"clock":"videoFrameZeroRelativeUs","observation":"x11RequestBracketMidpoint","initialState":"boundedPreRollHold","visibility":"sameScreenNativeImageOrArrow1xIntersectsCrop","serverVisibility":"unknown"},"initialProject":doc}})).unwrap()
}
fn generated_video(dir:&Path,bounds:CaptureBounds)->focusflow_core::ffmpeg::ProbedVideoEvidence {
    let status=std::process::Command::new("ffmpeg").args(["-hide_banner","-loglevel","error","-y","-f","lavfi","-i",&format!("color=c=black:s={}x{}:r=30:d=1",bounds.width,bounds.height),"-c:v","libx264","-pix_fmt","yuv420p"]).arg(dir.join("screen.mp4")).status().unwrap();assert!(status.success());
    focusflow_core::ffmpeg::probe_video_evidence(Path::new("ffmpeg"),&dir.join("screen.mp4")).unwrap()
}
fn hash(path:&Path)->String {
    let output=std::process::Command::new("sha256sum").arg(path).output().unwrap();assert!(output.status.success());String::from_utf8(output.stdout).unwrap().split_whitespace().next().unwrap().into()
}
fn publish_import(dir:&Path,manifest:&focusflow_core::capture::CaptureManifestV1,evidence:&focusflow_core::ffmpeg::ProbedVideoEvidence)->serde_json::Value {
    use focusflow_core::{capture,store::ProjectStore};
    let source=fs::read(dir.join("screen.mp4")).unwrap();let raw=fs::read(dir.join("cursor/raw.ndjson")).unwrap();
    let committed=capture::publish_finalized_assets(dir,manifest,evidence).unwrap();assert!(!dir.join("project.json").exists());
    let authority=fs::read(dir.join("capture.json")).unwrap();let before=serde_json::json!({"screen.mp4":hash(&dir.join("screen.mp4")),"cursor/raw.ndjson":hash(&dir.join("cursor/raw.ndjson")),"capture.json":hash(&dir.join("capture.json"))});
    let store=ProjectStore::new(dir.parent().unwrap()).unwrap();let id=dir.file_name().unwrap().to_str().unwrap();
    let imported=store.open_or_import(id,Path::new("ffmpeg"),focusflow_core::legacy::LegacyDefaults{scale:2.0,intro_us:0,hold_us:0,outro_us:0,pan_transition_us:0,settle_us:0}).unwrap();
    assert_eq!(imported.revision,0);assert_eq!(&imported,committed.finalized_project().unwrap());
    assert_eq!(fs::read(dir.join("screen.mp4")).unwrap(),source);assert_eq!(fs::read(dir.join("cursor/raw.ndjson")).unwrap(),raw);assert_eq!(fs::read(dir.join("capture.json")).unwrap(),authority);
    for name in ["screen.mp4","cursor/raw.ndjson","capture.json"] {assert_eq!(before[name],hash(&dir.join(name)));}
    before
}
#[test]
fn generated_mp4_probe_conversion_publication_and_import_preserve_original_bytes() {
    let f=Fixture::new();let evidence=generated_video(&f.0,source().bounds);let mut c=clock();c.saved_endpoint=evidence.endpoint;
    let mut hidden=observed(0,0.5);if let RawState::Observed{visible,..}=&mut hidden.state {*visible=false;}
    let o=f.raw(&[hidden]);let result=finalize(&f.0,source(),o,Some(&c)).unwrap();let FinalizedCursor::Separate{degraded}=result else {panic!("expected separate")};
    // This fixture constructs the clock and initial project; it does not prove encoder-clock admission.
    let candidate=manifest_for(&f.0,source(),&c,&evidence,degraded);let mut start=candidate.clone();start.state=focusflow_core::capture::CaptureStateV1::Incomplete{video_path:focusflow_core::capture::VideoPath::Screen,video_cursor:focusflow_core::capture::SeparateCursor::Separate,raw_samples_path:focusflow_core::capture::RawSamplesPath::Raw};
    fs::remove_file(f.0.join("capture.json")).unwrap();focusflow_core::capture::publish(&f.0,&start).unwrap();
    let hashes=publish_import(&f.0,&candidate,&evidence);eprintln!("Generated immutable source/authority hashes: {hashes}");
}
#[test]
#[ignore = "requires root-owned authenticated DISPLAY=:98 and existing generated-only FOCUSFLOW_NATIVE_EVIDENCE"]
fn generated_native_finalizer_fixture() {
    use x11rb::protocol::xproto::*;
    assert_eq!(std::env::var("DISPLAY").as_deref(),Ok(":98"));assert_eq!(std::env::var("FOCUSFLOW_ISOLATED_X11").as_deref(),Ok("1"));
    let evidence_dir=PathBuf::from(std::env::var_os("FOCUSFLOW_NATIVE_EVIDENCE").expect("owned evidence directory"));
    assert!(evidence_dir.is_dir());assert_eq!(evidence_dir.canonicalize().unwrap(),evidence_dir);
    let session=evidence_dir.join("finalizer-session");fs::create_dir(&session).unwrap();
    let (connection,screen)=x11rb::connect(None).unwrap();let root=connection.setup().roots[screen].root;
    let window=connection.generate_id().unwrap();connection.create_window(x11rb::COPY_DEPTH_FROM_PARENT,window,root,120,100,100,50,0,WindowClass::INPUT_OUTPUT,0,&CreateWindowAux::new().background_pixel(0x446688)).unwrap().check().unwrap();
    connection.map_window(window).unwrap().check().unwrap();connection.warp_pointer(0u32,window,0,0,0,0,10,10).unwrap().check().unwrap();connection.flush().unwrap();
    let s=Source{bounds:CaptureBounds{x:120,y:100,width:100,height:50},window:Some(window)};
    let mut sampler=Sampler::prepare(&session,s).unwrap();let epoch=Instant::now();sampler.start(epoch).unwrap();thread::sleep(Duration::from_millis(150));
    connection.warp_pointer(0u32,window,0,0,0,0,40,20).unwrap().check().unwrap();connection.flush().unwrap();thread::sleep(Duration::from_millis(100));
    connection.warp_pointer(0u32,window,0,0,0,0,70,30).unwrap().check().unwrap();connection.flush().unwrap();thread::sleep(Duration::from_millis(100));
    let outcome=sampler.finish();connection.destroy_window(window).unwrap().check().unwrap();connection.flush().unwrap();
    assert!(outcome.trustworthy_prefix);assert!(outcome.records>5);assert!(!outcome.shapes.is_empty());
    fs::write(evidence_dir.join("finalizer-sampler-outcome.json"),serde_json::to_vec_pretty(&outcome).unwrap()).unwrap();let raw=fs::read(&outcome.raw_path).unwrap();
    let saved=generated_video(&session,s.bounds);
    // Clearly synthetic frame-zero bridge for sampler-only conversion, not completed encoder-clock proof.
    let c=AcceptedClock{frame_zero_ns:100_000_000,anchor_low_ns:99_999_999,anchor_high_exclusive_ns:100_000_002,completed_frames:30,saved_endpoint:saved.endpoint};
    let result=finalize(&session,s,outcome,Some(&c)).unwrap();let FinalizedCursor::Separate{degraded}=result else {panic!("expected native separate assets")};
    let mut evaluation=doc(s.bounds,1_000_000);evaluation.cursor_settings.style=focusflow_core::project::CursorStyle::Native;
    let prepared=cursor::prepare_cursor(&session,&evaluation,&AtomicBool::new(false)).unwrap().unwrap();
    assert!((prepared.sample_at(0,1).unwrap().x-0.1).abs()<0.001);
    let mut evaluated=vec![];
    for fps in [30,60] {for frame in 0..=30 {let value=prepared.sample_at(frame*1_000_000,fps);evaluated.push(serde_json::json!({"fps":fps,"frame":frame,"visible":value.is_some(),"x":value.map(|v|v.x),"y":value.map(|v|v.y)}));}}
    assert!((prepared.sample_at(100_000,1).unwrap().x-0.4).abs()<0.001);assert!((prepared.sample_at(200_000,1).unwrap().x-0.7).abs()<0.001);assert!(prepared.sample_at(500_000,1).is_none());
    assert_eq!(fs::read(session.join("cursor/raw.ndjson")).unwrap(),raw);
    let candidate=manifest_for(&session,s,&c,&saved,degraded);let hashes=publish_import(&session,&candidate,&saved);
    fs::write(evidence_dir.join("finalizer-evidence.json"),serde_json::to_vec_pretty(&serde_json::json!({"clock":"synthetic 100ms frame zero; no completed-encoder admission proof","microphone":"off; no audio capture","video":"generated black 100x50 30fps MP4","hashes":hashes,"evaluated":evaluated})).unwrap()).unwrap();
}
#[test]
fn same_microsecond_budget_uses_merged_discontinuity_bytes() {
    let f=Fixture::new();let o=f.raw(&[observed(1000,0.1),observed(1999,0.2)]);
    let merged=FinalRow{at_us:1,x:0.2,y:0.5,visible:true,shape_id:None,discontinuity:true};let mut limits=Limits::default();limits.raw_bytes=3*MAX_RECORD_BYTES as u64+merged.bytes().unwrap().len() as u64;
    finalize_with(&f.0,source(),o,Some(&clock()),Limits::default(),limits,|_,_|Ok(())).unwrap();
    let rows=f.rows();assert_eq!(rows.len(),1);assert_eq!(rows[0]["x"],0.2);assert_eq!(rows[0]["visible"],true);assert_eq!(rows[0]["discontinuity"],true);
}
#[test]
fn reply_completed_after_cutoff_cannot_refresh_even_when_midpoint_precedes_it() {
    let mut late=observed(130_000_000,0.75);late.after_ns+=20_000_000;
    let (f,_)=run(&[observed(-100_000_000,0.25),late,observed(160_000_000,0.9)],Some(145_000_000));
    let rows=f.rows();assert_eq!(rows.len(),2);assert_eq!(rows[0]["x"],0.25);assert_eq!(rows[1]["atUs"],145_000);assert_eq!(rows[1]["visible"],false);
    assert_eq!(consumer(&f).sample_at(144_000,1).unwrap().x,0.25);
}
