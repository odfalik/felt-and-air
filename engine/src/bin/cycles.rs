//! Work per time step in processor cycles, on one thread, for a pedalled recording.
//! Compare alternating builds on the same machine: core placement, cache contention and
//! power management can still affect cycle counts. Use `bench` for wall-clock deadlines.
//!   cycles <file.mid> [grid scale]
use piano_engine::midi::{read_midi_file, MidiKind};
use piano_engine::*;

extern "C" {
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut u8) -> i32;
    fn getpid() -> i32;
}

/// (instructions, cycles) used by this process so far.
pub fn counters() -> (f64, f64) {
    // rusage_info_v4: a 16 byte uuid, then 64 bit fields; instructions and cycles are fields 29 and 30.
    let mut buf = [0u8; 16 + 8 * 40];
    let status = unsafe { proc_pid_rusage(getpid(), 4, buf.as_mut_ptr()) };
    assert_eq!(status, 0, "cannot read process hardware counters");
    let field = |i: usize| u64::from_le_bytes(buf[16 + 8 * i..24 + 8 * i].try_into().unwrap()) as f64;
    (field(29), field(30))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ev = read_midi_file(args.get(1).map(|s| s.as_str()).unwrap_or("demos/05.mid"));
    if ev.is_empty() {
        eprintln!("no MIDI events; provide a readable performance file");
        std::process::exit(1);
    }
    let grid: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.5);
    let mut p = Piano::new(&make_grand(&GrandOptions::default()), &PianoOptions { board_grid_scale: grid, ..PianoOptions::default() }).unwrap();
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    let (mut next, mut start, mut awake) = (0, (0.0, 0.0), 0);
    for b in 0..1900 {
        let t = b as f64 * 256.0 / 48000.0;
        while next < ev.len() && ev[next].seconds <= t {
            let e = ev[next];
            match e.kind {
                MidiKind::NoteOn => p.note_on(e.a, midi_velocity_to_hammer_speed(e.b)),
                MidiKind::NoteOff => p.note_off(e.a, 1.0),
                MidiKind::Sustain => p.set_pedal(e.a as f64 / 127.0),
                _ => {}
            }
            next += 1;
        }
        if b == 900 {
            start = counters();
        }
        p.process(&mut l, &mut r);
        awake = awake.max(p.awake_strings());
    }
    let end = counters();
    let steps = 1000.0 * 256.0;
    println!(
        "{:.0} cycles and {:.0} instructions per step on one thread (48 kHz, grid {grid}), up to {} strings awake",
        (end.1 - start.1) / steps,
        (end.0 - start.0) / steps,
        awake
    );
}
