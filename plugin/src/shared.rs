//! What the audio thread and the editor share. The audio thread never waits here: it uses
//! atomics, a fixed-size lock-free queue and `try_lock`, and skips a display update rather
//! than block.

use crossbeam::queue::ArrayQueue;
use piano_engine::design::Vec2;
use piano_engine::Snapshot;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Where a held key came from, as bits in `Shared::held`.
pub const FROM_HOST: u8 = 1;
pub const FROM_UI: u8 = 2;
pub const FROM_DEMO: u8 = 4;
pub const FROM_MOUSE: u8 = 8;

/// What the editor (or the standalone's MIDI listener) asks the audio thread to do.
#[derive(Clone, Copy, Debug)]
pub enum Input {
    NoteOn {
        midi: i32,
        velocity: i32,
        from: u8,
    },
    NoteOff {
        midi: i32,
        from: u8,
        release: f64,
    },
    /// 0..1, continuous.
    Sustain(f32, u8),
    Soft(bool, u8),
    Sostenuto(bool, u8),
    PlayDemo(usize),
    StopDemo,
}

/// MIDI from every source the standalone app connects to. Plug-ins get MIDI from their host
/// instead, so only the standalone fills this.
pub static EXTERNAL_MIDI: LazyLock<ArrayQueue<Input>> = LazyLock::new(|| ArrayQueue::new(1024));
/// Set by the standalone's `main` before the wrapper starts.
pub static STANDALONE: AtomicBool = AtomicBool::new(false);
pub static EXTERNAL_PANIC: AtomicBool = AtomicBool::new(false);

/// A float in an atomic, for meters.
#[derive(Default)]
pub struct AtomicF32(AtomicU32);
impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        AtomicF32(AtomicU32::new(v.to_bits()))
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
}

/// The instrument's geometry as the editor draws it, built with each instrument on the
/// background task so that it always matches the snapshot it is drawn with.
pub struct BoardView {
    pub generation: u64,
    pub outline: Vec<Vec2>,
    pub bridges: Vec<Vec<Vec2>>,
    pub string_bridge: Vec<Vec2>,
    /// Lower-left and upper-right corner of the board, piano coordinates, metres.
    pub min: Vec2,
    pub max: Vec2,
    /// Heat map: image size, and per pixel four grid storage indices with weights (weights
    /// all zero off the board).
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<([u32; 4], [f32; 4])>,
    pub nodes: usize,
    pub spacing: Vec2,
    pub strings: usize,
    pub reverb_time: f64,
}

/// A display snapshot labelled with the instrument it came from.
pub struct Display {
    pub generation: u64,
    pub snap: Snapshot,
    pub view: Option<Arc<BoardView>>,
}

pub struct Shared {
    pub inputs: ArrayQueue<Input>,
    /// Per MIDI note, the sources holding it down.
    pub held: [AtomicU8; 128],
    pub velocity: [AtomicU8; 128],
    pub panic: AtomicBool,
    /// Pedals as the audio thread last applied them: sustain 0..127, soft and sostenuto 0/1.
    pub sustain: AtomicU8,
    pub soft: AtomicBool,
    pub sostenuto: AtomicBool,
    /// Filled by the audio thread about 30 times a second.
    pub display: Mutex<Display>,
    pub display_frames: AtomicU64,
    pub epoch: AtomicU64,
    /// Serializes lifecycle invalidation and publication of completed builds, never audio.
    pub lifecycle: Mutex<()>,
    pub focus: AtomicI32,
    /// Share of each block's deadline used, smoothed and peak; blocks over budget.
    pub load: AtomicF32,
    pub load_peak: AtomicF32,
    pub over_budget: AtomicU32,
    pub building: AtomicBool,
    pub sample_rate: AtomicF32,
    pub output_peak: AtomicF32,
    pub demo_playing: AtomicI32,
    pub demo_seconds: AtomicF32,
}

impl Shared {
    pub fn new() -> Arc<Shared> {
        Arc::new(Shared {
            inputs: ArrayQueue::new(1024),
            held: std::array::from_fn(|_| AtomicU8::new(0)),
            velocity: std::array::from_fn(|_| AtomicU8::new(80)),
            panic: AtomicBool::new(false),
            sustain: AtomicU8::new(0),
            soft: AtomicBool::new(false),
            sostenuto: AtomicBool::new(false),
            display: Mutex::new(Display {
                generation: 0,
                snap: Snapshot::default(),
                view: None,
            }),
            display_frames: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            lifecycle: Mutex::new(()),
            focus: AtomicI32::new(-1),
            load: AtomicF32::new(0.0),
            load_peak: AtomicF32::new(0.0),
            over_budget: AtomicU32::new(0),
            building: AtomicBool::new(false),
            sample_rate: AtomicF32::new(48000.0),
            output_peak: AtomicF32::new(0.0),
            demo_playing: AtomicI32::new(-1),
            demo_seconds: AtomicF32::new(0.0),
        })
    }

    /// Queues an input; a full queue drops it rather than block.
    pub fn send(&self, input: Input) {
        if self.inputs.push(input).is_err() {
            self.panic.store(true, Ordering::Release);
        }
    }

    pub fn press(&self, midi: i32, from: u8, down: bool) {
        if !(0..128).contains(&midi) {
            return;
        }
        let bit = &self.held[midi as usize];
        if down {
            bit.fetch_or(from, Ordering::Relaxed);
        } else {
            bit.fetch_and(!from, Ordering::Relaxed);
        }
    }
}
