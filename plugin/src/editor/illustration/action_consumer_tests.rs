//! Consumer tests from REAL accepted builds/Piano/Snapshot data, not fabricated motion.
//! Controlled UI arrival timing is not native timing or full State.show/publication proof.
use super::*;

fn real_plugin(fs:f64)->crate::PhysicalPiano {
    let mut p=crate::PhysicalPiano::with_demos(||std::sync::Arc::new(Vec::new()));
    assert!(p.initialize_engine(fs,512)); p
}
fn render_real(p:&mut crate::PhysicalPiano,mut frames:usize) {
    let mut left=[0.0;512]; let mut right=[0.0;512];
    while frames>0 {
        let n=frames.min(512);
        p.built.as_mut().unwrap().piano.process(&mut left[..n],&mut right[..n]);
        assert!(left[..n].iter().chain(&right[..n]).all(|x|x.is_finite())); frames-=n;
    }
}
fn real_trace(p:&crate::PhysicalPiano)->(u64,ActionTrace) {
    let mut display=p.shared.display.lock().unwrap();
    let built=p.built.as_ref().unwrap();
    assert_eq!(display.generation,built.generation);
    assert_eq!(display.view.as_ref().expect("actual build BoardView").generation,display.generation);
    built.piano.fill_snapshot(&mut display.snap,-1);
    (display.generation,display.snap.action)
}
fn short_tap(p:&mut crate::PhysicalPiano,midi:i32) {
    p.input(crate::shared::Input::NoteOn {midi,velocity:80,from:crate::shared::FROM_UI});
    p.input(crate::shared::Input::NoteOff {midi,from:crate::shared::FROM_UI,release:1.0});
}
fn live(state:RenderState)->SampledPose {
    match state {RenderState::Live(sample)=>sample,_=>panic!("actual producer bracket must be live: {state:?}")}
}
// Count ONLY actual new retained strokes displayed DURING the8-tap playing window.
fn retained_dense_strokes_are_displayed_during_playing(p:&mut crate::PhysicalPiano,ui_base:f64) {
    assert_eq!(p.sample_rate,48000.0);
    let (_,before)=real_trace(p);let origin=before.latest_step;
    let mut art=ActionIllustration::default();let mut published:Option<ActionTrace>=None;
    let (mut position,mut note,mut publication,mut ui_tick)=(0usize,0usize,1usize,1usize);
    let mut displayed=Vec::new();
    while position<8192 {
        if note<8 && position==note*960 {short_tap(p,60);note+=1;}
        let next_note=if note<8 {note*960} else {8192};
        let next=next_note.min(publication*2048).min(ui_tick*800).min(8192);
        assert!(next>position);render_real(p,next-position);position=next;
        let now=ui_base+position as f64/48000.0;
        if position==publication*2048 {
            let (generation,trace)=real_trace(p);
            assert!(trace.valid && trace.poses().next().is_some());
            art.receive(generation,&trace,now);published=Some(trace);publication+=1;
        }
        if position==ui_tick*800 {
            if let Some(trace)=&published {
                if let RenderState::Live(sample)=art.render_state(trace,now) {
                    let owner=trace.stroke(sample.meta.stroke_sequence).expect("EXACT displayed owner");
                    assert_eq!(*owner,sample.meta);
                    if position<7*960 && owner.onset_step>=origin && owner.onset_step<trace.latest_step {
                        if !displayed.contains(&owner.stroke_sequence) {displayed.push(owner.stroke_sequence);}
                    }
                }
            }
            ui_tick+=1;
        }
    }
    assert_eq!(position,8192);assert_eq!(note,8);assert_eq!(publication-1,4);
    assert!(displayed.len()>=2,">=2 REAL new retained stroke IDs BEFORE last launch; no prelude/post-stream credit");
}

#[test]
fn real_consumer_clock_brackets_references_freeze_and_short_contact_are_consistent() {
    for fs in [44100.0,48000.0] {
        let mut p=real_plugin(fs); let mut art=ActionIllustration::default();
        let (generation,initial)=real_trace(&p);
        art.receive(generation,&initial,0.0);
        assert_eq!(art.render_state(&initial,0.0),RenderState::Neutral);
        short_tap(&mut p,60);
        // Real512-frame physics blocks at ceil(threshold/block); only UI receipt jitter
        // is controlled. We do NOT claim to run NIH/native publication/event delivery here.
        let quantum=((fs/30.0/512.0).ceil() as usize)*512;
        render_real(&mut p,quantum);
        let (_,first)=real_trace(&p);
        let arrival1=first.latest_step as f64*first.internal_dt_s+0.002;
        art.receive(generation,&first,arrival1);
        let anchor=(art.anchor_engine_s,art.anchor_ui_s);
        assert_eq!(art.render_state(&first,arrival1),RenderState::Buffering,"valid-data initial-fill WAIT");
        assert!(!art.suspended,"initial fill must not restart/resync its clock");
        let records:Vec<_>=first.poses().collect();
        let pair=records.windows(2).find(|pair| {
            let a=ActionIllustration::measured(first.current_stroke().unwrap(),&pair[0].pose).unwrap();
            let b=ActionIllustration::measured(first.current_stroke().unwrap(),&pair[1].pose).unwrap();
            a.phase==b.phase && a.head_m!=b.head_m && a.string_m!=b.string_m
                && a.string_m.iter().chain(&b.string_m).any(|value|*value!=0.0)
        }).expect("actual short tap has a moving same-phase bracket");
        let cursor=(pair[0].pose.pose_step+pair[1].pose.pose_step) as f64*first.internal_dt_s*0.5;
        let now=anchor.1+(cursor-anchor.0);
        assert!(now>=arrival1);
        let shown=live(art.render_state(&first,now));
        assert_eq!(shown.source.lower_step,pair[0].pose.pose_step);
        assert_eq!(shown.source.upper_step,pair[1].pose.pose_step);
        let a=ActionIllustration::measured(first.current_stroke().unwrap(),&pair[0].pose).unwrap();
        let b=ActionIllustration::measured(first.current_stroke().unwrap(),&pair[1].pose).unwrap();
        assert!((shown.draw.head_m-(a.head_m+(b.head_m-a.head_m)*shown.source.alpha)).abs()<1e-15);
        for i in 0..first.current_stroke().unwrap().string_count as usize {
            assert!((shown.draw.string_m[i]-(a.string_m[i]+(b.string_m[i]-a.string_m[i])*shown.source.alpha)).abs()<1e-15);
            let clear_a=a.string_m[i]-a.head_m; let clear_b=b.string_m[i]-b.head_m;
            assert!(((shown.draw.string_m[i]-shown.draw.head_m)-(clear_a+(clear_b-clear_a)*shown.source.alpha)).abs()<1e-15);
        }
        if let Some(boundary)=records.windows(2).find(|pair|pair[0].pose.phase!=pair[1].pose.phase) {
            let boundary_cursor=(boundary[0].pose.pose_step+boundary[1].pose.pose_step) as f64*first.internal_dt_s*0.5;
            let endpoint=ActionIllustration::sample(&first,boundary_cursor).expect("real metadata owner").expect("actual phase boundary");
            assert_eq!(endpoint.draw,ActionIllustration::measured(first.current_stroke().unwrap(),&boundary[0].pose).unwrap());
            assert_eq!(endpoint.source.alpha,0.0,"phase boundary must not invent interpolation");
            assert_eq!(endpoint.source.lower_step,endpoint.source.upper_step);
        } else {panic!("real short tap must include a phase boundary");}
        render_real(&mut p,quantum);
        let (_,trace)=real_trace(&p);
        let arrival2=trace.latest_step as f64*trace.internal_dt_s+0.007;
        assert!(arrival2>now);
        art.receive(generation,&trace,arrival2);
        assert_eq!(anchor,(art.anchor_engine_s,art.anchor_ui_s),"healthy jitter must not re-anchor");
        let elapsed_cursor=art.anchor_engine_s+(arrival2-art.anchor_ui_s);
        assert!((elapsed_cursor-shown.source.cursor_s-(arrival2-now)).abs()<1e-12,"healthy clock advances1x, not packet rebasing");
        let latest_shown=live(art.render_state(&trace,arrival2));
        assert!((latest_shown.source.cursor_s-elapsed_cursor).abs()<1e-12,"healthy jittered receipt must actually render1x Live");
        let later_cursor=cursor+1.0/60.0;
        let later=ActionIllustration::sample(&trace,later_cursor).expect("real metadata owner").expect("actual next display interval is bracketed");
        assert!(matches!(later.draw.phase,ActionPhase::Returning|ActionPhase::Rest),"real short tap already rebounds");
        let cue=ActionIllustration::recent_contact(&trace,later_cursor).expect("subframe contact survives phase/8-sample filtering");
        assert_eq!(cue.pose_step,cue.solve_step+1);
        let episode_pose=trace.contacts().flat_map(|contact|[&contact.entry_pose,&contact.peak_pose])
            .find(|pose|pose.pose_step==cue.pose_step).unwrap();
        assert!(episode_pose.force_n[cue.wire]>0.0);
        assert_eq!(cue.pose.string_m[cue.wire].to_bits(),episode_pose.string_along_m[cue.wire].to_bits());
        let before_cursor:Vec<_>=trace.poses().filter(|record|record.pose.pose_step as f64*trace.internal_dt_s<=later_cursor).collect();
        assert!(before_cursor.len()>8 && cue.pose_step<before_cursor[before_cursor.len()-8].pose.pose_step,
            "actual short-contact event must be older than the latest8 routine samples");
        // No intervening draw: stale/missing data must retain EXACTLY the last DISPLAYED
        // interpolation and its head/string/bracket provenance, not jump to either endpoint.
        let missing_now=anchor.1+(trace.latest_step as f64*trace.internal_dt_s+0.002-anchor.0);
        assert_eq!(art.render_state(&trace,missing_now),RenderState::Paused(latest_shown));
        assert_eq!(art.render_state(&trace,missing_now+STALE_S+0.001),RenderState::Paused(latest_shown));
        // A REAL historical contact can outlive the retained routine prefix.
        // It must remain a separate ghost/marker, not a trail across missing history.
        let target_frames=(fs as usize)*14/100;
        render_real(&mut p,target_frames-2*quantum);
        let (_,short_history)=real_trace(&p);
        let delayed=short_history.latest_step as f64*short_history.internal_dt_s-LOOKBEHIND_S;
        let older_cue=ActionIllustration::recent_contact(&short_history,delayed).expect("real recent contact outlives routine prefix");
        assert!(older_cue.pose_step<short_history.poses().next().unwrap().pose.pose_step);
    }
}
#[test]
fn real_consumer_identity_gap_and_fresh_resync_never_reuse_old_motion() {
    let mut p=real_plugin(48000.0); let mut art=ActionIllustration::default();
    short_tap(&mut p,60); render_real(&mut p,4096);
    let (generation,trace)=real_trace(&p);
    let now=trace.latest_step as f64*trace.internal_dt_s+0.002;
    art.receive(generation,&trace,now);
    let shown=live(art.render_state(&trace,now));
    let last_sequence=trace.pose_sequence;
    render_real(&mut p,8192); // REAL progression, not seeded sequence/history values.
    let (_,gap)=real_trace(&p);
    assert!(gap.pose_overwrites>0);
    assert!(gap.poses().next().unwrap().sequence>last_sequence+1);
    let gap_now=gap.latest_step as f64*gap.internal_dt_s+0.002;
    art.receive(generation,&gap,gap_now);
    assert_eq!(art.render_state(&gap,gap_now),RenderState::Paused(shown),"gap cannot tween old to new");
    render_real(&mut p,2048);
    let (_,fresh)=real_trace(&p);
    let fresh_now=fresh.latest_step as f64*fresh.internal_dt_s+0.002;
    art.receive(generation,&fresh,fresh_now);
    let resumed=live(art.render_state(&fresh,fresh_now));
    assert!(resumed.source.cursor_s>=shown.source.cursor_s);
    assert!(resumed.source.lower_step>=fresh.poses().next().unwrap().pose.pose_step);
    // Invalid UI receipt timing is controlled; physics clock/history are untouched.
    art.receive(generation,&fresh,fresh_now-0.01);
    assert_eq!(art.render_state(&fresh,fresh_now+0.001),RenderState::Paused(resumed));
    let mut stroke_clock=ActionIllustration::default();
    stroke_clock.receive(generation,&fresh,fresh_now);
    let old_display=live(stroke_clock.render_state(&fresh,fresh_now));
    let healthy_anchor=(stroke_clock.anchor_engine_s,stroke_clock.anchor_ui_s);
    short_tap(&mut p,62); render_real(&mut p,512);
    let (_,retrigger)=real_trace(&p);
    assert_ne!(retrigger.stroke_sequence,fresh.stroke_sequence);
    stroke_clock.receive(generation,&retrigger,fresh_now+0.020);
    assert_eq!(healthy_anchor,(stroke_clock.anchor_engine_s,stroke_clock.anchor_ui_s),"latest focus must not reset the healthy GENERATION clock");
    let old_stroke=live(stroke_clock.render_state(&retrigger,fresh_now+0.020));
    assert_eq!(old_stroke.meta.stroke_sequence,old_display.meta.stroke_sequence);
    assert_eq!(old_stroke.meta.midi,60);
    assert_eq!(retrigger.current_stroke().unwrap().midi,62);
    let old_owner=retrigger.stroke(old_display.meta.stroke_sequence).unwrap();
    assert!(!old_owner.observing);
    let boundary=old_owner.observation_end_step;
    let terminal=retrigger.poses().filter(|record|record.stroke_sequence==old_owner.stroke_sequence && record.pose.pose_step==boundary)
        .last().expect("actual old pre-retarget terminal endpoint");
    let before=retrigger.poses().filter(|record|record.stroke_sequence==old_owner.stroke_sequence && record.pose.pose_step<boundary)
        .last().expect("actual old distinct point before boundary");
    assert!(before.pose.pose_step<boundary);
    let cursor_before=(before.pose.pose_step+boundary) as f64*retrigger.internal_dt_s*0.5;
    let strict_old=ActionIllustration::sample(&retrigger,cursor_before).expect("old actual metadata").expect("strictly-before old bracket");
    assert_eq!(strict_old.meta.stroke_sequence,old_owner.stroke_sequence);
    assert_eq!(strict_old.source.lower_step,before.pose.pose_step);
    if before.pose.phase==terminal.pose.phase {
        assert_eq!(strict_old.source.upper_step,boundary,"old terminal is upper ONLY strictly before S");
        assert!(strict_old.source.alpha>0.0 && strict_old.source.alpha<1.0);
    } else {assert_eq!(strict_old.source.alpha,0.0);assert_eq!(strict_old.source.upper_step,strict_old.source.lower_step);}
    let at_boundary=ActionIllustration::sample(&retrigger,boundary as f64*retrigger.internal_dt_s).expect("new actual metadata").expect("exact boundary seed");
    let global_winner=retrigger.poses().filter(|record|record.pose.pose_step==boundary).last().unwrap();
    assert_eq!(at_boundary.meta.stroke_sequence,global_winner.stroke_sequence);
    assert_eq!(at_boundary.meta.stroke_sequence,retrigger.current_stroke().unwrap().stroke_sequence);
    assert!(p.initialize_engine(48000.0,512)); // REAL lifecycle/accepted build generation.
    let (replacement_generation,replacement)=real_trace(&p);
    assert_ne!(replacement_generation,generation);
    art.receive(replacement_generation,&replacement,fresh_now+0.030);
    assert_eq!(art.render_state(&replacement,fresh_now+0.030),RenderState::Neutral);
    assert!(art.generation.is_none() && art.frozen.is_none());
    // Same valid MIDI/stroke identity across TWO REAL accepted generations.
    // The second consumer holds the saved ORIGINAL valid60/stroke1 immediately
    // before it receives replacement generation's valid60/stroke1, not an invalid trace.
    let mut collision=ActionIllustration::default();
    collision.receive(generation,&trace,now);
    let original_shown=live(collision.render_state(&trace,now));
    assert_eq!(original_shown.meta.midi,trace.current_stroke().unwrap().midi);
    assert_eq!(original_shown.meta.stroke_sequence,trace.stroke_sequence);
    short_tap(&mut p,60); render_real(&mut p,4096);
    let (valid_generation,valid_replacement)=real_trace(&p);
    assert_eq!(valid_generation,replacement_generation);
    assert_ne!(valid_generation,generation);
    assert_eq!(valid_replacement.current_stroke().unwrap().midi,trace.current_stroke().unwrap().midi);
    assert_eq!(valid_replacement.stroke_sequence,trace.stroke_sequence);
    assert_eq!(collision.frozen,Some(original_shown));
    let valid_now=fresh_now+0.150;
    collision.receive(valid_generation,&valid_replacement,valid_now);
    assert_eq!(collision.generation,Some(valid_generation));
    assert!(collision.frozen.is_none(),"same MIDI/stroke in a new real generation invalidates old displayed provenance");
    let replacement_shown=live(collision.render_state(&valid_replacement,valid_now));
    assert!(replacement_shown.source.cursor_s>=0.0);
    retained_dense_strokes_are_displayed_during_playing(&mut p,valid_now+0.100);
    let (_,owned)=real_trace(&p);
    let old_id=owned.current_stroke().unwrap().stroke_sequence;
    let old_point=owned.poses().filter(|record|record.stroke_sequence==old_id && record.pose.pose_step<owned.latest_step)
        .last().expect("real old-stroke point before retarget boundary").pose.pose_step;
    // REAL same-step launches, no seeded IDs/metadata/buffers; oldest owner is evicted
    // while its recently captured raw point remains. GLOBAL last seed wins at S.
    for _ in 0..20 {short_tap(&mut p,60);}
    let (_,evicted)=real_trace(&p);
    assert!(evicted.stroke(old_id).is_none());
    assert!(evicted.poses().any(|record|record.stroke_sequence==old_id && record.pose.pose_step==old_point));
    assert_eq!(ActionIllustration::sample(&evicted,old_point as f64*evicted.internal_dt_s),Err(old_id),
        "missing exact owner must be UNAVAILABLE, never recycled/current metadata");
    let exact=evicted.latest_step as f64*evicted.internal_dt_s;
    let winner=ActionIllustration::sample(&evicted,exact).expect("actual newest owner retained").expect("same-step seed");
    assert_eq!(winner.meta.stroke_sequence,evicted.poses().last().unwrap().stroke_sequence);
    assert_eq!(winner.meta.stroke_sequence,evicted.current_stroke().unwrap().stroke_sequence);
}
