//! Standalone MIDI discovery and routing. No part of this module runs on the audio thread.
//! A single owner enumerates every 500 ms, preserving connections by midir port identity
//! (CoreMIDI unique ID, not display name). Callback/router locking never involves audio.
use midir::{MidiInput, MidiInputConnection, MidiInputPort};
use physical_piano_plugin::shared::{Input, EXTERNAL_MIDI, EXTERNAL_PANIC, FROM_HOST};
use std::sync::{atomic::Ordering, mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct MidiWatcher {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}
impl MidiWatcher {
    /// Starts only for live automatic MIDI mode, never dummy/help/explicit --midi-input.
    pub fn start() -> std::io::Result<Self> {
        let (stop, stopped) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("piano-midi-discovery".into())
            .spawn(move || watch(stopped))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for MidiWatcher {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Connection {
    port: MidiInputPort,
    token: u64,
    connection: MidiInputConnection<()>,
}
fn emit(input: Input) {
    if EXTERNAL_MIDI.push(input).is_err() {
        EXTERNAL_PANIC.store(true, Ordering::Release);
    }
}
fn watch(stopped: mpsc::Receiver<()>) {
    let router = Arc::new(Mutex::new(Router::default()));
    let mut enumerator = None;
    let mut connections: Vec<Connection> = Vec::new();
    let mut next_token = 0;
    loop {
        // Creation failure is retryable. Do not discard established connections on an
        // enumerator creation failure; no empty inventory has been observed in that case.
        if enumerator.is_none() {
            enumerator = MidiInput::new("Felt & Air discovery").ok();
        }
        if let Some(input) = &enumerator {
            let ports = unique_ports(input.ports());
            for i in (0..connections.len()).rev() {
                if !ports.contains(&connections[i].port) {
                    let old = connections.swap_remove(i);
                    // Disable the token first. Already queued callbacks cannot resurrect it.
                    router.lock().unwrap().remove(old.token, emit);
                    // Never close while holding the router lock: close may await a callback
                    // which holds midir's own handler lock and is waiting for the router.
                    drop(old.connection.close());
                }
            }
            for port in ports {
                if connections.iter().any(|c| c.port == port) {
                    continue;
                }
                let Ok(mut input) = MidiInput::new("Felt & Air") else {
                    continue;
                };
                input.ignore(midir::Ignore::None);
                next_token += 1;
                let token = next_token;
                router.lock().unwrap().add(token);
                let callback_router = router.clone();
                match input.connect(
                    &port,
                    "Felt & Air input",
                    move |_, bytes, _| {
                        callback_router.lock().unwrap().message(token, bytes, emit);
                    },
                    (),
                ) {
                    Ok(connection) => connections.push(Connection {
                        port,
                        token,
                        connection,
                    }),
                    Err(_) => router.lock().unwrap().remove(token, emit),
                }
            }
        }
        // Interruptible wait, not an audio callback timer. A live connection is preserved
        // across polls; failed/new endpoints are retried on the next inventory.
        match stopped.recv_timeout(Duration::from_millis(500)) {
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            _ => break,
        }
    }
    for old in connections {
        router.lock().unwrap().remove(old.token, emit);
        drop(old.connection.close());
    }
}

/// midir exposes PartialEq rather than Hash/Eq. On CoreMIDI a failed unique-ID lookup
/// makes a port unequal even to itself: do not repeatedly connect such an unidentified
/// endpoint. Duplicate inventory entries likewise must not create duplicate connections.
fn unique_ports<P: PartialEq>(ports: Vec<P>) -> Vec<P> {
    let mut unique = Vec::new();
    for port in ports {
        if port.eq(&port) && !unique.contains(&port) {
            unique.push(port);
        }
    }
    unique
}

#[derive(Clone, Copy)]
struct Channel {
    notes: [bool; 128],
    sustain: u8,
    soft: bool,
    sostenuto: bool,
}
impl Default for Channel {
    fn default() -> Self {
        Self {
            notes: [false; 128],
            sustain: 0,
            soft: false,
            sostenuto: false,
        }
    }
}
struct Endpoint {
    token: u64,
    channels: [Channel; 16],
}
#[derive(Default)]
struct Router {
    endpoints: Vec<Endpoint>,
    pedals: (u8, bool, bool),
}
impl Router {
    fn add(&mut self, token: u64) {
        self.endpoints.push(Endpoint {
            token,
            channels: [Channel::default(); 16],
        });
    }
    fn held(&self, note: usize) -> bool {
        self.endpoints
            .iter()
            .any(|e| e.channels.iter().any(|c| c.notes[note]))
    }
    fn message(&mut self, token: u64, bytes: &[u8], mut emit: impl FnMut(Input)) {
        if bytes.len() < 3 || bytes[1] > 127 || bytes[2] > 127 {
            return;
        }
        let Some(endpoint) = self.endpoints.iter_mut().find(|e| e.token == token) else {
            return;
        };
        let channel = &mut endpoint.channels[(bytes[0] & 0x0f) as usize];
        let (note, value) = (bytes[1] as usize, bytes[2]);
        match bytes[0] & 0xf0 {
            0x90 if value > 0 && (21..=108).contains(&note) => {
                channel.notes[note] = true;
                emit(Input::NoteOn {
                    midi: note as i32,
                    velocity: value as i32,
                    from: FROM_HOST,
                });
            }
            0x80 | 0x90 if (21..=108).contains(&note) => {
                let held = channel.notes[note];
                channel.notes[note] = false;
                if held && !self.held(note) {
                    emit(Input::NoteOff {
                        midi: note as i32,
                        from: FROM_HOST,
                        release: if value > 0 { value as f64 / 64.0 } else { 1.0 },
                    });
                }
            }
            0xb0 => {
                match bytes[1] {
                    64 => channel.sustain = value,
                    67 => channel.soft = value >= 64,
                    66 => channel.sostenuto = value >= 64,
                    _ => return,
                }
                self.publish_pedals(emit);
            }
            _ => {}
        }
    }
    fn remove(&mut self, token: u64, mut emit: impl FnMut(Input)) {
        let Some(i) = self.endpoints.iter().position(|e| e.token == token) else {
            return;
        };
        let old = self.endpoints.swap_remove(i);
        for note in 21..=108 {
            if old.channels.iter().any(|c| c.notes[note]) && !self.held(note) {
                emit(Input::NoteOff {
                    midi: note as i32,
                    from: FROM_HOST,
                    release: 1.0,
                });
            }
        }
        self.publish_pedals(emit);
    }
    fn publish_pedals(&mut self, mut emit: impl FnMut(Input)) {
        let pedals = self
            .endpoints
            .iter()
            .flat_map(|e| e.channels.iter())
            .fold((0, false, false), |p, c| {
                (p.0.max(c.sustain), p.1 || c.soft, p.2 || c.sostenuto)
            });
        if pedals.0 != self.pedals.0 {
            emit(Input::Sustain(pedals.0 as f32 / 127.0, FROM_HOST));
        }
        if pedals.1 != self.pedals.1 {
            emit(Input::Soft(pedals.1, FROM_HOST));
        }
        if pedals.2 != self.pedals.2 {
            emit(Input::Sostenuto(pedals.2, FROM_HOST));
        }
        self.pedals = pedals;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_deduplicates_identity_and_skips_unidentifiable_ports() {
        assert_eq!(unique_ports(vec![9, 3, 9, 3, 7]), vec![9, 3, 7]);
        assert_eq!(unique_ports(vec![f64::NAN, 2.0, 2.0]), vec![2.0]);
    }
    #[test]
    fn removal_preserves_other_endpoint_and_channel_holds_and_ignores_late_callbacks() {
        let mut router = Router::default();
        let mut events = Vec::new();
        router.add(1);
        router.add(2);
        for (token, msg) in [
            (1, [0x90, 60, 80]),
            (2, [0x90, 60, 90]),
            (2, [0x91, 60, 70]),
            (1, [0x90, 62, 80]),
        ] {
            router.message(token, &msg, |e| events.push(e));
        }
        events.clear();
        router.remove(1, |e| events.push(e));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Input::NoteOff { midi: 62, .. }));
        events.clear();
        router.message(1, &[0x90, 64, 127], |e| events.push(e));
        router.message(2, &[0x80, 60, 32], |e| events.push(e));
        assert!(events.is_empty()); // channel 1 still holds C
        router.message(2, &[0x91, 60, 0], |e| events.push(e));
        assert!(matches!(
            events.as_slice(),
            [Input::NoteOff { midi: 60, .. }]
        ));
        events.clear();
        router.add(3); // a reconnected endpoint has a fresh token/state
        router.message(3, &[0x90, 64, 80], |e| events.push(e));
        assert!(matches!(
            events.as_slice(),
            [Input::NoteOn { midi: 64, .. }]
        ));
    }
    #[test]
    fn disconnect_releases_only_its_pedal_contribution() {
        let mut router = Router::default();
        let mut events = Vec::new();
        router.add(1);
        router.add(2);
        for (token, msg) in [
            (1, [0xb0, 64, 127]),
            (1, [0xb0, 66, 127]),
            (2, [0xb0, 64, 80]),
            (2, [0xb1, 66, 127]),
            (2, [0xb0, 67, 127]),
        ] {
            router.message(token, &msg, |e| events.push(e));
        }
        events.clear();
        router.remove(1, |e| events.push(e));
        assert!(
            matches!(events.as_slice(),[Input::Sustain(v,FROM_HOST)] if (*v-80.0/127.0).abs()<1e-6)
        );
        events.clear();
        router.remove(2, |e| events.push(e));
        assert!(matches!(
            events.as_slice(),
            [
                Input::Sustain(0.0, FROM_HOST),
                Input::Soft(false, FROM_HOST),
                Input::Sostenuto(false, FROM_HOST)
            ]
        ));
    }
}
