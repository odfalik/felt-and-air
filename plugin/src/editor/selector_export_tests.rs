//! Fixed six-case real-selector export; opt-in only, no engine or host-value emulation.
use super::*;
use super::super::action_export_tests::{Capture,SelectorMetadata};
use std::os::unix::fs::{MetadataExt,OpenOptionsExt};
use std::os::fd::{AsRawFd,FromRawFd};

const CASES:[(&str,&str,&str);6]=[("preset-closed","preset","closed"),("demo-closed","demo","closed"),
    ("preset-hover","preset","hover"),("demo-hover","demo","hover"),
    ("preset-popup","preset","popup"),("demo-popup","demo","popup")];
const DRAW_CAP:u64=160;
const BATCH_CAP:usize=512*1024*1024-12*1024*1024-128*1024;
#[cfg(target_os="macos")]
const NOFOLLOW:i32=0x100;
#[cfg(target_os="linux")]
const NOFOLLOW:i32=0x20000;
#[cfg(target_os="macos")]
const CREATE:i32=0x1|0x200|0x800|NOFOLLOW;
#[cfg(target_os="linux")]
const CREATE:i32=0x1|0x40|0x80|NOFOLLOW;
extern "C" {fn geteuid()->u32;fn openat(fd:core::ffi::c_int,name:*const core::ffi::c_char,flags:core::ffi::c_int,...)->core::ffi::c_int;}
fn identity(m:&std::fs::Metadata)->(u64,u64,u32,u32,u64,u64,i64,i64,i64,i64) {
    (m.dev(),m.ino(),m.uid(),m.mode(),m.nlink(),m.len(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec())
}
fn directory(path:&std::path::Path,held:&std::fs::File,uid:u32) {
    let a=std::fs::symlink_metadata(path).expect("owned directory metadata");let b=held.metadata().unwrap();
    assert!(a.is_dir() && !a.file_type().is_symlink() && a.uid()==uid && a.mode()&0o7777==0o700,"owned0700directory");
    assert_eq!(std::fs::canonicalize(path).unwrap(),path,"canonical owned directory");
    assert_eq!((a.dev(),a.ino(),a.uid(),a.mode()),(b.dev(),b.ino(),b.uid(),b.mode()),"held directory/path binding");
}
fn budget(f:&Fixture,draws:u64) {assert!(f.frame.checked_add(draws).unwrap()<=DRAW_CAP,"hard actual selector UI draw cap");}
fn draw(f:&mut Fixture,events:Vec<Event>)->FullOutput {budget(f,1);f.frame(events)}
fn row(f:&mut Fixture,kind:&str,names:&[String;2],wheels:&mut usize)->(Id,Rect,Rect) {
    let key=item_key(kind,1,names);
    loop {
        let target=selector_trace(f,&key).expect("fresh actual catalog row");
        let (content,clip)=popup_trace(f,kind);let visible=content.intersect(clip).intersect(f.screen());
        assert!(visible.width()>0.0 && visible.height()>0.0,"actual popup viewport");
        let shown=target.1.intersect(target.2).intersect(visible);
        if shown.width()>8.0 && shown.height()>8.0 && target.1.top()>=visible.top()-TEXT_TOLERANCE &&
            target.1.bottom()<=visible.bottom()+TEXT_TOLERANCE {return target;}
        assert!(*wheels<6,"fixed six genuine wheel actions TOTAL per case");
        let direction=if target.1.center().y<visible.center().y {-1.0} else {1.0};
        budget(f,13);f.wheel(visible.center(),MouseWheelUnit::Point,128.0*direction);*wheels+=1;
    }
}
fn rect(r:Rect)->[f32;4] {[r.min.x,r.min.y,r.max.x,r.max.y]}
fn id_hex(id:Id)->String {format!("{:016x}",id.value())}
fn case(key:&str,kind:&str,surface:&str)->(Fixture,SelectorMetadata) {
    let (mut f,names)=selector_catalog_unsettled(vec2(800.0,600.0));
    assert_eq!(f.frame,0,"Capture BEFORE first Context.run");f.replay=Some(Capture::default());
    budget(&f,3);f.settle();let mut wheels=0;
    budget(&f,5);let combo=open_selector(&mut f,kind);
    let target=row(&mut f,kind,&names,&mut wheels);
    assert_eq!(f.ctx.read_response(target.0).unwrap().rect,target.1,"actual row response identity");
    budget(&f,2);f.click_at(target.1.intersect(target.2).intersect(f.screen()).center());budget(&f,3);f.settle();
    assert!(!ComboBox::is_open(&f.ctx,combo),"actual choice closes actual popup");
    if kind=="preset" {assert_eq!(f.state.preset,names[1]);}
    else {assert_eq!(f.state.demo,1);assert_eq!(f.state.demos[1].title,names[1]);}
    let mut output=draw(&mut f,Vec::new());
    let mut target_row=None;
    if surface=="hover" {
        let (_,closed,clip)=selector_trace(&f,kind).unwrap();
        let point=closed.intersect(clip).intersect(f.screen()).center();
        draw(&mut f,vec![Event::PointerMoved(point)]);
        for _ in 0..48 {output=draw(&mut f,Vec::new());}
    } else if surface=="popup" {
        budget(&f,5);let opened=open_selector(&mut f,kind);assert_eq!(opened,combo);
        row(&mut f,kind,&names,&mut wheels);output=draw(&mut f,Vec::new());
        target_row=Some(selector_trace(&f,&item_key(kind,1,&names)).expect("same LAST-frame row metadata"));
        assert!(ComboBox::is_open(&f.ctx,combo),"actual target popup stays open");
    }
    let (response_id,closed,closed_clip)=selector_trace(&f,kind).unwrap();assert_eq!(response_id,combo);
    let texts=painted_text(&output);
    let closed_names:Vec<_>=texts.iter().filter(|(t,_)|t.galley.job.text==names[1] && t.visual_bounding_rect().intersects(closed)).collect();
    assert!(closed_names.len()==1,"one actual selected-name galley");
    let (closed_text,shape_clip)=closed_names[0];let available=closed.intersect(closed_clip).intersect(*shape_clip).intersect(f.screen());
    assert!(closed_text.galley.elided && closed_text.galley.rows.len()==1 && closed_text.angle==0.0,"actual closed elision");
    assert!(inside(closed_text.visual_bounding_rect(),available) && inside(closed_text.galley.rect.translate(closed_text.pos.to_vec2()),available) &&
        closed_text.galley.rows.iter().all(|r|inside(r.rect.translate(closed_text.pos.to_vec2()),available)),"actual elided glyph bounds");
    let expected=if surface=="popup" && kind=="preset" {format!("Saved · {}",names[1])} else {names[1].clone()};
    let candidates:Vec<_>=texts.iter().filter(|(t,clip)| {
        if t.galley.job.text!=expected {return false;}
        match surface {
            "closed"=>t.visual_bounding_rect().intersects(closed),
            "hover"=>!t.galley.elided && !t.visual_bounding_rect().intersects(closed),
            "popup"=>t.visual_bounding_rect().intersects(target_row.unwrap().1.intersect(*clip)),
            _=>false,
        }
    }).cloned().collect();
    assert!(candidates.len()==1,"one exact actual target galley");
    let (galley,galley_clip)=&candidates[0];
    if surface!="closed" {
        let panel=target_row.map_or(f.screen(),|(_,r,_)|r);
        let visible=target_row.map_or(f.screen(),|(_,_,c)|c.intersect(f.screen()));
        assert!(seen_text(&candidates,&expected,panel,visible),"actual COMPLETE target glyphs/clip");
    }
    assert!(names.iter().all(|n|f.state.setups.contains_key(n)),"full stored setup keys retained");
    assert_eq!(f.state.shared.demo_playing.load(Ordering::Relaxed),-1,"no engine playback");
    assert!(f.musical_inputs().is_empty(),"no musical input from catalog cases");
    let metadata=SelectorMetadata {case:key.into(),selector:kind.into(),surface:surface.into(),target_index:f.frame-1,
        setup_keys:f.state.setups.keys().cloned().collect(),demo_titles:f.state.demos.iter().map(|d|d.title.clone()).collect(),
        selected_setup:f.state.preset.clone(),selected_demo:f.state.demo,selected_demo_title:f.state.demos[f.state.demo].title.clone(),
        full_label:expected,response_id_hex:id_hex(response_id),response_rect:rect(closed),response_clip:rect(closed_clip),
        row_id_hex:target_row.map(|(id,_,_)|id_hex(id)),row_rect:target_row.map(|(_,r,_)|rect(r)),row_clip:target_row.map(|(_,_,c)|rect(c)),
        galley_text:galley.galley.job.text.clone(),galley_rect:rect(galley.galley.rect.translate(galley.pos.to_vec2())),galley_clip:rect(*galley_clip),
        selected_elided:closed_text.galley.elided,popup_open:ComboBox::is_open(&f.ctx,combo),full_glyphs:surface!="closed"};
    assert!(f.frame<=160 && wheels<=6,"fixed actual selector script budget");
    (f,metadata)
}
#[test]
#[ignore="explicit SOURCE-reviewed six-selector800 typed export only; no runtime authority"]
fn export_six_actual_selector_states() {
    let path=std::env::var_os("FELT_SELECTOR_REPLAY_OUTPUT_DIR").expect("explicit NEW owned selector output directory");
    let path=std::path::PathBuf::from(path);assert!(path.is_absolute());let uid=unsafe {geteuid()};
    let held=std::fs::OpenOptions::new().read(true).custom_flags(NOFOLLOW).open(&path).expect("held nofollow output directory");
    directory(&path,&held,uid);assert!(std::fs::read_dir(&path).unwrap().next().is_none(),"fresh empty directory");
    let revision=std::env::var("FELT_SELECTOR_REPLAY_REV").expect("explicit staged selector revision");
    let _mode=EditorModeGuard::new(true);let mut total=0usize;let mut records=Vec::new();
    for (key,kind,surface) in CASES {
        let (f,metadata)=case(key,kind,surface);directory(&path,&held,uid);
        let leaf=format!("selector-{key}-800x600.json");let name=std::ffi::CString::new(leaf.as_bytes()).unwrap();
        let fd=unsafe {openat(held.as_raw_fd(),name.as_ptr(),CREATE,0o600 as core::ffi::c_int)};assert!(fd>=0,"exclusive owned nofollow leaf");
        let file=unsafe {std::fs::File::from_raw_fd(fd)};let before=file.metadata().unwrap();
        assert!(before.is_file() && before.uid()==uid && before.nlink()==1 && before.mode()&0o7777==0o600,"owned single-link regular leaf");
        let mut writer=std::io::BufWriter::new(file);
        let summary=f.replay.as_ref().unwrap().write_selector_sequence(&revision,[f.size.x,f.size.y],&metadata,&mut writer);
        std::io::Write::flush(&mut writer).unwrap();writer.get_ref().sync_all().unwrap();let after=writer.get_ref().metadata().unwrap();
        assert_eq!((before.dev(),before.ino(),before.uid(),before.mode(),before.nlink()),(after.dev(),after.ino(),after.uid(),after.mode(),after.nlink()),"held leaf identity");
        assert_eq!(after.len(),summary.bytes as u64);assert_eq!(summary.target_index,f.frame-1);
        assert_eq!(summary.target_pts.to_bits(),((f.frame-1) as f64/60.0).to_bits());assert_eq!(summary.frames as u64,f.frame);
        total=total.checked_add(summary.bytes).unwrap();assert!(total<=BATCH_CAP,"six JSON TOTAL including PNG/source reserves");
        directory(&path,&held,uid);assert_eq!(identity(&after),identity(&std::fs::symlink_metadata(path.join(&leaf)).unwrap()),"held leaf to nofollow path");
        records.push((leaf,identity(&after)));
        eprintln!("SELECTOR_EXPORT case={} frames={} bytes={} rgba={} total={} cap={}",key,summary.frames,summary.bytes,summary.texture_bytes,total,BATCH_CAP);
    }
    directory(&path,&held,uid);
    let actual:std::collections::BTreeSet<_>=std::fs::read_dir(&path).unwrap().map(|e|e.unwrap().file_name()).collect();
    let expected:std::collections::BTreeSet<_>=records.iter().map(|(n,_)|std::ffi::OsString::from(n)).collect();assert!(actual==expected,"exact SIX new leaves");
    for (leaf,expected) in records {
        let name=std::ffi::CString::new(leaf.as_bytes()).unwrap();let fd=unsafe {openat(held.as_raw_fd(),name.as_ptr(),NOFOLLOW)};
        assert!(fd>=0,"postflight nofollow leaf");let file=unsafe {std::fs::File::from_raw_fd(fd)};
        assert_eq!(identity(&file.metadata().unwrap()),expected,"final ALL held leaf identities");
        assert_eq!(identity(&std::fs::symlink_metadata(path.join(leaf)).unwrap()),expected,"final ALL pathname bindings");
        directory(&path,&held,uid);
    }
}
