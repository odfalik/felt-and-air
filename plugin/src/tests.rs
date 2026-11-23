//! Integration regressions against the actual plugin state machine, background executor,
//! and mechanical engine. No audio device, GUI, or mocked engine/context is involved.
use super::*;
use shared::FROM_UI;

fn instrument() -> PhysicalPiano {
    instrument_with_demos(Arc::new(Vec::new()))
}
fn instrument_with_demos(demos:Arc<Vec<demo::Demo>>) -> PhysicalPiano {
    let mut p = PhysicalPiano::with_demos(||demos);
    assert!(p.initialize_engine(48_000.0, 512));
    p
}
fn on(p: &mut PhysicalPiano, midi: i32) {
    p.input(Input::NoteOn {
        midi,
        velocity: 80,
        from: FROM_HOST,
    });
}
fn off(p: &mut PhysicalPiano, midi: i32) {
    p.input(Input::NoteOff {
        midi,
        from: FROM_HOST,
        release: 1.0,
    });
}
fn task(p: &mut PhysicalPiano) -> Task {
    p.generation += 1;
    Task::Rebuild(
        p.params.options(),
        p.sample_rate,
        p.params.grid_scale(),
        p.generation,
        p.epoch,
    )
}
fn render(p: &mut PhysicalPiano, seconds: f64) {
    let mut l = [0.0; 512];
    let mut r = [0.0; 512];
    for _ in 0..(seconds * p.sample_rate / 512.0).ceil() as usize {
        p.built.as_mut().unwrap().piano.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    }
}
fn level(p: &PhysicalPiano, midi: i32) -> f32 {
    let piano = &p.built.as_ref().unwrap().piano;
    let mut snap = piano.new_snapshot();
    piano.fill_snapshot(&mut snap, midi);
    snap.string_level
        .iter()
        .enumerate()
        .filter(|(i, _)| piano.string_midi(*i) == midi)
        .map(|(_, v)| *v)
        .fold(0.0, f32::max)
}

#[test]
fn source_cleanup_does_not_release_another_sources_pedals() {
    let mut p = instrument();
    on(&mut p, 60);
    p.input(Input::Sustain(0.72, FROM_HOST));
    p.input(Input::Soft(true, FROM_HOST));
    p.input(Input::Sostenuto(true, FROM_HOST));
    p.input(Input::Sustain(1.0, FROM_UI));
    p.input(Input::Soft(true, FROM_UI));
    p.input(Input::Sostenuto(true, FROM_UI));
    p.input(Input::Sustain(1.0, FROM_DEMO));
    p.clear_source(FROM_UI | shared::FROM_MOUSE); // same operation as editor close
    assert_eq!(p.shared.sustain.load(Ordering::Relaxed), 127); // demo still holds it
    p.input(Input::StopDemo);
    assert_eq!(
        p.shared.sustain.load(Ordering::Relaxed),
        (0.72f32 * 127.0).round() as u8
    );
    assert!(p.shared.soft.load(Ordering::Relaxed));
    assert!(p.shared.sostenuto.load(Ordering::Relaxed));
    assert!(p.captured[60]);
    assert_eq!(p.shared.held[60].load(Ordering::Relaxed), FROM_HOST);
    p.input(Input::Sustain(0.0, FROM_HOST));
    p.input(Input::Soft(false, FROM_HOST));
    p.input(Input::Sostenuto(false, FROM_HOST));
    assert_eq!(p.shared.sustain.load(Ordering::Relaxed), 0);
    assert!(!p.shared.soft.load(Ordering::Relaxed));
    assert!(!p.shared.sostenuto.load(Ordering::Relaxed));
    println!("source cleanup: host half-pedal, soft, sostenuto and held C survived editor close/demo stop; host release clears them");
}

#[test]
fn reactivation_rejects_ready_and_late_old_rate_builds_without_publishing_geometry() {
    let mut p = instrument();
    let worker = p.task_executor();
    let pending48 = task(&mut p);
    let old_epoch = p.epoch;
    let old_display = p.shared.display.lock().unwrap().generation;
    worker(task(&mut p)); // 48 kHz build ready, but not yet accepted
    assert_eq!(p.fresh.len(), 1);
    assert_eq!(p.shared.display.lock().unwrap().generation, old_display);
    assert!(p.initialize_engine(96_000.0, 512)); // drains ready old-rate result
    let generation96 = p.shared.display.lock().unwrap().generation;
    assert!(p.epoch > old_epoch);
    assert!(p.fresh.is_empty());
    p.shared.building.store(true, Ordering::Release); // a new lifecycle's build flag
    worker(pending48); // old task completes after reactivation
    assert!(p.fresh.is_empty());
    assert!(p.shared.building.load(Ordering::Acquire)); // old task cannot clear this
    assert_eq!(p.shared.display.lock().unwrap().generation, generation96);
    assert_eq!(p.built.as_ref().unwrap().sample_rate, 96_000.0);
    assert_eq!(p.built.as_ref().unwrap().epoch, p.epoch);
    worker(task(&mut p)); // current 96 kHz result, still must not publish prematurely
    assert_eq!(p.shared.display.lock().unwrap().generation, generation96);
    let shared = p.shared.clone();
    let copy_in_progress = shared.display.lock().unwrap();
    assert!(!p.accept_fresh()); // bounded skip, no waiting on a UI snapshot copy
    drop(copy_in_progress);
    assert!(p.accept_fresh());
    assert_eq!(
        p.shared.display.lock().unwrap().generation,
        p.built.as_ref().unwrap().generation
    );
    assert_eq!(p.built.as_ref().unwrap().sample_rate, 96_000.0);
    worker(Task::Collect);
    println!("reactivation: queued + late 48 kHz builds rejected at 96 kHz; display stayed current until accepted; UI contention skips swap");
}

#[test]
fn unrelated_pedals_and_source_cleanup_do_not_recapture_sostenuto() {
    let mut p = instrument();
    on(&mut p, 60);
    p.input(Input::Sostenuto(true, FROM_HOST));
    off(&mut p, 60); // captured but released C must survive all unrelated updates
    on(&mut p, 62); // later held D must never be added to the capture
    p.input(Input::Sustain(0.8, FROM_HOST));
    p.input(Input::Soft(true, FROM_HOST));
    p.input(Input::Sustain(0.0, FROM_HOST));
    p.input(Input::Soft(false, FROM_HOST));
    p.clear_source(FROM_UI | FROM_DEMO);
    off(&mut p, 62);
    render(&mut p, 0.8);
    let c = level(&p, 60);
    let d = level(&p, 62);
    println!("unrelated pedal/source updates: captured released C={c:.3e}, later D={d:.3e} m");
    assert!(c > 1e-6 && d < c * 0.1);
    p.input(Input::Sostenuto(false, FROM_HOST));
    render(&mut p, 0.8);
    assert!(level(&p, 60) < c * 0.1);
}

#[test]
fn sostenuto_rebuild_preserves_capture_membership_and_released_captures() {
    for release_c_before in [false, true] {
        let mut p = instrument();
        on(&mut p, 60); // C held when CC66 goes down
        p.input(Input::Sostenuto(true, FROM_HOST));
        on(&mut p, 62); // D played after pedal-down: must not be captured
        if release_c_before {
            off(&mut p, 60);
        }
        let worker = p.task_executor();
        worker(task(&mut p));
        assert!(p.accept_fresh());
        worker(Task::Collect);
        assert!(p.captured[60] && !p.captured[62]);
        off(&mut p, 62);
        if !release_c_before {
            off(&mut p, 60);
        }
        render(&mut p, 0.8);
        let c = level(&p, 60);
        let d = level(&p, 62);
        println!("sostenuto rebuild, C released before={release_c_before}: C={c:.3e} m, later D={d:.3e} m");
        assert!(
            c > 1e-6 && d < c * 0.1,
            "captured C should ring while later D damps"
        );
        p.input(Input::Sostenuto(false, FROM_HOST));
        render(&mut p, 0.8);
        let released = level(&p, 60);
        println!("after CC66-up: C={released:.3e} m");
        assert!(released < c * 0.1, "pedal-up must release the retained C");
    }
}


// The production segment method is tested with real Pianos and typed event input,
// not a mocked ProcessContext. These are raw engine PCM/public observations,
// NOT whole-physical-state, native-host, final-volume or CLAP-wrapper proof.
const TIMING_BLOCK:usize=256;
const TIMING_SUFFIX:usize=8192;
type HostFixture=(PluginNoteEvent<PhysicalPiano>,Option<Input>);
fn host_on(at:u32,note:u8,velocity:i32)->HostFixture {
    (NoteEvent::NoteOn {timing:at,voice_id:None,channel:0,note,velocity:velocity as f32/127.0},
     Some(Input::NoteOn {midi:note as i32,velocity,from:FROM_HOST}))
}
fn host_off(at:u32,note:u8)->HostFixture {
    (NoteEvent::NoteOff {timing:at,voice_id:None,channel:0,note,velocity:0.5},
     Some(Input::NoteOff {midi:note as i32,from:FROM_HOST,release:1.0}))
}
fn host_cc(at:u32,cc:u8,value:f32,expected:Input)->HostFixture {
    (NoteEvent::MidiCC {timing:at,channel:0,cc,value},Some(expected))
}
fn timing_pair(demos:Arc<Vec<demo::Demo>>)->(PhysicalPiano,PhysicalPiano) {
    let mut a=instrument_with_demos(demos.clone());
    let mut b=instrument_with_demos(demos);
    for p in [&mut a,&mut b] {
        assert_eq!(p.sample_rate,48_000.0);
        assert_eq!(p.params.grid.value(),2);
        assert_eq!(p.params.grid_scale(),1.5);
        let piano=&mut p.built.as_mut().unwrap().piano;
        piano.set_helper_threads(0,None,None);
        let keys:std::collections::BTreeSet<_>=(0..piano.string_count()).map(|i|piano.string_midi(i)).collect();
        assert!(keys==std::collections::BTreeSet::from_iter(21..=108),"full real 88-key instrument");
    }
    (a,b)
}
fn same_raw(a:&PhysicalPiano,b:&PhysicalPiano,n:usize) {
    for (x,y) in a.left[..n].iter().zip(&b.left[..n]).chain(a.right[..n].iter().zip(&b.right[..n])) {
        assert!(x.is_finite() && y.is_finite(),"finite actual stereo");
        assert_eq!(x.to_bits(),y.to_bits(),"every raw stereo sample bit");
    }
}
fn measured(p:&PhysicalPiano)->piano_engine::Snapshot {
    let piano=&p.built.as_ref().unwrap().piano;
    let mut snapshot=piano.new_snapshot();piano.fill_snapshot(&mut snapshot,60);snapshot
}
fn same_public(a:&PhysicalPiano,b:&PhysicalPiano) {
    let (x,y)=(measured(a),measured(b));
    assert_eq!(x.shape_nodes,y.shape_nodes);assert_eq!(x.focus_midi,y.focus_midi);assert_eq!(x.awake,y.awake);
    let fields:[(&[f32],&[f32]);6]=[(&x.board,&y.board),(&x.string_level,&y.string_level),
        (&x.shape[0],&y.shape[0]),(&x.shape[1],&y.shape[1]),(&x.shape[2],&y.shape[2]),(&x.hammer_force,&y.hammer_force)];
    for (left,right) in fields {
        assert_eq!(left.len(),right.len());
        for (l,r) in left.iter().zip(right) {assert_eq!(l.to_bits(),r.to_bits(),"public measured f32 bits");}
    }
    assert!(x.action==y.action,"public real action observations");
    for midi in 0..128 {
        assert_eq!(a.shared.held[midi].load(Ordering::Relaxed),b.shared.held[midi].load(Ordering::Relaxed));
        assert_eq!(a.shared.velocity[midi].load(Ordering::Relaxed),b.shared.velocity[midi].load(Ordering::Relaxed));
    }
    assert!(a.captured==b.captured,"actual source capture membership");
    assert_eq!(a.shared.sustain.load(Ordering::Relaxed),b.shared.sustain.load(Ordering::Relaxed));
    assert_eq!(a.shared.soft.load(Ordering::Relaxed),b.shared.soft.load(Ordering::Relaxed));
    assert_eq!(a.shared.sostenuto.load(Ordering::Relaxed),b.shared.sostenuto.load(Ordering::Relaxed));
    for (l,r) in a.pedals.iter().zip(&b.pedals) {
        assert_eq!(l.sustain.to_bits(),r.sustain.to_bits());assert_eq!(l.soft,r.soft);assert_eq!(l.sostenuto,r.sostenuto);
    }
}
fn direct_render(p:&mut PhysicalPiano,start:usize,end:usize) {
    assert!(start<end,"reference never invokes zero-length engine work");
    p.built.as_mut().unwrap().piano.process(&mut p.left[start..end],&mut p.right[start..end]);
}
fn timed_pair(a:&mut PhysicalPiano,b:&mut PhysicalPiano,events:&[HostFixture]) {
    let mut input=events.iter().map(|(event,_)|*event);let mut polls=0usize;
    a.render_segments(TIMING_BLOCK,||{polls+=1;input.next()});
    assert_eq!(polls,events.len()+1,"actual iterator consumption exactly once plus end");
    assert!(input.next().is_none(),"all provided events consumed");
    let mut done=0usize;
    for (event,expected) in events {
        let at=event.timing() as usize;assert!(done<=at && at<=TIMING_BLOCK,"ordered finite fixture");
        if done<at {direct_render(b,done,at);}
        if let Some(input)=expected {b.input(*input);} done=at;
    }
    if done<TIMING_BLOCK {direct_render(b,done,TIMING_BLOCK);}
    same_raw(a,b,TIMING_BLOCK);same_public(a,b);
}
fn paired_empty_blocks(a:&mut PhysicalPiano,b:&mut PhysicalPiano,frames:usize)->bool {
    assert_eq!(frames%TIMING_BLOCK,0);
    let mut nonzero=false;
    for _ in 0..frames/TIMING_BLOCK {
        a.render_segments(TIMING_BLOCK,||None);direct_render(b,0,TIMING_BLOCK);
        same_raw(a,b,TIMING_BLOCK);
        nonzero|=a.left[..TIMING_BLOCK].iter().chain(&a.right[..TIMING_BLOCK]).any(|v|*v!=0.0);
    }
    same_public(a,b);nonzero
}
#[test]
fn real_segments_preserve_ordinary_offsets_and_same_time_order() {
    let (mut a,mut b)=timing_pair(Arc::new(Vec::new()));
    let events=[host_on(0,60,80),
        host_cc(1,64,0.8,Input::Sustain(0.8,FROM_HOST)),host_on(1,64,88),
        host_cc(64,66,1.0,Input::Sostenuto(true,FROM_HOST)),
        host_off(255,60)];
    timed_pair(&mut a,&mut b,&events);
    assert_eq!(measured(&a).action.latest_step,256);
    assert_eq!(a.shared.held[60].load(Ordering::Relaxed),0);assert!(a.captured[60]);
    assert!(a.player.index.is_none());assert_eq!(a.player.frame,0);
    assert!(paired_empty_blocks(&mut a,&mut b,TIMING_SUFFIX),"nonzero real post-event suffix");
    assert_eq!(measured(&a).action.latest_step,8448);
}
#[test]
fn real_segments_deliver_terminal_notes_and_pedals_once_without_demo_or_extra_frames() {
    let seconds=TIMING_BLOCK as f64/48_000.0;
    assert_eq!((seconds*48_000.0) as u64,TIMING_BLOCK as u64,"exact fixture sample");
    let demos=Arc::new(vec![demo::Demo {title:"Terminal sample fixture".into(),note:"In-memory; no device or file".into(),
        events:vec![piano_engine::midi::MidiEvent {seconds,kind:piano_engine::midi::MidiKind::NoteOn,a:72,b:80}],seconds}]);
    let (mut a,mut b)=timing_pair(demos);
    a.input(Input::PlayDemo(0));b.input(Input::PlayDemo(0));
    let n=TIMING_BLOCK as u32;
    let events=[(NoteEvent::MidiCC {timing:0,channel:0,cc:7,value:0.5},None),
        host_on(n,60,80),host_on(n,60,96),host_on(n,64,88),
        host_cc(n,66,1.0,Input::Sostenuto(true,FROM_HOST)),
        host_cc(n,64,0.4,Input::Sustain(0.4,FROM_HOST)),
        host_cc(n,67,1.0,Input::Soft(true,FROM_HOST))];
    timed_pair(&mut a,&mut b,&events);
    let trace=measured(&a).action;
    assert_eq!(trace.latest_step,256);assert_eq!(trace.stroke_sequence,3);
    assert!(trace.strokes().map(|s|s.midi).eq([60,60,64]),"actual repeated-note FIFO order");
    assert!(trace.strokes().all(|s|s.onset_step==256),"terminal dispatch advances no physical step");
    assert_eq!(a.shared.held[60].load(Ordering::Relaxed),FROM_HOST);assert_eq!(a.shared.held[64].load(Ordering::Relaxed),FROM_HOST);
    assert_eq!(a.shared.velocity[60].load(Ordering::Relaxed),96);assert_eq!(a.shared.velocity[64].load(Ordering::Relaxed),88);
    assert!(a.captured[60] && a.captured[64]);assert!(a.shared.soft.load(Ordering::Relaxed));
    assert_eq!(a.player.frame,256);assert_eq!(a.player.next,0);
    assert_eq!(a.shared.held[72].load(Ordering::Relaxed)&FROM_DEMO,0,"terminal demo event remains pending");
    a.input(Input::StopDemo);b.input(Input::StopDemo);
    assert!(paired_empty_blocks(&mut a,&mut b,TIMING_SUFFIX),"nonzero real terminal-note suffix");
    assert_eq!(measured(&a).action.latest_step,8448);
}
#[test]
fn real_segments_release_terminal_host_note_and_pedals_after_ringing_preroll() {
    let (mut a,mut b)=timing_pair(Arc::new(Vec::new()));
    for p in [&mut a,&mut b] {
        on(p,60);p.input(Input::Sustain(0.8,FROM_HOST));p.input(Input::Sostenuto(true,FROM_HOST));
    }
    assert!(paired_empty_blocks(&mut a,&mut b,4096),"real ringing preparation");
    let n=TIMING_BLOCK as u32;
    let events=[host_cc(n,66,0.0,Input::Sostenuto(false,FROM_HOST)),host_off(n,60),
        host_cc(n,64,0.0,Input::Sustain(0.0,FROM_HOST)),host_cc(n,67,1.0,Input::Soft(true,FROM_HOST))];
    timed_pair(&mut a,&mut b,&events);
    assert_eq!(measured(&a).action.latest_step,4352);
    assert_eq!(a.shared.held[60].load(Ordering::Relaxed),0);assert!(!a.captured[60]);
    assert_eq!(a.shared.sustain.load(Ordering::Relaxed),0);assert!(!a.shared.sostenuto.load(Ordering::Relaxed));
    assert!(a.shared.soft.load(Ordering::Relaxed));assert!(a.player.index.is_none());assert_eq!(a.player.frame,0);
    assert!(paired_empty_blocks(&mut a,&mut b,TIMING_SUFFIX),"nonzero real release suffix");
    assert_eq!(measured(&a).action.latest_step,12544);
}
