//! Integration regressions against the actual plugin state machine, background executor,
//! and mechanical engine. No audio device, GUI, or mocked engine/context is involved.
use super::*;
use shared::FROM_UI;

fn instrument() -> PhysicalPiano {
    let mut p = PhysicalPiano::default();
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
