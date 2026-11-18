//! C interface, for the Mac app and the Audio Unit. Declared in include/piano_engine.h.

use crate::design::*;
use crate::midi::{lead_notes_by_flight, read_midi_file, MidiEvent, MidiKind};
use crate::piano::{Piano, PianoOptions, Snapshot, ThreadHook};
use std::ffi::{c_char, c_void, CStr};
use std::sync::Arc;

#[no_mangle]
pub extern "C" fn pp_options_default(out: *mut GrandOptions) {
    unsafe { *out = GrandOptions::default() };
}

#[no_mangle]
pub extern "C" fn pp_options_size() -> usize {
    std::mem::size_of::<GrandOptions>()
}

#[no_mangle]
pub extern "C" fn pp_velocity_to_speed(velocity: i32) -> f64 {
    midi_velocity_to_hammer_speed(velocity)
}

/// Returns null if the instrument cannot be built (for example at an unsupported rate).
#[no_mangle]
pub extern "C" fn pp_new(options: *const GrandOptions, sample_rate: f64, oversample: i32, grid_scale: f64) -> *mut Piano {
    let o = unsafe { &*options };
    let built = std::panic::catch_unwind(|| {
        Piano::new(&make_grand(o), &PianoOptions { sample_rate, oversample: oversample.max(1) as usize, board_grid_scale: grid_scale, ..PianoOptions::default() })
    });
    match built {
        Ok(Ok(p)) => Box::into_raw(Box::new(p)),
        _ => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn pp_free(p: *mut Piano) {
    if !p.is_null() {
        drop(unsafe { Box::from_raw(p) });
    }
}

#[no_mangle]
pub extern "C" fn pp_note_on(p: *mut Piano, midi: i32, hammer_speed: f64) {
    unsafe { (*p).note_on(midi, hammer_speed) }
}
#[no_mangle]
pub extern "C" fn pp_note_off(p: *mut Piano, midi: i32, release_speed: f64) {
    unsafe { (*p).note_off(midi, release_speed) }
}
#[no_mangle]
pub extern "C" fn pp_set_pedal(p: *mut Piano, position: f64) {
    unsafe { (*p).set_pedal(position) }
}
#[no_mangle]
pub extern "C" fn pp_set_soft_pedal(p: *mut Piano, down: bool) {
    unsafe { (*p).set_soft_pedal(down) }
}
#[no_mangle]
pub extern "C" fn pp_set_sostenuto(p: *mut Piano, down: bool) {
    unsafe { (*p).set_sostenuto(down) }
}

/// Renders n stereo frames. Allocation and lock free.
#[no_mangle]
pub extern "C" fn pp_process(p: *mut Piano, left: *mut f32, right: *mut f32, n: i32) {
    unsafe { (*p).process(std::slice::from_raw_parts_mut(left, n as usize), std::slice::from_raw_parts_mut(right, n as usize)) }
}

struct Hook(extern "C" fn(*mut c_void), *mut c_void);
unsafe impl Send for Hook {}
unsafe impl Sync for Hook {}

/// on_start and on_exit run once on each helper thread, with `context`; either may be null.
#[no_mangle]
pub extern "C" fn pp_set_helper_threads(p: *mut Piano, count: i32, on_start: Option<extern "C" fn(*mut c_void)>, on_exit: Option<extern "C" fn(*mut c_void)>, context: *mut c_void) {
    let wrap = |f: Option<extern "C" fn(*mut c_void)>| -> Option<ThreadHook> {
        f.map(|f| {
            let h = Hook(f, context);
            Arc::new(move || {
                let h = &h;
                (h.0)(h.1)
            }) as ThreadHook
        })
    };
    unsafe { (*p).set_helper_threads(count.max(0) as usize, wrap(on_start), wrap(on_exit)) }
}

#[no_mangle]
pub extern "C" fn pp_helpers_busy(p: *const Piano) -> bool {
    unsafe { (*p).helpers_busy() }
}
#[no_mangle]
pub extern "C" fn pp_string_count(p: *const Piano) -> i32 {
    unsafe { (*p).string_count() as i32 }
}
#[no_mangle]
pub extern "C" fn pp_awake_strings(p: *const Piano) -> i32 {
    unsafe { (*p).awake_strings() as i32 }
}
#[no_mangle]
pub extern "C" fn pp_board_nodes(p: *const Piano) -> i32 {
    unsafe { (*p).plate().n as i32 }
}
#[no_mangle]
pub extern "C" fn pp_board_spacing(p: *const Piano, hx: *mut f64, hy: *mut f64) {
    unsafe {
        *hx = (*p).plate().hx;
        *hy = (*p).plate().hy;
    }
}
#[no_mangle]
pub extern "C" fn pp_reverb_time(p: *const Piano) -> f64 {
    unsafe { (*p).reverb_time() }
}

/// Grid storage index of the board node nearest to a point (piano coordinates), or -1 off the board.
#[no_mangle]
pub extern "C" fn pp_board_node_at(p: *const Piano, x: f64, y: f64) -> i32 {
    let it = unsafe { (*p).plate().interp(Vec2 { x, y }) };
    let (mut best, mut weight) = (-1, 0.2);
    for k in 0..4 {
        if it.w[k] > weight {
            weight = it.w[k];
            best = it.idx[k] as i32;
        }
    }
    best
}

#[no_mangle]
pub extern "C" fn pp_string_bridge(p: *const Piano, index: i32, x: *mut f64, y: *mut f64) {
    unsafe {
        let b = (*p).string_bridge(index as usize);
        *x = b.x;
        *y = b.y;
    }
}

fn copy_points(points: &[Vec2], xy: *mut f64, max_points: i32) -> i32 {
    let n = points.len().min(max_points.max(0) as usize);
    for (i, p) in points.iter().take(n).enumerate() {
        unsafe {
            *xy.add(2 * i) = p.x;
            *xy.add(2 * i + 1) = p.y;
        }
    }
    n as i32
}

/// Soundboard outline in piano coordinates; returns the number of points written.
#[no_mangle]
pub extern "C" fn pp_outline(options: *const GrandOptions, xy: *mut f64, max_points: i32) -> i32 {
    copy_points(&make_grand(unsafe { &*options }).board.outline, xy, max_points)
}

/// Bridge line `which` (0 bass, 1 treble).
#[no_mangle]
pub extern "C" fn pp_bridge(options: *const GrandOptions, which: i32, xy: *mut f64, max_points: i32) -> i32 {
    let d = make_grand(unsafe { &*options });
    d.board.bridges.get(which as usize).map_or(0, |b| copy_points(b, xy, max_points))
}

/// The two ears, piano coordinates: x, y, z for the left, then for the right.
#[no_mangle]
pub extern "C" fn pp_ears(options: *const GrandOptions, xyz: *mut f64) {
    let d = make_grand(&GrandOptions { first_key: 40, last_key: 40, ..unsafe { *options } });
    for e in 0..2 {
        unsafe {
            *xyz.add(3 * e) = d.ears[e].x;
            *xyz.add(3 * e + 1) = d.ears[e].y;
            *xyz.add(3 * e + 2) = d.ears[e].z;
        }
    }
}

#[repr(C)]
pub struct SnapshotView {
    board: *const f32,
    board_len: i32,
    string_level: *const f32,
    string_count: i32,
    shape: [*const f32; 3],
    shape_nodes: [i32; 3],
    hammer_force: [f32; 3],
    focus_midi: i32,
    awake: i32,
}

#[no_mangle]
pub extern "C" fn pp_snapshot_new(p: *const Piano) -> *mut Snapshot {
    Box::into_raw(Box::new(unsafe { (*p).new_snapshot() }))
}
#[no_mangle]
pub extern "C" fn pp_snapshot_free(s: *mut Snapshot) {
    if !s.is_null() {
        drop(unsafe { Box::from_raw(s) });
    }
}
/// Allocation free: may be called from the audio thread.
#[no_mangle]
pub extern "C" fn pp_snapshot_fill(p: *const Piano, s: *mut Snapshot, focus_midi: i32) {
    unsafe { (*p).fill_snapshot(&mut *s, focus_midi) }
}
/// Copies between two snapshots of the same instrument without allocating.
#[no_mangle]
pub extern "C" fn pp_snapshot_copy(dst: *mut Snapshot, src: *const Snapshot) {
    let (d, s) = unsafe { (&mut *dst, &*src) };
    d.board.copy_from_slice(&s.board);
    d.string_level.copy_from_slice(&s.string_level);
    for i in 0..3 {
        d.shape[i].copy_from_slice(&s.shape[i]);
    }
    d.shape_nodes = s.shape_nodes;
    d.hammer_force = s.hammer_force;
    d.focus_midi = s.focus_midi;
    d.awake = s.awake;
}
#[no_mangle]
pub extern "C" fn pp_snapshot_view(s: *const Snapshot, out: *mut SnapshotView) {
    let s = unsafe { &*s };
    unsafe {
        *out = SnapshotView {
            board: s.board.as_ptr(),
            board_len: s.board.len() as i32,
            string_level: s.string_level.as_ptr(),
            string_count: s.string_level.len() as i32,
            shape: [s.shape[0].as_ptr(), s.shape[1].as_ptr(), s.shape[2].as_ptr()],
            shape_nodes: [s.shape_nodes[0] as i32, s.shape_nodes[1] as i32, s.shape_nodes[2] as i32],
            hammer_force: s.hammer_force,
            focus_midi: s.focus_midi,
            awake: s.awake as i32,
        }
    };
}

/// Reads a MIDI file. Notes are started early by their hammer's flight time over
/// `let_off_metres`, so that they sound when the recording says. Null if nothing was read.
#[no_mangle]
pub extern "C" fn pp_midi_read(path: *const c_char, let_off_metres: f64) -> *mut Vec<MidiEvent> {
    let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else { return std::ptr::null_mut() };
    let mut ev = read_midi_file(path);
    if ev.is_empty() {
        return std::ptr::null_mut();
    }
    lead_notes_by_flight(&mut ev, let_off_metres, midi_velocity_to_hammer_speed);
    Box::into_raw(Box::new(ev))
}
#[no_mangle]
pub extern "C" fn pp_midi_free(m: *mut Vec<MidiEvent>) {
    if !m.is_null() {
        drop(unsafe { Box::from_raw(m) });
    }
}
#[no_mangle]
pub extern "C" fn pp_midi_count(m: *const Vec<MidiEvent>) -> i32 {
    unsafe { (*m).len() as i32 }
}
/// kind: 0 note off (b release velocity), 1 note on (b velocity), 2 sustain, 3 soft, 4 sostenuto (a position 0..127).
#[no_mangle]
pub extern "C" fn pp_midi_get(m: *const Vec<MidiEvent>, index: i32, seconds: *mut f64, kind: *mut i32, a: *mut i32, b: *mut i32) {
    let e = unsafe { (&(*m))[index as usize] };
    unsafe {
        *seconds = e.seconds;
        *kind = match e.kind {
            MidiKind::NoteOff => 0,
            MidiKind::NoteOn => 1,
            MidiKind::Sustain => 2,
            MidiKind::Soft => 3,
            MidiKind::Sostenuto => 4,
        };
        *a = e.a;
        *b = e.b;
    }
}

#[repr(C)]
pub struct ParamInfo {
    /// 0 slider, 1 switch, 2 position on the floor plan.
    kind: i32,
    tab: i32,
    name: *const c_char,
    lo: f64,
    hi: f64,
    log: bool,
    format: *const c_char,
    help: *const c_char,
}

#[no_mangle]
pub extern "C" fn pp_param_count() -> i32 {
    crate::params::PARAMS.len() as i32
}
#[no_mangle]
pub extern "C" fn pp_param_info(index: i32, out: *mut ParamInfo) {
    let p = &crate::params::PARAMS[index as usize];
    unsafe {
        *out = ParamInfo {
            kind: p.kind as i32,
            tab: p.tab as i32,
            name: p.c_name.as_ptr() as *const c_char,
            lo: p.lo,
            hi: p.hi,
            log: p.log,
            format: p.c_format.as_ptr() as *const c_char,
            help: p.c_help.as_ptr() as *const c_char,
        }
    };
}
#[no_mangle]
pub extern "C" fn pp_param_get(options: *const GrandOptions, index: i32) -> f64 {
    (crate::params::PARAMS[index as usize].get)(unsafe { &*options })
}
/// The value is held inside the parameter's range.
#[no_mangle]
pub extern "C" fn pp_param_set(options: *mut GrandOptions, index: i32, value: f64) {
    crate::params::PARAMS[index as usize].write(unsafe { &mut *options }, value);
}
