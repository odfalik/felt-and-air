//! Offline renderer: plays a small score through the model and writes a WAV file.
//!   render <out.wav> [score] [oversample] [seconds]
//! Scores: "c4" (one mezzo forte note), "phrase" (a short two-hand phrase with pedal),
//! or the path of a MIDI file.

use piano_engine::midi::{lead_notes_by_flight, read_midi_file, MidiKind};
use piano_engine::*;

#[derive(Clone, Copy)]
enum Do {
    On(i32, i32),
    Off(i32, i32),
    Pedal(f64),
    Soft(bool),
    Sostenuto(bool),
}

fn phrase() -> Vec<(f64, Do)> {
    let mut e = Vec::new();
    let mut note = |t: f64, len: f64, midi: i32, vel: i32| {
        e.push((t, Do::On(midi, vel)));
        e.push((t + len, Do::Off(midi, 0)));
    };
    let q = 0.42;
    // Slow broken chords, C major to A minor to F to G, then a held cadence.
    let bass = [36, 33, 29, 31];
    let chords = [[48, 55, 64, 72], [45, 52, 60, 69], [41, 53, 60, 65], [43, 50, 59, 67]];
    let melody = [[76, 79], [77, 76], [72, 74], [71, 74]];
    for bar in 0..4 {
        let t = bar as f64 * 4.0 * q;
        note(t, 3.8 * q, bass[bar], 70);
        for i in 0..4 {
            note(t + i as f64 * q, 1.6 * q, chords[bar][i], 54 + 6 * i as i32);
        }
        note(t + 2.0 * q, 1.0 * q, melody[bar][0], 82);
        note(t + 3.0 * q, 0.9 * q, melody[bar][1], 74);
    }
    let t = 16.0 * q;
    for m in [24, 36, 48, 52, 55, 60, 64, 72] {
        note(t, 5.0, m, if m < 40 { 88 } else { 76 });
    }
    for bar in 0..4 {
        let t = bar as f64 * 4.0 * q;
        e.push((t + 0.02, Do::Pedal(1.0)));
        e.push((t + 3.9 * q, Do::Pedal(0.0)));
    }
    e.push((t + 0.02, Do::Pedal(1.0)));
    e.push((t + 5.2, Do::Pedal(0.0)));
    e
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or("out/render.wav".into());
    let score = args.get(2).cloned().unwrap_or("phrase".into());
    let os: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut g = GrandOptions::default();
    let mut ev: Vec<(f64, Do)>;
    let mut seconds;
    if score == "c4" {
        g.first_key = 40;
        g.last_key = 40;
        ev = vec![(0.05, Do::On(60, 80)), (4.0, Do::Off(60, 0))];
        seconds = 5.0;
    } else if score.ends_with(".mid") {
        let mut midi = read_midi_file(&score);
        lead_notes_by_flight(&mut midi, g.let_off_mm * 1e-3, midi_velocity_to_hammer_speed);
        ev = midi
            .iter()
            .map(|m| {
                (m.seconds, match m.kind {
                    MidiKind::NoteOn => Do::On(m.a, m.b),
                    MidiKind::NoteOff => Do::Off(m.a, m.b),
                    MidiKind::Sustain => Do::Pedal(m.a as f64 / 127.0),
                    MidiKind::Soft => Do::Soft(m.a >= 64),
                    MidiKind::Sostenuto => Do::Sostenuto(m.a >= 64),
                })
            })
            .collect();
        if ev.is_empty() {
            eprintln!("no notes in {score}");
            std::process::exit(1);
        }
        seconds = ev.last().unwrap().0 + 4.0;
    } else {
        ev = phrase();
        seconds = 13.5;
    }
    if let Some(s) = args.get(4).and_then(|s| s.parse().ok()) {
        seconds = s;
    }
    let helpers: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(3);

    let fs = 48000usize;
    let block = 64;
    let mut piano = Piano::new(&make_grand(&g), &PianoOptions { oversample: os, ..PianoOptions::default() }).expect("engine");
    piano.set_helper_threads(helpers, None, None);
    println!(
        "{} strings ({} nodes), board {} nodes at {:.1} x {:.1} mm, internal rate {} Hz",
        piano.string_count(),
        piano.total_string_nodes(),
        piano.plate().n,
        piano.plate().hx * 1e3,
        piano.plate().hy * 1e3,
        fs * os
    );
    ev.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    let mut wav: Vec<f32> = Vec::new();
    let (mut next, mut peak, mut max_awake) = (0, 0.0f32, 0);
    let t0 = std::time::Instant::now();
    let mut frame = 0;
    while frame < (seconds * fs as f64) as usize {
        while next < ev.len() && ev[next].0 * fs as f64 <= frame as f64 {
            match ev[next].1 {
                Do::On(m, v) => piano.note_on(m, midi_velocity_to_hammer_speed(v)),
                Do::Off(m, v) => piano.note_off(m, if v > 0 { v as f64 / 64.0 } else { 1.0 }),
                Do::Pedal(x) => piano.set_pedal(x),
                Do::Soft(x) => piano.set_soft_pedal(x),
                Do::Sostenuto(x) => piano.set_sostenuto(x),
            }
            next += 1;
        }
        piano.process(&mut l, &mut r);
        max_awake = max_awake.max(piano.awake_strings());
        for i in 0..block {
            peak = peak.max(l[i].abs()).max(r[i].abs());
            wav.push(l[i]);
            wav.push(r[i]);
        }
        frame += block;
    }
    let wall = t0.elapsed().as_secs_f64();
    // Normalise only if the render would clip; report the gain so levels stay traceable.
    let norm = if peak > 0.98 { 0.98 / peak } else { 1.0 };
    wav.iter_mut().for_each(|x| *x *= norm);
    util::write_wav_float(&out, &wav, 2, fs as u32).expect("write");
    println!("wrote {out}: {seconds:.1} s, peak {peak:.3} (scaled by {norm:.3}), up to {max_awake} strings awake, {:.2}x realtime", seconds / wall);
}
