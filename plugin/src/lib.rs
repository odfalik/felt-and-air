//! Shared CLAP/VST3 editor and standalone instrument. Rebuilds, geometry, snapshot allocation
//! and disposal happen off the audio thread. Audio only moves ownership through bounded queues.
mod demo;
mod editor;
mod paths;
mod presets;
pub mod shared;
#[cfg(test)]
mod tests;

use crossbeam::queue::ArrayQueue;
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use piano_engine::params::{Kind, PARAMS};
use piano_engine::{
    make_grand, midi_velocity_to_hammer_speed, GrandOptions, Piano, PianoOptions, Vec2,
};
use shared::{BoardView, Display, Input, Shared, FROM_DEMO, FROM_HOST};
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) enum Setting {
    Number(FloatParam),
    Switch(BoolParam),
}
pub(crate) struct PianoParams {
    volume: FloatParam,
    grid: IntParam,
    settings: Vec<Setting>,
    dirty: Arc<AtomicBool>,
    editor_state: Arc<EguiState>,
}
impl PianoParams {
    fn new() -> Self {
        let dirty = Arc::new(AtomicBool::new(false));
        let reference = GrandOptions::default();
        let settings = PARAMS
            .iter()
            .map(|p| {
                let flag = dirty.clone();
                let default = (p.get)(&reference);
                match p.kind {
                    Kind::Switch => Setting::Switch(
                        BoolParam::new(p.name, default >= 0.5)
                            .with_callback(Arc::new(move |_| flag.store(true, Ordering::Release))),
                    ),
                    _ => {
                        let range = if p.log {
                            FloatRange::Skewed {
                                min: p.lo as f32,
                                max: p.hi as f32,
                                factor: FloatRange::skew_factor(-1.5),
                            }
                        } else {
                            FloatRange::Linear {
                                min: p.lo as f32,
                                max: p.hi as f32,
                            }
                        };
                        Setting::Number(
                            FloatParam::new(p.name, default as f32, range).with_callback(Arc::new(
                                move |_| flag.store(true, Ordering::Release),
                            )),
                        )
                    }
                }
            })
            .collect();
        let flag = dirty.clone();
        Self {
            volume: FloatParam::new("Volume", 0.8, FloatRange::Linear { min: 0.0, max: 3.0 }),
            grid: IntParam::new("Board grid", 2, IntRange::Linear { min: 0, max: 3 })
                .with_callback(Arc::new(move |_| flag.store(true, Ordering::Release))),
            settings,
            dirty,
            editor_state: EguiState::from_size(1280, 860),
        }
    }
    fn options(&self) -> GrandOptions {
        let mut o = GrandOptions::default();
        for (p, s) in PARAMS.iter().zip(&self.settings) {
            p.write(
                &mut o,
                match s {
                    Setting::Number(f) => f.value() as f64,
                    Setting::Switch(b) => b.value() as u8 as f64,
                },
            );
        }
        o
    }
    fn grid_scale(&self) -> f64 {
        [1.0, 1.25, 1.5, 2.0][self.grid.value() as usize]
    }
}
unsafe impl Params for PianoParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let mut map = vec![
            ("volume".into(), self.volume.as_ptr(), String::new()),
            ("grid".into(), self.grid.as_ptr(), String::new()),
        ];
        for (i, (p, s)) in PARAMS.iter().zip(&self.settings).enumerate() {
            let ptr = match s {
                Setting::Number(f) => f.as_ptr(),
                Setting::Switch(b) => b.as_ptr(),
            };
            map.push((
                format!("setting{i}"),
                ptr,
                piano_engine::params::TAB_NAMES[p.tab].into(),
            ));
        }
        map
    }
    fn serialize_fields(&self) -> BTreeMap<String, String> {
        BTreeMap::from([(
            "editor".into(),
            serde_json::to_string(&*self.editor_state).unwrap_or_default(),
        )])
    }
    fn deserialize_fields(&self, fields: &BTreeMap<String, String>) {
        use nih_plug::params::persist::PersistentField;
        if let Some(state) = fields
            .get("editor")
            .and_then(|s| serde_json::from_str::<EguiState>(s).ok())
        {
            self.editor_state.set(state);
        }
    }
}

struct Built {
    piano: Piano,
    generation: u64,
    epoch: u64,
    sample_rate: f64,
    /// Prepared off-thread; moved into Shared only when this exact instrument is accepted.
    display: Option<Display>,
}
fn build(o: &GrandOptions, fs: f64, grid: f64, generation: u64, epoch: u64) -> Option<Built> {
    let d = make_grand(o);
    let mut piano = Piano::new(
        &d,
        &PianoOptions {
            sample_rate: fs,
            board_grid_scale: grid,
            ..PianoOptions::default()
        },
    )
    .ok()?;
    piano.set_helper_threads(3, None, None);
    let plate = piano.plate();
    let (width, height) = (224, 256);
    let min = Vec2 {
        x: d.board
            .outline
            .iter()
            .map(|p| p.x)
            .fold(f64::INFINITY, f64::min),
        y: d.board
            .outline
            .iter()
            .map(|p| p.y)
            .fold(f64::INFINITY, f64::min),
    };
    let max = Vec2 {
        x: d.board
            .outline
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max),
        y: d.board
            .outline
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max),
    };
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let it = plate.interp(Vec2 {
                x: min.x + (max.x - min.x) * (x as f64 + 0.5) / width as f64,
                y: max.y - (max.y - min.y) * (y as f64 + 0.5) / height as f64,
            });
            pixels.push((it.idx.map(|n| n as u32), it.w.map(|w| w as f32)));
        }
    }
    let view = BoardView {
        generation,
        outline: d.board.outline,
        bridges: d.board.bridges,
        string_bridge: (0..piano.string_count())
            .map(|s| piano.string_bridge(s))
            .collect(),
        min,
        max,
        width,
        height,
        pixels,
        nodes: plate.n,
        spacing: Vec2 {
            x: plate.hx,
            y: plate.hy,
        },
        strings: piano.string_count(),
        reverb_time: piano.reverb_time(),
    };
    let display = Some(Display {
        generation,
        snap: piano.new_snapshot(),
        view: Some(Arc::new(view)),
    });
    Some(Built {
        piano,
        generation,
        epoch,
        sample_rate: fs,
        display,
    })
}

#[derive(Clone, Copy, Default)]
struct Pedals {
    sustain: f32,
    soft: bool,
    sostenuto: bool,
}

pub struct PhysicalPiano {
    params: Arc<PianoParams>,
    shared: Arc<Shared>,
    built: Option<Built>,
    sample_rate: f64,
    fresh: Arc<ArrayQueue<Built>>,
    retired: Arc<ArrayQueue<Built>>,
    demos: Arc<Vec<demo::Demo>>,
    player: demo::Player,
    left: Vec<f32>,
    right: Vec<f32>,
    since_snapshot: usize,
    dirty_since: Option<Instant>,
    editor_was_open: bool,
    generation: u64,
    epoch: u64,
    pedals: [Pedals; 4],
    captured: [bool; 128],
}
impl Default for PhysicalPiano {
    fn default() -> Self {
        Self {
            params: Arc::new(PianoParams::new()),
            shared: Shared::new(),
            built: None,
            sample_rate: 48000.0,
            fresh: Arc::new(ArrayQueue::new(1)),
            retired: Arc::new(ArrayQueue::new(8)),
            demos: demo::demos(),
            player: demo::Player::default(),
            left: Vec::new(),
            right: Vec::new(),
            since_snapshot: 0,
            dirty_since: None,
            editor_was_open: false,
            generation: 0,
            epoch: 0,
            pedals: [Pedals::default(); 4],
            captured: [false; 128],
        }
    }
}
pub enum Task {
    Rebuild(GrandOptions, f64, f64, u64, u64),
    Collect,
}
impl PhysicalPiano {
    /// Called only by lifecycle callbacks, not process(). Serializes against build completion.
    fn invalidate_builds(&mut self) {
        let shared = self.shared.clone();
        let _guard = shared.lifecycle.lock().unwrap();
        self.epoch = shared.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        shared.building.store(false, Ordering::Release);
        while let Some(old) = self.fresh.pop() {
            drop(old);
        }
        while let Some(old) = self.retired.pop() {
            drop(old);
        }
        self.dirty_since = None;
    }
    fn initialize_engine(&mut self, sample_rate: f64, max_buffer: usize) -> bool {
        self.invalidate_builds();
        self.reset();
        self.sample_rate = sample_rate;
        self.shared.sample_rate.store(sample_rate as f32);
        self.left = vec![0.0; max_buffer];
        self.right = vec![0.0; max_buffer];
        self.params.dirty.store(false, Ordering::Release);
        self.generation += 1;
        let Some(mut new) = build(
            &self.params.options(),
            sample_rate,
            self.params.grid_scale(),
            self.generation,
            self.epoch,
        ) else {
            return false;
        };
        *self.shared.display.lock().unwrap() = new.display.take().unwrap();
        self.shared.display_frames.fetch_add(1, Ordering::Release);
        self.built = Some(new);
        self.since_snapshot = 0;
        true
    }
    /// The sole publication point for rebuilt audio + display. If the UI is copying a
    /// snapshot, leave the prepared build queued until another block. No waiting or freeing.
    fn accept_fresh(&mut self) -> bool {
        if self.built.is_none() || self.retired.is_full() || self.fresh.is_empty() {
            return false;
        }
        let shared = self.shared.clone();
        let Ok(mut display) = shared.display.try_lock() else {
            return false;
        };
        let Some(mut new) = self.fresh.pop() else {
            return false;
        };
        if new.epoch != self.epoch || new.sample_rate != self.sample_rate {
            let _ = self.retired.push(new);
            return true;
        }
        self.restore_performance(&mut new.piano);
        let old_display = std::mem::replace(&mut *display, new.display.take().unwrap());
        let mut old = self.built.replace(new).unwrap();
        debug_assert!(old.display.is_none());
        old.display = Some(old_display);
        let _ = self.retired.push(old);
        shared.display_frames.fetch_add(1, Ordering::Release);
        true
    }
    fn input(&mut self, input: Input) {
        if matches!(input, Input::PlayDemo(_) | Input::StopDemo) {
            self.clear_source(FROM_DEMO);
            match input {
                Input::PlayDemo(i) if i < self.demos.len() => self.player.start(i),
                _ => self.player.index = None,
            }
            self.shared.demo_playing.store(
                self.player.index.map_or(-1, |i| i as i32),
                Ordering::Relaxed,
            );
            return;
        }
        match input {
            Input::Sustain(v, source) => {
                if let Some(p) = self.pedals.get_mut(source.trailing_zeros() as usize) {
                    p.sustain = v.clamp(0.0, 1.0);
                }
                self.apply_pedals();
                return;
            }
            Input::Soft(v, source) => {
                if let Some(p) = self.pedals.get_mut(source.trailing_zeros() as usize) {
                    p.soft = v;
                }
                self.apply_pedals();
                return;
            }
            Input::Sostenuto(v, source) => {
                if let Some(p) = self.pedals.get_mut(source.trailing_zeros() as usize) {
                    p.sostenuto = v;
                }
                self.apply_pedals();
                return;
            }
            _ => {}
        }
        let Some(b) = &mut self.built else { return };
        match input {
            Input::NoteOn {
                midi,
                velocity,
                from,
            } if (21..=108).contains(&midi) => {
                self.shared.press(midi, from, true);
                self.shared.velocity[midi as usize].store(velocity as u8, Ordering::Relaxed);
                self.shared.focus.store(midi, Ordering::Relaxed);
                b.piano
                    .note_on(midi, midi_velocity_to_hammer_speed(velocity));
            }
            Input::NoteOff {
                midi,
                from,
                release,
            } if (21..=108).contains(&midi) => {
                self.shared.press(midi, from, false);
                if self.shared.held[midi as usize].load(Ordering::Relaxed) == 0 {
                    b.piano.note_off(midi, release);
                }
            }
            _ => {}
        }
    }
    fn clear_source(&mut self, from: u8) {
        for midi in 21..=108 {
            self.input(Input::NoteOff {
                midi,
                from,
                release: 1.0,
            });
        }
        for (i, pedals) in self.pedals.iter_mut().enumerate() {
            if from & (1 << i) != 0 {
                *pedals = Pedals::default();
            }
        }
        self.apply_pedals();
    }
    /// Sustain is the maximum position requested by any source; binary pedals are ORed.
    /// Sostenuto captures only at the combined up→down transition, like one physical pedal.
    fn apply_pedals(&mut self) {
        let sustain = self.pedals.iter().fold(0.0f32, |m, p| m.max(p.sustain));
        let soft = self.pedals.iter().any(|p| p.soft);
        let sostenuto = self.pedals.iter().any(|p| p.sostenuto);
        let was_sostenuto = self.shared.sostenuto.load(Ordering::Relaxed);
        if sostenuto && !was_sostenuto {
            self.captured =
                std::array::from_fn(|m| self.shared.held[m].load(Ordering::Relaxed) != 0);
        } else if !sostenuto {
            self.captured.fill(false);
        }
        self.shared
            .sustain
            .store((sustain * 127.0).round() as u8, Ordering::Relaxed);
        self.shared.soft.store(soft, Ordering::Relaxed);
        self.shared.sostenuto.store(sostenuto, Ordering::Relaxed);
        if let Some(b) = &mut self.built {
            b.piano.set_pedal(sustain as f64);
            b.piano.set_soft_pedal(soft);
            // The engine's setter captures keys on every true call, not just an edge.
            if sostenuto != was_sostenuto {
                b.piano.set_sostenuto(sostenuto);
            }
        }
    }
    fn restore_performance(&self, piano: &mut Piano) {
        piano.set_pedal(self.pedals.iter().fold(0.0f32, |m, p| m.max(p.sustain)) as f64);
        piano.set_soft_pedal(self.shared.soft.load(Ordering::Relaxed));
        let speed = |m: usize| {
            midi_velocity_to_hammer_speed(self.shared.velocity[m].load(Ordering::Relaxed) as i32)
        };
        // Recreate the exact capture set before lowering the sostenuto pedal. A captured
        // key released before rebuilding must ring; a later key must not become captured.
        for m in 21..=108 {
            if self.captured[m] {
                piano.note_on(m as i32, speed(m));
            }
        }
        piano.set_sostenuto(self.shared.sostenuto.load(Ordering::Relaxed));
        for m in 21..=108 {
            let held = self.shared.held[m].load(Ordering::Relaxed) != 0;
            if held && !self.captured[m] {
                piano.note_on(m as i32, speed(m));
            }
            if !held && self.captured[m] {
                piano.note_off(m as i32, 1.0);
            }
        }
    }
    fn demo_event(&mut self, e: piano_engine::midi::MidiEvent) {
        use piano_engine::midi::MidiKind::*;
        self.input(match e.kind {
            NoteOn => Input::NoteOn {
                midi: e.a,
                velocity: e.b,
                from: FROM_DEMO,
            },
            NoteOff => Input::NoteOff {
                midi: e.a,
                from: FROM_DEMO,
                release: if e.b > 0 { e.b as f64 / 64.0 } else { 1.0 },
            },
            Sustain => Input::Sustain(e.a as f32 / 127.0, FROM_DEMO),
            Soft => Input::Soft(e.a >= 64, FROM_DEMO),
            Sostenuto => Input::Sostenuto(e.a >= 64, FROM_DEMO),
        });
    }
}
impl Plugin for PhysicalPiano {
    const NAME: &'static str = "Felt & Air";
    const VENDOR: &'static str = "odfalik";
    const URL: &'static str = "https://github.com/odfalik/physical-piano";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;
    type SysExMessage = ();
    type BackgroundTask = Task;
    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }
    fn editor(&mut self, _: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone(), self.demos.clone())
    }
    fn task_executor(&mut self) -> TaskExecutor<Self> {
        let (fresh, retired, shared) = (
            self.fresh.clone(),
            self.retired.clone(),
            self.shared.clone(),
        );
        Box::new(move |task| {
            while let Some(old) = retired.pop() {
                drop(old);
            }
            if let Task::Rebuild(o, fs, grid, generation, epoch) = task {
                let result = build(&o, fs, grid, generation, epoch);
                let _guard = shared.lifecycle.lock().unwrap();
                if shared.epoch.load(Ordering::Acquire) == epoch {
                    if let Some(b) = result {
                        let _ = fresh.push(b);
                    }
                    shared.building.store(false, Ordering::Release);
                }
                // Obsolete builds are destroyed here on the background worker. They never
                // publish geometry, reset a new lifecycle's building flag, or reach audio.
            }
        })
    }
    fn initialize(
        &mut self,
        _: &AudioIOLayout,
        c: &BufferConfig,
        _: &mut impl InitContext<Self>,
    ) -> bool {
        self.initialize_engine(c.sample_rate as f64, c.max_buffer_size as usize)
    }
    fn deactivate(&mut self) {
        self.invalidate_builds();
    }
    fn reset(&mut self) {
        self.clear_source(255);
        self.player = demo::Player::default();
        self.shared.demo_playing.store(-1, Ordering::Relaxed);
        self.shared.demo_seconds.store(0.0);
    }
    fn process(
        &mut self,
        buffer: &mut Buffer,
        _: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let t = Instant::now();
        let editor_open = self.params.editor_state.is_open();
        if self.editor_was_open && !editor_open {
            self.clear_source(shared::FROM_UI | shared::FROM_MOUSE);
        }
        self.editor_was_open = editor_open;
        if self.accept_fresh() {
            context.execute_background(Task::Collect);
        }
        if self.params.dirty.swap(false, Ordering::AcqRel) {
            self.dirty_since = Some(t);
        }
        if self
            .dirty_since
            .is_some_and(|s| t.duration_since(s) >= Duration::from_millis(180))
            && self.fresh.is_empty()
            && !self.shared.building.swap(true, Ordering::AcqRel)
        {
            self.dirty_since = None;
            self.generation += 1;
            context.execute_background(Task::Rebuild(
                self.params.options(),
                self.sample_rate,
                self.params.grid_scale(),
                self.generation,
                self.epoch,
            ));
        }
        if self.shared.panic.swap(false, Ordering::AcqRel) {
            for _ in 0..self.shared.inputs.len() {
                let _ = self.shared.inputs.pop();
            }
            self.clear_source(255);
        }
        // Snapshot queue lengths and cap normal work: producers cannot extend this block.
        for _ in 0..self.shared.inputs.len().min(128) {
            if let Some(e) = self.shared.inputs.pop() {
                self.input(e);
            }
        }
        if shared::STANDALONE.load(Ordering::Relaxed) {
            if shared::EXTERNAL_PANIC.swap(false, Ordering::AcqRel) {
                for _ in 0..shared::EXTERNAL_MIDI.len() {
                    let _ = shared::EXTERNAL_MIDI.pop();
                }
                self.clear_source(FROM_HOST);
            }
            for _ in 0..shared::EXTERNAL_MIDI.len().min(128) {
                if let Some(e) = shared::EXTERNAL_MIDI.pop() {
                    self.input(e);
                }
            }
        }
        let frames = buffer.samples().min(self.left.len());
        let mut done = 0;
        let mut next = context.next_event();
        while done < frames {
            let mut until = frames;
            while let Some(event) = next {
                if event.timing() as usize > done {
                    until = (event.timing() as usize).min(frames);
                    break;
                }
                let input = match event {
                    NoteEvent::NoteOn { note, velocity, .. } => Some(Input::NoteOn {
                        midi: note as i32,
                        velocity: (velocity * 127.0).round() as i32,
                        from: FROM_HOST,
                    }),
                    NoteEvent::NoteOff { note, velocity, .. } => Some(Input::NoteOff {
                        midi: note as i32,
                        from: FROM_HOST,
                        release: if velocity > 0.0 {
                            velocity as f64 * 2.0
                        } else {
                            1.0
                        },
                    }),
                    NoteEvent::MidiCC { cc: 64, value, .. } => {
                        Some(Input::Sustain(value, FROM_HOST))
                    }
                    NoteEvent::MidiCC { cc: 67, value, .. } => {
                        Some(Input::Soft(value >= 0.5, FROM_HOST))
                    }
                    NoteEvent::MidiCC { cc: 66, value, .. } => {
                        Some(Input::Sostenuto(value >= 0.5, FROM_HOST))
                    }
                    _ => None,
                };
                if let Some(i) = input {
                    self.input(i);
                }
                next = context.next_event();
            }
            if let Some(i) = self.player.index {
                while self.player.next < self.demos[i].events.len()
                    && (self.demos[i].events[self.player.next].seconds * self.sample_rate) as u64
                        <= self.player.frame
                {
                    let e = self.demos[i].events[self.player.next];
                    self.player.next += 1;
                    self.demo_event(e);
                }
                if let Some(e) = self.demos[i].events.get(self.player.next) {
                    until = until.min(
                        done + ((e.seconds * self.sample_rate) as u64)
                            .saturating_sub(self.player.frame)
                            as usize,
                    );
                } else if self.player.frame as f64
                    > (self.demos[i].seconds + 4.0) * self.sample_rate
                {
                    self.input(Input::StopDemo);
                }
            }
            if let Some(b) = &mut self.built {
                b.piano
                    .process(&mut self.left[done..until], &mut self.right[done..until]);
            }
            if self.player.index.is_some() {
                self.player.frame += (until - done) as u64;
            }
            done = until;
        }
        self.shared
            .demo_seconds
            .store((self.player.frame as f64 / self.sample_rate) as f32);
        let volume = self.params.volume.value();
        let sound = self.shared.sound_on.load(Ordering::Relaxed);
        let out = buffer.as_slice();
        let mut peak = 0.0f32;
        for i in 0..frames {
            let l = (self.left[i] * volume).tanh();
            let r = (self.right[i] * volume).tanh();
            peak = peak.max(l.abs()).max(r.abs());
            out[0][i] = if sound { l } else { 0.0 };
            out[1][i] = if sound { r } else { 0.0 };
        }
        self.shared
            .output_peak
            .store(peak.max(self.shared.output_peak.load() * 0.9));
        self.since_snapshot += frames;
        if self.since_snapshot >= (self.sample_rate / 30.0) as usize
            && self.params.editor_state.is_open()
        {
            if let (Some(b), Ok(mut display)) = (&self.built, self.shared.display.try_lock()) {
                if display.generation == b.generation {
                    b.piano.fill_snapshot(
                        &mut display.snap,
                        self.shared.focus.load(Ordering::Relaxed),
                    );
                    self.shared.display_frames.fetch_add(1, Ordering::Release);
                    self.since_snapshot = 0;
                }
            }
        }
        let load = t.elapsed().as_secs_f32() / (frames.max(1) as f32 / self.sample_rate as f32);
        self.shared
            .load
            .store(self.shared.load.load() * 0.95 + load * 0.05);
        self.shared
            .load_peak
            .store(self.shared.load_peak.load().max(load));
        if load > 1.0 {
            self.shared.over_budget.fetch_add(1, Ordering::Relaxed);
        }
        ProcessStatus::Tail((8.0 * self.sample_rate) as u32)
    }
}
impl ClapPlugin for PhysicalPiano {
    const CLAP_ID: &'static str = "com.odfalik.felt-and-air";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A piano simulated from its mechanics");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::Instrument,
        ClapFeature::Synthesizer,
        ClapFeature::Stereo,
    ];
}
nih_export_clap!(PhysicalPiano);
#[cfg(feature = "vst3")]
impl Vst3Plugin for PhysicalPiano {
    const VST3_CLASS_ID: [u8; 16] = *b"OdfkFeltAirV0001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Instrument, Vst3SubCategory::Piano];
}
#[cfg(feature = "vst3")]
nih_export_vst3!(PhysicalPiano);
