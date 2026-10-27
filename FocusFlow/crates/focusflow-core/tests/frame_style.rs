mod common;
use common::{fixture_project, TestDir};
use focusflow_core::{project::*, edits::Edit, store::ProjectStore, ErrorCode};
use std::fs;
fn setup() -> (TestDir, ProjectStore) {
    let temp=TestDir::new(); fs::create_dir(temp.path().join("fixture")).unwrap();
    let mut value=serde_json::to_value(fixture_project()).unwrap(); value.as_object_mut().unwrap().remove("frameStyle");
    fs::write(temp.path().join("fixture/project.json"),serde_json::to_vec(&value).unwrap()).unwrap();
    let store=ProjectStore::new(temp.path()).unwrap(); (temp,store)
}
#[test]
fn schema_one_reads_unchanged_and_successful_mutation_upgrades() {
    let (temp,store)=setup(); let path=temp.path().join("fixture/project.json"); let bytes=fs::read(&path).unwrap();
    let old=store.get("fixture").unwrap(); assert_eq!(old.frame_style,FrameStyle::default()); assert_eq!(old.revision,0);
    assert_eq!(fs::read(&path).unwrap(),bytes);
    let updated=store.apply_edits("fixture",0,&[Edit::TrimClip{clip_id:"clip-0".into(),in_us:1,out_us:4_000_000}]).unwrap();
    assert_eq!(updated.schema_version,4); assert_eq!(updated.revision,1);
}
#[test]
fn style_edit_is_persisted_and_invalid_style_is_atomic() {
    let (temp,store)=setup(); let style=FrameStyle{padding_px:10,..Default::default()};
    let doc=store.apply_edits("fixture",0,&[Edit::SetFrameStyle{style:style.clone()}]).unwrap();
    assert_eq!(doc.frame_style,style);
    let before=fs::read(temp.path().join("fixture/project.json")).unwrap();
    let invalid=FrameStyle{padding_px:257,..Default::default()};
    assert_eq!(store.apply_edits("fixture",1,&[Edit::SetFrameStyle{style:invalid}]).unwrap_err().code,ErrorCode::InvalidDocument);
    assert_eq!(fs::read(temp.path().join("fixture/project.json")).unwrap(),before);
}
#[test]
fn restore_recovers_destructive_edits_without_revision_rollback() {
    let (_temp,store)=setup(); let old=store.get("fixture").unwrap(); let state=EditableState::from_document(&old);
    let edited=store.apply_edits("fixture",0,&[Edit::SplitClip{clip_id:"clip-0".into(),at_us:2_000_000,right_clip_id:"right".into()},Edit::RemoveClip{clip_id:"clip-0".into()},Edit::TrimClip{clip_id:"right".into(),in_us:3_000_000,out_us:4_000_000}]).unwrap();
    let restored=store.restore_edits("fixture",edited.revision,&state).unwrap();
    assert_eq!(restored.clips,old.clips); assert_eq!(restored.revision,2);
    assert_eq!(restored.video,old.video); assert_eq!(restored.timing,old.timing); assert_eq!(restored.warnings,old.warnings);
    assert_eq!(store.restore_edits("fixture",1,&state).unwrap_err().code,ErrorCode::RevisionConflict);
}
#[test]
fn listing_pages_are_stable_and_report_malformed_entries() {
    let (temp,store)=setup(); fs::create_dir(temp.path().join("broken")).unwrap(); fs::write(temp.path().join("broken/project.json"),b"broken").unwrap();
    let first=store.list_projects(None,1).unwrap(); assert_eq!(first.issues.len(),1); assert_eq!(first.issues[0].entry,"broken");
    assert_eq!(first.next_cursor.as_deref(),Some("broken"));
    let second=store.list_projects(first.next_cursor.as_deref(),1).unwrap(); assert_eq!(second.projects.len(),1); assert_eq!(second.projects[0].project_id,"fixture");
    assert!(second.next_cursor.is_none()); assert!(second.projects[0].capabilities.fixed_zoom); assert!(!second.projects[0].capabilities.cursor_follow);
    for limit in [0,101] {assert!(store.list_projects(None,limit).is_err());}
    assert!(store.list_projects(Some("../outside"),1).is_err());
}

#[test]
fn frame_validation_rejects_invalid_colors_ranges_and_empty_geometry() {
    let mut doc=fixture_project(); doc.schema_version=2;
    for value in ["white","#fff","#000000;evil","#é0000","#12gg34"] {
        doc.frame_style.background=FrameBackground::Solid{color:value.into()}; assert!(validate(&doc).is_err(),"{value}");
    }
    doc.frame_style=FrameStyle{corner_radius_px:129,..Default::default()}; assert!(validate(&doc).is_err());
    doc.frame_style=FrameStyle{padding_px:25,..Default::default()}; assert!(validate(&doc).is_err());
    doc.frame_style=FrameStyle{padding_px:1,..Default::default()}; assert!(validate(&doc).is_ok());
    doc.schema_version=1; assert!(validate(&doc).is_err());
}
#[test]
fn schema_two_and_nested_style_are_strict_and_future_bytes_survive() {
    let (temp,store)=setup(); let path=temp.path().join("fixture/project.json");
    let mut doc=fixture_project(); doc.schema_version=2;
    let valid=serde_json::to_string(&doc).unwrap();
    let cases=[valid.replacen("{","{\"schemaVersion\":2,",1),
        valid.replacen("\"paddingPx\":0","\"paddingPx\":0,\"paddingPx\":0",1),
        valid.replacen("\"shadow\":false","\"shadow\":false,\"unknown\":0",1),
        valid.replacen("\"schemaVersion\":2","\"schemaVersion\":4",1)];
    for bytes in cases { fs::write(&path,&bytes).unwrap(); assert!(store.get("fixture").is_err()); assert_eq!(fs::read_to_string(&path).unwrap(),bytes); }
    fs::write(&path,&valid).unwrap(); assert_eq!(store.get("fixture").unwrap(),doc);
}
#[test]
fn editable_state_rejects_metadata_and_restore_preserves_current_noneditable_fields() {
    let (temp,store)=setup(); let mut current=store.get("fixture").unwrap();
    let mut state=EditableState::from_document(&current);
    let mut value=serde_json::to_value(&state).unwrap(); value["revision"]=serde_json::json!(0);
    assert!(serde_json::from_value::<EditableState>(value).is_err());
    current.revision=5; current.camera.settle_us=999; current.clicks=vec![Click{at_us:1,x:0.2,y:0.8}];
    current.microphone=Some(AudioAsset{path:"mic.wav".into()}); current.warnings=vec![ProjectWarning::EmbeddedCursor];
    fs::write(temp.path().join("fixture/project.json"),serde_json::to_vec(&current).unwrap()).unwrap();
    state.frame_style.padding_px=2;
    let restored=store.restore_edits("fixture",5,&state).unwrap();
    assert_eq!(restored.revision,6); assert_eq!(restored.frame_style,state.frame_style);
    assert_eq!(restored.camera,current.camera); assert_eq!(restored.clicks,current.clicks);
    assert_eq!(restored.microphone,current.microphone); assert_eq!(restored.warnings,current.warnings);
    let before=fs::read(temp.path().join("fixture/project.json")).unwrap();
    state.clips.clear(); assert!(store.restore_edits("fixture",6,&state).is_err());
    assert_eq!(fs::read(temp.path().join("fixture/project.json")).unwrap(),before);
}
#[test]
fn restore_recovers_zoom_removed_by_trim() {
    let (temp,store)=setup(); let mut doc=store.get("fixture").unwrap();
    doc.zooms=vec![ZoomElement{mode:ZoomMode::Fixed,id:"zoom".into(),clip_id:"clip-0".into(),start_us:1_000_000,end_us:2_000_000,x:0.3,y:0.7,scale:2.0,intro_us:100_000,hold_us:100_000,outro_us:100_000}];
    fs::write(temp.path().join("fixture/project.json"),serde_json::to_vec(&doc).unwrap()).unwrap();
    let state=EditableState::from_document(&doc);
    assert!(store.apply_edits("fixture",0,&[Edit::TrimClip{clip_id:"clip-0".into(),in_us:3_000_000,out_us:4_000_000}]).unwrap().zooms.is_empty());
    assert_eq!(store.restore_edits("fixture",1,&state).unwrap().zooms,state.zooms);
}
#[test]
fn listing_does_not_import_and_reports_busy_entries() {
    let (temp,store)=setup(); let legacy=temp.path().join("legacy"); fs::create_dir(&legacy).unwrap(); fs::write(legacy.join("screen.mp4"),b"placeholder").unwrap();
    fs::write(temp.path().join("unrelated"),b"unrelated root file").unwrap();
    fs::create_dir(temp.path().join("not an id")).unwrap();
    store.get("fixture").unwrap(); let lock=fs::OpenOptions::new().write(true).open(temp.path().join(".locks/fixture.document")).unwrap(); lock.try_lock().unwrap();
    let page=store.list_projects(None,100).unwrap(); assert_eq!(page.issues.len(),1); assert_eq!(page.issues[0].error.code,ErrorCode::Busy);
    assert_eq!(page.projects.len(),1); let summary=&page.projects[0]; assert!(summary.needs_import); assert_eq!(summary.name,"legacy"); assert!(summary.revision.is_none() && summary.duration_us.is_none());
    assert!(!legacy.join("project.json").exists()); assert!(page.next_cursor.is_none());
}
#[test]
#[cfg(unix)]
fn listing_rejects_aliases_and_continues_to_healthy_projects() {
    let (temp,store)=setup(); std::os::unix::fs::symlink(temp.path().join("fixture"),temp.path().join("alias")).unwrap();
    let first=store.list_projects(None,1).unwrap(); assert_eq!(first.issues[0].error.code,ErrorCode::InvalidPath); assert_eq!(first.next_cursor.as_deref(),Some("alias"));
    assert_eq!(store.list_projects(first.next_cursor.as_deref(),1).unwrap().projects[0].project_id,"fixture");
    assert!(!temp.path().join(".locks/alias.document").exists());
}
#[test]
fn listing_scan_ceiling_counts_skipped_entries() {
    let (temp,store)=setup(); for n in 0..9999 { fs::write(temp.path().join(format!("unrelated.{n}")),b"").unwrap(); }
    assert_eq!(store.list_projects(None,1).unwrap_err().code,ErrorCode::InvalidDocument);
}
#[test]
fn geometry_presets_bound_content_and_preserve_identity() {
    use focusflow_core::frame::geometry;
    let g=geometry(&FrameStyle::default(),161,97).unwrap(); assert_eq!((g.width,g.height,g.content_width,g.content_height,g.content_x,g.content_y),(160,96,160,96,0,0));
    for canvas in [CanvasFormat::Source,CanvasFormat::Landscape,CanvasFormat::Portrait,CanvasFormat::Square] {
        let g=geometry(&FrameStyle{canvas,padding_px:20,corner_radius_px:128,..Default::default()},1920,1080).unwrap();
        assert!(g.content_x>=20 && g.content_y>=20); assert!(g.content_width%2==0 && g.content_height%2==0);
        assert!((g.content_width as f64/g.content_height as f64-16.0/9.0).abs()<0.01);
    }
}

#[test]
fn schema_two_requires_explicit_style_and_issues_are_bounded() {
    let (temp,store)=setup(); let path=temp.path().join("fixture/project.json");
    let bytes=fs::read_to_string(&path).unwrap().replace("\"schemaVersion\":1","\"schemaVersion\":2");
    fs::write(&path,&bytes).unwrap(); assert_eq!(store.get("fixture").unwrap_err().field.as_deref(),Some("frameStyle"));
    assert_eq!(fs::read_to_string(&path).unwrap(),bytes);
    let mut value=serde_json::to_value(fixture_project()).unwrap(); value["x".repeat(20_000)]=serde_json::json!(0);
    fs::write(&path,serde_json::to_vec(&value).unwrap()).unwrap();
    let page=store.list_projects(None,100).unwrap(); assert_eq!(page.issues.len(),1); assert!(page.issues[0].error.message.len()<=1024);
}

#[test]
fn invalid_style_cannot_be_hidden_by_later_reset() {
    let (temp,store)=setup(); let path=temp.path().join("fixture/project.json"); let bytes=fs::read(&path).unwrap();
    assert!(store.apply_edits("fixture",0,&[Edit::SetFrameStyle{style:FrameStyle{padding_px:257,..Default::default()}},Edit::SetFrameStyle{style:Default::default()}]).is_err());
    assert_eq!(fs::read(&path).unwrap(),bytes);
    let json=serde_json::json!({"type":"setFrameStyle","style":{"canvas":"square","background":{"type":"gradient","start":"#112233","end":"#445566"},"paddingPx":10,"cornerRadiusPx":20,"shadow":true}});
    let edit:Edit=serde_json::from_value(json.clone()).unwrap(); assert_eq!(serde_json::to_value(edit).unwrap(),json);
}

#[test]
fn full_issue_page_advances_to_later_healthy_project() {
    let (temp,store)=setup();
    for i in 0..100 { let path=temp.path().join(format!("broken-{i:03}")); fs::create_dir(&path).unwrap(); fs::write(path.join("project.json"),b"bad").unwrap(); }
    let first=store.list_projects(None,100).unwrap(); assert_eq!(first.issues.len(),100); assert!(first.projects.is_empty());
    assert_eq!(first.next_cursor.as_deref(),Some("broken-099"));
    let second=store.list_projects(first.next_cursor.as_deref(),100).unwrap(); assert_eq!(second.projects.len(),1); assert!(second.next_cursor.is_none());
    assert_eq!(second.projects[0].duration_us,Some(5_000_000)); assert_eq!(second.projects[0].revision,Some(0));
}
