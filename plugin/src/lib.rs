//! The instrument as a plug-in, through nih-plug. Every setting of the engine's parameter
//! table is a host parameter. Changing one means building a new instrument, which takes a
//! fraction of a second, so that happens on a background task and the audio thread only
//! swaps the result in.

use nih_plug::prelude::*;
use piano_engine::params::{Kind, PARAMS};
use piano_engine::{make_grand, midi_velocity_to_hammer_speed, GrandOptions, Piano, PianoOptions};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

enum Setting {
    Number(FloatParam),
    Switch(BoolParam),
}

struct PianoParams {
    volume: FloatParam,
    settings: Vec<Setting>,
    /// Set by any change; the audio thread turns it into one rebuild.
    dirty: Arc<AtomicBool>,
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
                    Kind::Switch => Setting::Switch(BoolParam::new(p.name, default >= 0.5).with_callback(Arc::new(move |_| flag.store(true, Ordering::Release)))),
                    _ => {
                        let range = if p.log {
                            FloatRange::Skewed { min: p.lo as f32, max: p.hi as f32, factor: FloatRange::skew_factor(-1.5) }
                        } else {
                            FloatRange::Linear { min: p.lo as f32, max: p.hi as f32 }
                        };
                        Setting::Number(FloatParam::new(p.name, default as f32, range).with_callback(Arc::new(move |_| flag.store(true, Ordering::Release))))
                    }
                }
            })
            .collect();
        PianoParams { volume: FloatParam::new("Volume", 0.8, FloatRange::Linear { min: 0.0, max: 3.0 }), settings, dirty }
    }

    fn options(&self) -> GrandOptions {
        let mut o = GrandOptions::default();
        for (p, s) in PARAMS.iter().zip(&self.settings) {
            let v = match s {
                Setting::Number(f) => f.value() as f64,
                Setting::Switch(b) => {
                    if b.value() {
                        1.0
                    } else {
                        0.0
                    }
                }
            };
            p.write(&mut o, v);
        }
        o
    }
}

// The parameters come from a table, so the map is written out by hand instead of derived.
unsafe impl Params for PianoParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let mut map = vec![("volume".to_string(), self.volume.as_ptr(), String::new())];
        for (i, (p, s)) in PARAMS.iter().zip(&self.settings).enumerate() {
            let ptr = match s {
                Setting::Number(f) => f.as_ptr(),
                Setting::Switch(b) => b.as_ptr(),
            };
            map.push((format!("setting{i}"), ptr, piano_engine::params::TAB_NAMES[p.tab].to_string()));
        }
        map
    }
}

fn build(options: &GrandOptions, sample_rate: f64) -> Option<Piano> {
    let mut piano = Piano::new(&make_grand(options), &PianoOptions { sample_rate, board_grid_scale: 1.5, ..PianoOptions::default() }).ok()?;
    piano.set_helper_threads(3, None, None);
    Some(piano)
}

pub struct PhysicalPiano {
    params: Arc<PianoParams>,
    piano: Option<Piano>,
    sample_rate: f64,
    /// A rebuilt instrument waiting to be swapped in, and the old one waiting to be freed.
    fresh: Arc<Mutex<Option<Piano>>>,
    stale: Arc<Mutex<Option<Piano>>>,
    building: Arc<AtomicBool>,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Default for PhysicalPiano {
    fn default() -> Self {
        PhysicalPiano {
            params: Arc::new(PianoParams::new()),
            piano: None,
            sample_rate: 48000.0,
            fresh: Arc::new(Mutex::new(None)),
            stale: Arc::new(Mutex::new(None)),
            building: Arc::new(AtomicBool::new(false)),
            left: Vec::new(),
            right: Vec::new(),
        }
    }
}

pub struct Rebuild(GrandOptions, f64);

impl Plugin for PhysicalPiano {
    const NAME: &'static str = "Physical Piano";
    const VENDOR: &'static str = "odfalik";
    const URL: &'static str = "https://github.com/odfalik/physical-piano";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout { main_input_channels: None, main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() }];
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;
    type SysExMessage = ();
    type BackgroundTask = Rebuild;

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        let (fresh, stale, building) = (self.fresh.clone(), self.stale.clone(), self.building.clone());
        Box::new(move |Rebuild(options, sample_rate)| {
            drop(stale.lock().unwrap().take());
            let piano = build(&options, sample_rate);
            *fresh.lock().unwrap() = piano;
            building.store(false, Ordering::Release);
        })
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _context: &mut impl InitContext<Self>) -> bool {
        self.sample_rate = config.sample_rate as f64;
        self.left = vec![0.0; config.max_buffer_size as usize];
        self.right = vec![0.0; config.max_buffer_size as usize];
        self.params.dirty.store(false, Ordering::Release);
        self.piano = build(&self.params.options(), self.sample_rate);
        self.piano.is_some()
    }

    fn reset(&mut self) {
        if let Some(p) = &mut self.piano {
            (21..=108).for_each(|m| p.note_off(m, 1.0));
            p.set_pedal(0.0);
        }
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        // A rebuilt instrument is ready: swap it in, and leave the old one for the background
        // task to free.
        if let Ok(mut fresh) = self.fresh.try_lock() {
            if let Some(new) = fresh.take() {
                if let Ok(mut stale) = self.stale.try_lock() {
                    *stale = self.piano.replace(new);
                } else {
                    *fresh = Some(new);
                }
            }
        }
        if self.params.dirty.load(Ordering::Acquire) && !self.building.swap(true, Ordering::AcqRel) {
            self.params.dirty.store(false, Ordering::Release);
            context.execute_background(Rebuild(self.params.options(), self.sample_rate));
        }
        let Some(piano) = &mut self.piano else { return ProcessStatus::Normal };
        let frames = buffer.samples().min(self.left.len());
        // Render in pieces so that each event lands on its own sample.
        let mut done = 0;
        let mut next = context.next_event();
        while done < frames {
            let mut until = frames;
            while let Some(event) = next {
                if (event.timing() as usize) > done {
                    until = (event.timing() as usize).min(frames);
                    break;
                }
                match event {
                    NoteEvent::NoteOn { note, velocity, .. } => piano.note_on(note as i32, midi_velocity_to_hammer_speed((velocity * 127.0).round() as i32)),
                    NoteEvent::NoteOff { note, velocity, .. } => piano.note_off(note as i32, if velocity > 0.0 { velocity as f64 * 2.0 } else { 1.0 }),
                    NoteEvent::MidiCC { cc: 64, value, .. } => piano.set_pedal(value as f64),
                    NoteEvent::MidiCC { cc: 67, value, .. } => piano.set_soft_pedal(value >= 0.5),
                    NoteEvent::MidiCC { cc: 66, value, .. } => piano.set_sostenuto(value >= 0.5),
                    _ => {}
                }
                next = context.next_event();
            }
            piano.process(&mut self.left[done..until], &mut self.right[done..until]);
            done = until;
        }
        let volume = self.params.volume.value();
        let out = buffer.as_slice();
        for i in 0..frames {
            out[0][i] = (self.left[i] * volume).tanh(); // soft ceiling
            out[1][i] = (self.right[i] * volume).tanh();
        }
        ProcessStatus::Tail((8.0 * self.sample_rate) as u32)
    }
}

impl ClapPlugin for PhysicalPiano {
    const CLAP_ID: &'static str = "com.odfalik.physical-piano";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A piano simulated from its mechanics");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Synthesizer, ClapFeature::Stereo];
}
nih_export_clap!(PhysicalPiano);

#[cfg(feature = "vst3")]
impl Vst3Plugin for PhysicalPiano {
    const VST3_CLASS_ID: [u8; 16] = *b"OdfkPhysPianoV01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Piano];
}
#[cfg(feature = "vst3")]
nih_export_vst3!(PhysicalPiano);
