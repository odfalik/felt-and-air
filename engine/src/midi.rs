//! Minimal Standard MIDI File reader: notes and the three pedals, with tempo changes,
//! flattened to a list of timed events.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MidiKind {
    /// a: note, b: release velocity (0 unknown).
    NoteOff,
    /// a: note, b: velocity.
    NoteOn,
    /// a: position 0..127.
    Sustain,
    Soft,
    Sostenuto,
}

#[derive(Clone, Copy, Debug)]
pub struct MidiEvent {
    pub seconds: f64,
    pub kind: MidiKind,
    pub a: i32,
    pub b: i32,
}

pub fn read_midi_file(path: &str) -> Vec<MidiEvent> {
    match std::fs::read(path) {
        Ok(d) => parse_midi(&d),
        Err(_) => Vec::new(),
    }
}

pub fn parse_midi(d: &[u8]) -> Vec<MidiEvent> {
    let mut out = Vec::new();
    if d.len() < 14 || &d[0..4] != b"MThd" {
        return out;
    }
    let be = |p: usize, n: usize| d[p..p + n].iter().fold(0u32, |v, &b| v << 8 | b as u32);
    let header = be(4, 4) as usize;
    let tracks = be(10, 2) as usize;
    let division = be(12, 2);
    if division == 0 || division & 0x8000 != 0 {
        return out; // SMPTE time is not supported
    }
    let mut p = 8 + header;

    // (tick, kind or tempo, a, b); tempo carries microseconds per quarter in a.
    let mut raw: Vec<(u64, Option<MidiKind>, i32, i32)> = Vec::new();
    for _ in 0..tracks {
        if p + 8 > d.len() || &d[p..p + 4] != b"MTrk" {
            break;
        }
        let len = be(p + 4, 4) as usize;
        p += 8;
        let end = d.len().min(p + len);
        let mut tick = 0u64;
        let mut status = 0u8;
        let var = |p: &mut usize| {
            let mut v = 0u32;
            while *p < end {
                let c = d[*p];
                *p += 1;
                v = v << 7 | (c & 0x7f) as u32;
                if c & 0x80 == 0 {
                    break;
                }
            }
            v
        };
        while p < end {
            tick += var(&mut p) as u64;
            if p >= end {
                break;
            }
            if d[p] & 0x80 != 0 {
                status = d[p];
                p += 1;
            }
            if status == 0xff {
                let meta = if p < end { d[p] } else { 0 };
                p += 1;
                let l = var(&mut p) as usize;
                if meta == 0x51 && l == 3 && p + 3 <= end {
                    raw.push((tick, None, be(p, 3) as i32, 0));
                }
                p += l;
            } else if status == 0xf0 || status == 0xf7 {
                p += var(&mut p) as usize;
            } else {
                let hi = status & 0xf0;
                let a = if p < end { d[p] as i32 } else { 0 };
                p += 1;
                let mut b = 0;
                if hi != 0xc0 && hi != 0xd0 {
                    b = if p < end { d[p] as i32 } else { 0 };
                    p += 1;
                }
                match (hi, a) {
                    (0x90, _) if b > 0 => raw.push((tick, Some(MidiKind::NoteOn), a, b)),
                    (0x80, _) => raw.push((tick, Some(MidiKind::NoteOff), a, b)),
                    (0x90, _) => raw.push((tick, Some(MidiKind::NoteOff), a, 0)),
                    (0xb0, 64) => raw.push((tick, Some(MidiKind::Sustain), b, 0)),
                    (0xb0, 67) => raw.push((tick, Some(MidiKind::Soft), b, 0)),
                    (0xb0, 66) => raw.push((tick, Some(MidiKind::Sostenuto), b, 0)),
                    _ => {}
                }
            }
        }
        p = end;
    }
    raw.sort_by_key(|e| e.0);
    let (mut seconds, mut us_per_quarter, mut last, mut pedal) = (0.0f64, 500000.0f64, 0u64, -1);
    for (tick, kind, a, b) in raw {
        seconds += (tick - last) as f64 * us_per_quarter * 1e-6 / division as f64;
        last = tick;
        let Some(kind) = kind else {
            us_per_quarter = a as f64;
            continue;
        };
        if kind == MidiKind::Sustain {
            if a == pedal {
                continue;
            }
            pedal = a;
        }
        out.push(MidiEvent { seconds, kind, a, b });
    }
    out
}

/// A recorded note-on marks the hammer reaching the string. The simulated hammer is let go a
/// little earlier and flies there, so start each note early by its flight time.
pub fn lead_notes_by_flight(events: &mut [MidiEvent], let_off_metres: f64, speed_of: fn(i32) -> f64) {
    for e in events.iter_mut() {
        if e.kind == MidiKind::NoteOn {
            e.seconds = (e.seconds - let_off_metres / speed_of(e.b)).max(0.0);
        }
    }
    events.sort_by(|a, b| a.seconds.partial_cmp(&b.seconds).unwrap());
}
