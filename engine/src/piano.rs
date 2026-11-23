//! The coupled instrument: stiff strings, felt hammers, action, dampers, soundboard, air.
//!
//! All parts form one discrete mechanical system  M q'' + C q' + K q = f  with a symmetric K,
//! stepped by a leapfrog scheme. The discrete energy returned by `energy()` is conserved to
//! rounding error when all losses are switched off, and can only decrease otherwise.
//! docs/MODEL.md has the derivation.
//!
//! Threads. Within one time step every string depends only on the state of the previous step,
//! so strings, the plate stencil and the previous step's radiation can be advanced side by
//! side. The audio thread posts a job, takes chunks itself, and helpers take the rest; a late
//! helper costs time, never correctness. State touched during a job lives in `Shared` and is
//! reached through raw buffers, each chunk keeping to its own part.

mod action_observer;
pub use action_observer::{ActionContact, ActionPhase, ActionPose, ActionPoseRecord, ActionReturnOrigin, ActionStroke, ActionTrace};
use action_observer::{ActionObserver, FORCE_OBSERVED, KEY_DOWN, SHANK_OBSERVED};

use crate::acoustics::Radiation;
use crate::action_contact::{felt, RAIL_REFERENCE, STOP_REFERENCE};
use crate::design::*;
use crate::plate::{Attachment, Coupling, Interp, Plate};
use std::cell::UnsafeCell;
use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

pub struct PianoOptions {
    pub sample_rate: f64,
    /// Internal steps per output sample.
    pub oversample: usize,
    /// Full scale per pascal.
    pub output_gain: f64,
    /// Soundboard grid spacing relative to the finest stable one. 1 is the finest; larger
    /// values trade spatial resolution for processor time (node count falls as 1/scale^2).
    pub board_grid_scale: f64,
}

impl Default for PianoOptions {
    fn default() -> Self {
        PianoOptions { sample_rate: 48000.0, oversample: 1, output_gain: 0.06, board_grid_scale: 1.0 }
    }
}

const LONG: usize = 24;

#[derive(Clone)]
struct StringRt {
    note: usize,
    n: usize,
    /// Index of node 0 in the string arrays; a ghost node sits on each side of nodes 0..=n.
    off: usize,
    h: f64,
    a_t: f64,
    a_b: f64,
    a_d: f64,
    s0k: f64,
    m_node: f64,
    z0: f64,
    tension: f64,
    // Stencil coefficients of the string update.
    cu0: f64,
    cu1: f64,
    cu2: f64,
    cv0: f64,
    cv1: f64,
    // Energy coefficients.
    t_h: f64,
    ei_h3: f64,
    damp_h: f64,
    attached: bool,
    awake: bool,
    /// Active hammers must correct the next increment before displacement advances.
    defer_advance: bool,
    has_damper: bool,
    /// The second polarisation of another string.
    sideways: bool,
    /// Share of the hammer direction seen by this polarisation.
    ham_w: f64,
    bridge: Vec2,
    ham_l: usize,
    ham_g: [f64; 2],
    ham_a: f64,
    dmp0: usize,
    dmp1: usize,
    damp_rate: f64,
    damp_target: f64,
    /// Steps spent fully damped.
    damped_steps: usize,
    /// Existing grounded-contact compression-rate proxy, relative to ordinary.
    damp_land: f64,
    /// dt^2 * resting damper force per node / node mass, at full engagement.
    press_k: f64,
    /// Recent size of the bridge motion at this string.
    end_peak: f64,
    /// Partial used to fit the grid dispersion.
    fit_partial: usize,
    f0: f64,
    inharm: f64,
    zeta: f64,
    zeta_prev: f64,
    force: f64,
    // Longitudinal motion on its own coarse grid (its waves are about 15 times faster).
    l_cells: usize,
    l_span: usize,
    /// Only strings that have been struck stretch enough to matter.
    l_on: bool,
    l_a: f64,
    l_damp: f64,
    l_ea: f64,
    l_bridge: f64,
    l_peak: f64,
    l_off: usize,
    /// At the bridge, N.
    tension_change: f64,
}

#[derive(Clone)]
struct NoteRt {
    midi: i32,
    first: usize,
    /// Hammer contacts; each may have a second polarisation.
    count: usize,
    /// Strings owned, both polarisations.
    total: usize,
    /// String first+count+i is the sideways motion of first+i.
    twin: bool,
    h_mass: f64,
    h_k: f64,
    h_p: f64,
    h_mu: f64,
    eta: f64,
    veta: f64,
    // Shank: mass behind the spring, stiffness, damping, position, increment.
    s_mass: f64,
    s_k: f64,
    s_c: f64,
    xi: f64,
    vxi: f64,
    hammer_active: bool,
    key_down: bool,
    /// Damper held up by the sostenuto pedal.
    sost: bool,
    // Key front: 0 at rest, positive down. Phase 0 at rest, 1 going down, 2 held, 3 coming up.
    key_phase: u8,
    ky: f64,
    kv: f64,
    k_force: f64,
    k_slow: f64,
    // Hammer on its way back: distance below the string and speed downward.
    returning: bool,
    checked: bool,
    hy: f64,
    hv: f64,
    h_slow: f64,
    h_total: f64,
    thump: f64,
    thump_at: Interp,
}

/// A buffer several threads work on at once, each on its own part.
struct Buf<T> {
    _own: Box<[T]>,
    ptr: *mut T,
    len: usize,
}
unsafe impl<T: Send> Send for Buf<T> {}
unsafe impl<T: Send> Sync for Buf<T> {}

impl<T> Buf<T> {
    fn from_vec(v: Vec<T>) -> Buf<T> {
        let mut own = v.into_boxed_slice();
        let ptr = own.as_mut_ptr();
        let len = own.len();
        Buf { _own: own, ptr, len }
    }
    fn ptr(&self) -> *mut T {
        self.ptr
    }
    /// # Safety
    /// No other thread may be writing the buffer.
    unsafe fn all(&self) -> &[T] {
        std::slice::from_raw_parts(self.ptr, self.len)
    }
    /// # Safety
    /// No other thread may be touching the buffer.
    #[allow(clippy::mut_from_ref)]
    unsafe fn all_mut(&self) -> &mut [T] {
        std::slice::from_raw_parts_mut(self.ptr, self.len)
    }
    /// # Safety
    /// No other thread may be touching element `i`.
    #[allow(clippy::mut_from_ref)]
    unsafe fn at(&self, i: usize) -> &mut T {
        debug_assert!(i < self.len);
        &mut *self.ptr.add(i)
    }
}

struct Cell<T>(UnsafeCell<T>);
unsafe impl<T: Send> Sync for Cell<T> {}
impl<T> Cell<T> {
    fn new(v: T) -> Self {
        Cell(UnsafeCell::new(v))
    }
    /// # Safety
    /// No other thread may be touching the value.
    #[allow(clippy::mut_from_ref)]
    unsafe fn get(&self) -> &mut T {
        &mut *self.0.get()
    }
}

/// Units of work that threads can take independently.
#[derive(Clone, Copy)]
enum Chunk {
    Radiation,
    PlateSpans(usize, usize),
    Strings(usize, usize),
}

/// Everything that is touched while a job is posted.
struct Shared {
    k: f64,
    damp_max: f64,
    plate: Plate,
    strings: Buf<StringRt>,
    /// All string nodes. Each string owns N+3 entries: a ghost node, nodes 0..N, a ghost node.
    u: Buf<f64>,
    /// The per-step increments, double buffered (`vb` selects the current one).
    v: [Buf<f64>; 2],
    vb: AtomicI32,
    /// Longitudinal displacement and increment, LONG + 1 per stretching string.
    xi: Buf<f64>,
    vxi: Buf<f64>,
    // What passes between strings and board each step, kept in dense arrays of their own.
    // Threads working on strings and the thread working on the board then exchange a handful
    // of cache lines instead of one or two per string.
    end_live: Buf<bool>,
    /// How each string's end follows the board (never changes).
    end_at: Vec<Coupling>,
    end_force: Buf<f64>,
    // Plate displacement, increment, right hand side (grid storage).
    w: Buf<f64>,
    vw: Buf<f64>,
    r: Buf<f64>,
    /// Radiation does not act back on the instrument, so it runs one step behind, alongside
    /// the next step's string work: the sound of step n is computed during step n+1.
    rad_in: Buf<f64>,
    radiation: Cell<Radiation>,
    rad_out: Cell<(f64, f64)>,
    /// Air reaction per patch being applied this step.
    air_now: Buf<f64>,
    chunks: Vec<Chunk>,

    // One word holds the whole job: generation, inactive flag and next unclaimed chunk.
    // Compare-and-swap prevents a stale claim from consuming a new job's chunk.
    ticket: AtomicU64,
    done: AtomicI32,
    stop: AtomicBool,
    active: AtomicBool,
    /// Helpers spin only while a block is being rendered.
    in_block: AtomicBool,
    park: Mutex<()>,
    wake: Condvar,
}

const NO_JOB: u64 = 3 << 32;

impl Shared {
    fn claim(&self) -> Option<usize> {
        let mut t = self.ticket.load(Ordering::Acquire);
        loop {
            let idx = (t & 0xffff_ffff) as usize;
            if t & NO_JOB != 0 || idx >= self.chunks.len() {
                return None;
            }
            match self.ticket.compare_exchange_weak(t, t + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(idx),
                Err(now) => t = now,
            }
        }
    }

    /// Compute forces and advance strings not awaiting hammer contact. The bridge sees the
    /// saved end force, not the updated string displacement, so no second barrier is needed.
    ///
    /// # Safety
    /// Must be called once per (job, index); chunks touch disjoint state.
    unsafe fn chunk_work(&self, index: usize) {
        match self.chunks[index] {
            Chunk::Radiation => {
                *self.rad_out.get() = self.radiation.get().step(self.rad_in.all());
            }
            Chunk::PlateSpans(from, to) => {
                let p = &self.plate;
                p.apply_stencil(self.w.ptr(), self.vw.ptr(), self.r.ptr(), from, to);
                p.apply_local(self.w.ptr(), self.r.ptr(), from, to);
                // Reaction of the air, as worked out by the radiation task one step ago.
                if self.air_now.len > 0 {
                    let r = self.r.ptr();
                    let patch = self.radiation_patches();
                    let air = self.air_now.ptr();
                    for sp in &p.spans[from..to] {
                        for t in sp.start..sp.start + sp.len {
                            *r.add(t) += *air.add(*patch.get_unchecked(t) as usize);
                        }
                    }
                }
            }
            Chunk::Strings(from, to) => {
                let next = self.vb.load(Ordering::Relaxed) as usize ^ 1;
                for i in from..to {
                    self.step_string(i);
                    let s = &*self.strings.ptr().add(i);
                    if !s.defer_advance {
                        self.advance_string(s, next);
                    }
                }
            }
        }
    }

    /// # Safety
    /// Caller has exclusive access to this string, after its increment has been computed.
    unsafe fn advance_string(&self, s: &StringRt, vb: usize) {
        if !s.awake {
            return;
        }
        let u = std::slice::from_raw_parts_mut(self.u.ptr().add(s.off + 1), s.n - 1);
        let v = std::slice::from_raw_parts(self.v[vb].ptr().add(s.off + 1), s.n - 1);
        for (ul, vl) in u.iter_mut().zip(v) {
            *ul += *vl;
        }
    }

    /// The patch map never changes after construction, so it may be read from any thread.
    unsafe fn radiation_patches(&self) -> &[u32] {
        (*self.radiation.0.get()).patch_of_grid()
    }

    /// Explicit update of one string: reads displacement u and the current increments, writes
    /// the next increments into the other buffer, and leaves the bridge force in `end_force`.
    /// Displacements are advanced later, after the hammer has had its say.
    unsafe fn step_string(&self, index: usize) {
        if !*self.end_live.ptr().add(index) {
            return;
        }
        let s = self.strings.at(index);
        let n = s.n;
        let k = self.k;
        let vb = self.vb.load(Ordering::Relaxed) as usize;
        // Where the board has carried this string's end. The plate is only read in this phase.
        let at = &self.end_at[index];
        let (mut u_n, mut v_n) = (0.0, 0.0);
        for q in 0..at.n {
            u_n += at.w[q] * *self.w.ptr().add(at.idx[q]);
            v_n += at.w[q] * *self.vw.ptr().add(at.idx[q]);
        }
        let end_force = self.end_force.ptr().add(index);

        if !s.awake {
            // A damped string absorbs what the bridge sends into it: characteristic impedance.
            *end_force = if s.attached { -k * s.z0 * v_n } else { 0.0 };
            return;
        }
        // Nodes -1..=n+1, i.e. index l+1 holds node l.
        let u = std::slice::from_raw_parts_mut(self.u.ptr().add(s.off - 1), n + 3);
        let v = std::slice::from_raw_parts_mut(self.v[vb].ptr().add(s.off - 1), n + 3);
        let vn = std::slice::from_raw_parts_mut(self.v[vb ^ 1].ptr().add(s.off - 1), n + 3);
        // Both ends are simply supported: zero bending moment, expressed through ghost nodes.
        u[n + 1] = u_n;
        v[n + 1] = v_n;
        u[0] = -u[2];
        u[n + 2] = 2.0 * u_n - u[n];

        if s.attached {
            let (ua, ub, va) = (u[n], u[n - 1], v[n]);
            *end_force = s.m_node * (-(s.a_t * (u_n - ua) + s.a_d * (v_n - va)) - s.a_b * (ub - 2.0 * ua + u_n));
        }
        string_kernel(u, v, vn, [s.cu0, s.cu1, s.cu2, s.cv0, s.cv1]);
        vn[1] = 0.0;
        // The bridge is prescribed, not a stencil unknown. Keep its sampled increment for
        // housekeeping (like u[n + 1], this is from before the plate advances this step).
        vn[n + 1] = v_n;

        // Stretching. The strain of a cell is its end-to-end stretch plus the extra length the
        // transverse wave takes up, half the mean squared slope. Tension follows the strain; where
        // it differs between cells the string is pulled along its length. This is one-way: the
        // tension change is not fed back into the transverse motion.
        s.tension_change = 0.0;
        if s.l_on {
            let (cells, span) = (s.l_cells, s.l_span);
            let xi = std::slice::from_raw_parts_mut(self.xi.ptr().add(s.l_off), LONG + 1);
            let vxi = std::slice::from_raw_parts_mut(self.vxi.ptr().add(s.l_off), LONG + 1);
            let invh2 = 1.0 / (s.h * s.h);
            let mut strain = [0.0f64; LONG];
            let mut peak = 0.0f64;
            let mut l = 0;
            for c in 0..cells {
                let end = if c == cells - 1 { n } else { l + span };
                let mut sq = 0.0;
                for j in l..end {
                    let dq = u[j + 2] - u[j + 1];
                    sq += dq * dq;
                }
                let len = (end - l) as f64 * s.h;
                strain[c] = (xi[c + 1] - xi[c]) / len + 0.5 * sq * invh2 / (end - l) as f64;
                peak = peak.max(strain[c].abs());
                l = end;
            }
            let len = span as f64 * s.h;
            // The node next to the longer last cell is heavier.
            let last_scale = 2.0 * span as f64 / (span + (n - (cells - 1) * span)) as f64;
            for c in 1..cells {
                let mut acc = s.l_a * len * (strain[c] - strain[c - 1]);
                if c == cells - 1 {
                    acc *= last_scale;
                }
                vxi[c] = (vxi[c] + acc) * s.l_damp;
                xi[c] += vxi[c];
            }
            s.tension_change = s.l_ea * strain[cells - 1];
            s.l_peak = peak.max(0.999 * s.l_peak);
            if s.l_peak < 2e-9 {
                // No longer stretching to any effect: stop until it is struck again.
                s.l_on = false;
                xi.fill(0.0);
                vxi.fill(0.0);
            }
            if s.attached {
                *end_force += k * k * s.l_bridge * s.tension_change;
            }
        }

        // Damper felt.
        if s.damp_rate != s.damp_target {
            // Felt takes a few milliseconds to seat or to let go.
            // Grounded contact approximation retained unchanged. Key return sets
            // first contact; release-speed compression proxy remains independent.
            let rate = if s.damp_target > s.damp_rate { 0.004 * s.damp_land } else { 0.006 };
            s.damp_rate += (s.damp_target - s.damp_rate) * rate * (48000.0 * k);
            if (s.damp_rate - s.damp_target).abs() < 1e-3 {
                s.damp_rate = s.damp_target;
            }
        }
        if s.damp_rate > 0.0 {
            // The damper rests on the string (toward the board) and soaks up its motion: viscous,
            // backward Euler so that it is stable at any strength.
            let scale = (1.0 + s.s0k) / (1.0 + s.s0k + 2.0 * k * s.damp_rate);
            let press = s.press_k * s.damp_rate / self.damp_max;
            for x in &mut vn[s.dmp0 + 1..s.dmp1 + 1] {
                *x = (*x - press) * scale;
            }
        }
    }
}

/// Decaying vibration ends in subnormal numbers, which are very slow to compute with. Flush
/// them to zero while rendering, and restore the caller's floating point mode after.
struct FlushDenormals {
    #[cfg(target_arch = "aarch64")]
    saved: u64,
}

impl FlushDenormals {
    fn new() -> Self {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            let saved: u64;
            std::arch::asm!("mrs {}, fpcr", out(reg) saved);
            let set = saved | (1 << 24);
            std::arch::asm!("msr fpcr, {}", in(reg) set);
            FlushDenormals { saved }
        }
        #[cfg(not(target_arch = "aarch64"))]
        FlushDenormals {}
    }
}

impl Drop for FlushDenormals {
    fn drop(&mut self) {
        #[cfg(target_arch = "aarch64")]
        unsafe {
            std::arch::asm!("msr fpcr, {}", in(reg) self.saved);
        }
    }
}

#[cfg(target_os = "macos")]
extern "C" {
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
}

/// Asks the system for a performance core for the calling thread.
pub fn request_performance_core() {
    #[cfg(target_os = "macos")]
    unsafe {
        pthread_set_qos_class_self_np(0x21, 0); // QOS_CLASS_USER_INTERACTIVE
    }
}

#[inline]
fn spin() {
    std::hint::spin_loop();
}

#[derive(Clone, Copy)]
struct Pulse {
    at: Interp,
    peak: f64,
    step: usize,
    length: usize,
}

/// A copy of what a display needs. Make one with `Piano::new_snapshot`, then fill it from the
/// audio thread (allocation free).
#[derive(Clone)]
pub struct Snapshot {
    /// Plate displacement, grid storage, metres.
    pub board: Vec<f32>,
    /// Per string: largest displacement, metres.
    pub string_level: Vec<f32>,
    /// Displacement along the strings of the focus note.
    pub shape: [Vec<f32>; 3],
    pub shape_nodes: [usize; 3],
    pub hammer_force: [f32; 3],
    pub focus_midi: i32,
    pub awake: usize,
    /// Read-only real action history; identity is also labelled by Display.generation.
    pub action: ActionTrace,
    /// Focus note real-time simulation telemetry (copied at 30 Hz, zero audio-step work).
    /// Damper lift: 0.0 resting on wire .. 1.0 fully lifted.
    pub damper_lift: f32,
    /// Focus note damper contact span [start, end] normalized along speaking length (0..1).
    /// [0.0, 0.0] when the note has no damper (high treble).
    pub damper_span: [f32; 2],
    /// Focus key depression: 0.0 at rest .. 1.0 full key travel.
    pub key_depression: f32,
    /// Focus wooden shank elastic flex (eta - xi), metres during hammer flight.
    pub shank_flex_m: f32,
    /// Focus hammer felt compression max(0, head - wire), metres during contact.
    pub felt_compression_m: f32,
    /// Per-key (A0=0..C8=87) real normalized hammer elevation (0.0 rest .. 1.0 string).
    pub key_hammer_lift: [f32; 88],
    /// Per-key (A0=0..C8=87) real normalized damper lift (0.0 resting .. 1.0 lifted).
    pub key_damper_lift: [f32; 88],
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            board: Vec::new(),
            string_level: Vec::new(),
            shape: [Vec::new(), Vec::new(), Vec::new()],
            shape_nodes: [0; 3],
            hammer_force: [0.0; 3],
            focus_midi: -1,
            awake: 0,
            action: ActionTrace::default(),
            damper_lift: 0.0,
            damper_span: [0.0, 0.0],
            key_depression: 0.0,
            shank_flex_m: 0.0,
            felt_compression_m: 0.0,
            key_hammer_lift: [0.0; 88],
            key_damper_lift: [0.0; 88],
        }
    }
}

pub type ThreadHook = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pristine {
    Candidate,
    Zero,
    Disabled,
}

pub struct Piano {
    sh: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
    k: f64,
    fs: f64,
    out_gain: f64,
    os: usize,
    notes: Vec<NoteRt>,
    note_of_midi: [Option<usize>; 128],
    /// Share of full damper strength the pedal position leaves on.
    pedal_damping: f64,
    sostenuto_down: bool,
    never_sleep: bool,
    action: ActionDesign,
    soft: bool,
    solve_work: Vec<f64>,
    // Decimation when oversampling.
    fir: Vec<f64>,
    hist: [Vec<f64>; 2],
    hist_pos: usize,
    // External point force on the board for the next step only (measurement hook).
    ext_at: Interp,
    ext_f: f64,
    // Knocks in flight: short force pulses on the board.
    pulses: [Pulse; 32],
    pulse_count: usize,
    // Pedal work: where it is, how fast it is moving, and its friction noise.
    pedal_at: Interp,
    pedal_pos: f64,
    pedal_speed: f64,
    pedal_hiss: f64,
    pedal_step: u64,
    mech_noise: f64,
    rng: u32,
    step_count: u64,
    action_observer: ActionObserver,
    quiet_checks: u32,
    hammer_iters_max: usize,
    // One-way constructor experiment: prove a real zero tick once, never detect tails.
    pristine: Pristine,
}

impl Piano {
    pub fn new(d: &PianoDesign, o: &PianoOptions) -> Result<Piano, String> {
        let os = o.oversample.max(1);
        let fs = o.sample_rate * os as f64;
        let k = 1.0 / fs;
        let damp_max = 1500.0 * d.damper_strength;
        let mut note_of_midi = [None; 128];
        let mut strings: Vec<StringRt> = Vec::new();
        let mut notes: Vec<NoteRt> = Vec::new();
        let mut att: Vec<Attachment> = Vec::new();
        let mut xi_len = 0usize;
        let mut offset = 0usize;

        for nd in &d.notes {
            let count = nd.strings.len();
            if count > 3 {
                return Err("at most three strings per note".into());
            }
            let first = strings.len();
            let h_mass = nd.hammer_mass * (1.0 - nd.shank_share);
            let s_mass = nd.hammer_mass * nd.shank_share;
            let (mut s_k, mut s_c) = (0.0, 0.0);
            if s_mass > 0.0 {
                let mu = h_mass * s_mass / (h_mass + s_mass);
                let om = 2.0 * PI * nd.shank_hz;
                s_k = mu * om * om;
                s_c = 2.0 * nd.shank_loss * mu * om;
            }
            let note_index = notes.len();

            let build_string = |sd: &StringDesign, duplex: bool, strings: &mut Vec<StringRt>, offset: &mut usize, xi_len: &mut usize| -> Result<bool, String> {
                // Finest grid the stability condition allows. The two coefficients of the discrete
                // string (wave speed and stiffness) are then fitted so that its fundamental is exactly
                // f0 and a mid-band partial sits where the continuous stiff string puts it. This is
                // what a tuner does for the fundamental, extended to cancel the leading part of the
                // grid dispersion. The physical stiffness enters through the inharmonicity B.
                let b = PI * PI * sd.ei / (sd.tension * sd.length * sd.length);
                let target = |q: usize| q as f64 * sd.f0 * ((1.0 + b * (q * q) as f64) / (1.0 + b)).sqrt();
                let mut c2 = sd.tension / sd.rho_a;
                let mut kap2 = sd.ei / sd.rho_a;
                let a = c2 * k * k + 4.0 * sd.sigma1 * k;
                let hmin = (0.5 * (a + (a * a + 16.0 * kap2 * k * k).sqrt())).sqrt();
                let mut n = (sd.length / hmin).floor() as usize + 2;
                let mut fit_q = 1;
                let mut h = 0.0;
                let mut found = false;
                while n >= 3 {
                    h = sd.length / n as f64;
                    let sn = |q: usize| (q as f64 * PI / (2.0 * n as f64)).sin().powi(2);
                    let lhs = |q: usize| 4.0 / (k * k) * (PI * target(q) * k).sin().powi(2);
                    let h4 = h * h * h * h;
                    let stable = |cc: f64, kk: f64| cc > 0.0 && kk >= 0.0 && h4 >= ((cc * k * k + 4.0 * sd.sigma1 * k) * h * h + 4.0 * kk * k * k) * (1.0 + 1e-9);
                    // Fit: lhs(q) = c2 * (4/h^2) s_q + kap2 * (16/h^4) s_q^2 for q = 1 and q = fit.
                    // Among the stable fits, keep the one with the smallest worst-case error over
                    // the partials below 8 kHz.
                    let q_max = (n - 1).min(400);
                    let mut best = 1e30;
                    for q in 2..=q_max.max(2) {
                        if target(q) > 0.45 * fs && q > 2 {
                            break;
                        }
                        let (a1, b1) = (4.0 / (h * h) * sn(1), 16.0 / h4 * sn(1) * sn(1));
                        let (a2, b2) = (4.0 / (h * h) * sn(q), 16.0 / h4 * sn(q) * sn(q));
                        let det = a1 * b2 - a2 * b1;
                        let cc = (lhs(1) * b2 - lhs(q) * b1) / det;
                        let kk = (a1 * lhs(q) - a2 * lhs(1)) / det;
                        if !stable(cc, kk) {
                            continue;
                        }
                        let mut worst = 0.0f64;
                        let mut m = 2;
                        while m <= q_max && target(m) < 8000.0 {
                            let x = cc * k * k / (h * h) * sn(m) + 4.0 * kk * k * k / h4 * sn(m) * sn(m);
                            let f = if x >= 1.0 { 1e-3 } else { x.sqrt().asin() / (PI * k) };
                            worst = worst.max((f / target(m)).log2().abs());
                            m += 1;
                        }
                        if worst < best {
                            best = worst;
                            c2 = cc;
                            kap2 = kk;
                            fit_q = q;
                            found = true;
                        }
                    }
                    if found {
                        break;
                    }
                    n -= 1;
                }
                if !found {
                    if duplex {
                        return Ok(false); // too short for the grid: its pitch is beyond hearing anyway
                    }
                    return Err("string cannot be resolved at this sample rate".into());
                }
                let a_t = c2 * k * k / (h * h);
                let a_b = kap2 * k * k / (h * h * h * h);
                let a_d = 2.0 * sd.sigma1 * k / (h * h);
                let s0k = sd.sigma0 * k;
                let m_node = sd.rho_a * h;
                let tension = c2 * sd.rho_a;

                let hp = nd.strike_ratio * sd.length / h;
                let ham_l = (hp.floor() as usize).min(n - 2);
                let fr = hp - ham_l as f64;
                let ham_g = [if ham_l == 0 { 0.0 } else { 1.0 - fr }, fr]; // node 0 is the fixed agraffe
                let ham_a = k * k * (ham_g[0] * ham_g[0] + ham_g[1] * ham_g[1]) / (m_node * (1.0 + s0k));

                let dl = (0.12 * sd.length).clamp(0.03, 0.14);
                let dmp0 = (ham_l + 3).max((0.16 * sd.length / h).ceil() as usize);
                let dmp1 = n.min(dmp0 + ((dl / h).round() as usize).max(2));
                let mut has_damper = nd.has_damper && dmp0 < dmp1;
                let mut damp_rate = if has_damper { damp_max } else { 0.0 };
                let press_k = if has_damper { k * k * d.damper_press * d.damper_strength / (dmp1 - dmp0) as f64 / m_node } else { 0.0 };

                // Longitudinal grid: the coarsest whole number of transverse segments per cell that
                // keeps the (fast) longitudinal wave stable. Short strings get a single cell, which
                // is the quasi-static limit: the tension follows the mean squared slope.
                let (mut l_cells, mut l_span, mut l_a, mut l_off) = (0, 0, 0.0, 0);
                if d.longitudinal && sd.ea > 0.0 && !duplex {
                    let c_l = (sd.ea / sd.rho_a).sqrt();
                    let mut span = ((c_l * k * 1.03 / h).ceil() as usize).max(1);
                    let cells = (n / span).min(LONG).max(1);
                    span = if cells == 1 { n } else { span.max(n / cells) }; // the last cell takes any remainder, so it is the longest
                    l_cells = cells;
                    l_span = span;
                    let len = span as f64 * h;
                    l_a = sd.ea * k * k / (sd.rho_a * len * len);
                    l_off = *xi_len;
                    *xi_len += LONG + 1;
                }
                if duplex {
                    has_damper = false;
                    damp_rate = 0.0;
                }
                let inv = 1.0 / (1.0 + s0k);
                strings.push(StringRt {
                    note: note_index,
                    n,
                    off: *offset + 1,
                    h,
                    a_t,
                    a_b,
                    a_d,
                    s0k,
                    m_node,
                    z0: (tension * sd.rho_a).sqrt(),
                    tension,
                    cu0: (-2.0 * a_t - 6.0 * a_b) * inv,
                    cu1: (a_t + 4.0 * a_b) * inv,
                    cu2: -a_b * inv,
                    cv0: (1.0 - s0k - 2.0 * a_d) * inv,
                    cv1: a_d * inv,
                    t_h: tension / h,
                    ei_h3: kap2 * sd.rho_a / (h * h * h),
                    damp_h: 2.0 * sd.sigma1 * sd.rho_a / h,
                    attached: sd.attached,
                    awake: false,
                    defer_advance: false,
                    has_damper,
                    sideways: false,
                    ham_w: if duplex { 0.0 } else { 1.0 },
                    bridge: sd.bridge,
                    ham_l,
                    ham_g,
                    ham_a,
                    dmp0,
                    dmp1,
                    damp_rate,
                    damp_target: damp_rate,
                    damped_steps: 0,
                    damp_land: 1.0,
                    press_k,
                    end_peak: 0.0,
                    fit_partial: fit_q,
                    f0: sd.f0,
                    inharm: b,
                    zeta: 0.0,
                    zeta_prev: 0.0,
                    force: 0.0,
                    l_cells,
                    l_span,
                    l_on: false,
                    l_a,
                    l_damp: 1.0 / (1.0 + 30.0 * k), // longitudinal modes ring for a few tenths of a second
                    l_ea: sd.ea,
                    l_bridge: 2.0 * (d.downbearing_deg * PI / 180.0).sin(),
                    l_peak: 0.0,
                    l_off,
                    tension_change: 0.0,
                });
                *offset += n + 3;
                Ok(true)
            };

            for sd in &nd.strings {
                build_string(sd, false, &mut strings, &mut offset, &mut xi_len)?;
            }
            // Second polarisation: the same string moving parallel to the soundboard.
            let twin = nd.two_polarisations;
            let total = count * if twin { 2 } else { 1 };
            for i in 0..count {
                strings[first + i].ham_w = if twin { nd.hammer_tilt.cos() } else { 1.0 };
            }
            if twin {
                for i in 0..count {
                    let mut t = strings[first + i].clone();
                    t.ham_w = nd.hammer_tilt.sin();
                    t.sideways = true;
                    t.l_cells = 0; // stretching is computed once, on the upright motion
                    t.press_k = 0.0; // the damper rests on the string from above, not from the side
                    t.off = offset + 1;
                    offset += t.n + 3;
                    strings.push(t);
                }
            }
            // How each string's end follows the board. The upright motion follows the board's
            // displacement at the bridge. The sideways motion follows the bridge top as the board
            // tilts under it: the board's slope across the string, times the height of the string
            // above the board's middle. Nothing here is a free parameter.
            let lever = d.board.bridge_height + 0.5 * d.board.thickness;
            let half_width = 0.5 * d.board.bridge_width;
            for i in 0..total {
                let sd = &nd.strings[i % count];
                let s = &strings[first + i];
                let points = if i < count {
                    vec![(sd.bridge, 1.0)]
                } else {
                    vec![
                        (Vec2 { x: sd.bridge.x + half_width, y: sd.bridge.y }, lever / (2.0 * half_width)),
                        (Vec2 { x: sd.bridge.x - half_width, y: sd.bridge.y }, -lever / (2.0 * half_width)),
                    ]
                };
                if s.attached {
                    att.push(Attachment { points, mass: 0.5 * s.m_node, dashpot: s.z0 });
                }
            }
            for sd in &nd.duplex {
                if build_string(sd, true, &mut strings, &mut offset, &mut xi_len)? {
                    let s = strings.last().unwrap();
                    if s.attached {
                        att.push(Attachment { points: vec![(sd.bridge, 1.0)], mass: 0.5 * s.m_node, dashpot: s.z0 });
                    }
                }
            }
            note_of_midi[(nd.midi & 127) as usize] = Some(notes.len());
            notes.push(NoteRt {
                midi: nd.midi,
                first,
                count,
                total,
                twin,
                h_mass,
                h_k: nd.hammer_k,
                h_p: nd.hammer_p,
                h_mu: nd.hammer_mu,
                eta: 0.0,
                veta: 0.0,
                s_mass,
                s_k,
                s_c,
                xi: 0.0,
                vxi: 0.0,
                hammer_active: false,
                key_down: false,
                sost: false,
                key_phase: 0,
                ky: 0.0,
                kv: 0.0,
                k_force: 0.0,
                k_slow: 0.0,
                returning: false,
                checked: false,
                hy: 0.0,
                hv: 0.0,
                h_slow: 0.0,
                h_total: nd.hammer_mass,
                thump: nd.key_thump,
                thump_at: Interp::default(),
            });
        }

        let plate = Plate::new(&d.board, &att, k, o.board_grid_scale.max(1.0))?;
        let mut end_at = vec![Coupling::default(); strings.len()];
        let mut end_live = vec![true; strings.len()];
        {
            let mut ai = 0;
            for (i, s) in strings.iter_mut().enumerate() {
                if s.attached {
                    end_at[i] = plate.couple(&att[ai].points);
                    ai += 1;
                }
                // Sideways motion is followed only for strings that have been struck: it starts
                // asleep. Strings without a damper (top treble, duplex, detached test strings)
                // never sleep.
                s.awake = !s.sideways && (!s.has_damper || !s.attached || d.damped_strings_ring);
                if s.sideways && !s.attached {
                    s.awake = true;
                }
                end_live[i] = s.awake || !s.sideways; // a string's sideways motion at rest neither moves nor loads the board
            }
        }
        for (i, n) in notes.iter_mut().enumerate() {
            n.thump_at = plate.interp(d.notes[i].key_thump_at);
        }
        let pedal_at = plate.interp(Vec2 { x: 0.70, y: 0.06 }); // the pedal work hangs under the middle of the keybed
        let radiation = Radiation::new(&plate, d, fs);
        let air_now = radiation.air_force().to_vec();

        // Chunks: the radiation task, ranges of soundboard spans, then ranges of strings. Heavy
        // ones come first. Every claim costs a round trip of a shared cache line between cores,
        // so there must be few of them.
        let mut chunks = vec![Chunk::Radiation];
        {
            let per = (plate.n / 3).max(200);
            let (mut start, mut nodes) = (0, 0);
            for i in 0..plate.spans.len() {
                nodes += plate.spans[i].len;
                if nodes >= per || i + 1 == plate.spans.len() {
                    chunks.push(Chunk::PlateSpans(start, i + 1));
                    start = i + 1;
                    nodes = 0;
                }
            }
            let per = (offset / 8).max(400);
            let (mut start, mut nodes) = (0, 0);
            for i in 0..strings.len() {
                nodes += strings[i].n;
                if nodes >= per || i + 1 == strings.len() {
                    chunks.push(Chunk::Strings(start, i + 1));
                    start = i + 1;
                    nodes = 0;
                }
            }
        }

        // Kaiser windowed sinc, cutoff at 0.45 of the output rate.
        let (mut fir, mut hist) = (Vec::new(), [Vec::new(), Vec::new()]);
        if os > 1 {
            let half = 24 * os;
            let len = 2 * half + 1;
            let fc = 0.45 / os as f64;
            let beta = 9.0;
            let bessel = |x: f64| {
                let (mut s, mut t) = (1.0, 1.0);
                for i in 1..40 {
                    t *= (x / (2.0 * i as f64)) * (x / (2.0 * i as f64));
                    s += t;
                }
                s
            };
            fir = (0..len)
                .map(|i| {
                    let m = i as f64 - half as f64;
                    let sinc = if m == 0.0 { 2.0 * fc } else { (2.0 * PI * fc * m).sin() / (PI * m) };
                    sinc * bessel(beta * (1.0 - (m / half as f64).powi(2)).sqrt()) / bessel(beta)
                })
                .collect();
            let sum: f64 = fir.iter().sum();
            fir.iter_mut().for_each(|x| *x /= sum);
            hist = [vec![0.0; len], vec![0.0; len]];
        }

        let grid = plate.grid_size;
        let solve_work = vec![0.0; plate.solve_work_len()];
        let ns = strings.len();
        let sh = Arc::new(Shared {
            k,
            damp_max,
            plate,
            strings: Buf::from_vec(strings),
            u: Buf::from_vec(vec![0.0; offset]),
            v: [Buf::from_vec(vec![0.0; offset]), Buf::from_vec(vec![0.0; offset])],
            vb: AtomicI32::new(0),
            xi: Buf::from_vec(vec![0.0; xi_len]),
            vxi: Buf::from_vec(vec![0.0; xi_len]),
            end_live: Buf::from_vec(end_live),
            end_at,
            end_force: Buf::from_vec(vec![0.0; ns]),
            w: Buf::from_vec(vec![0.0; grid]),
            vw: Buf::from_vec(vec![0.0; grid]),
            r: Buf::from_vec(vec![0.0; grid]),
            rad_in: Buf::from_vec(vec![0.0; grid]),
            radiation: Cell::new(radiation),
            rad_out: Cell::new((0.0, 0.0)),
            air_now: Buf::from_vec(air_now),
            chunks,
            ticket: AtomicU64::new(NO_JOB),
            done: AtomicI32::new(0),
            stop: AtomicBool::new(false),
            active: AtomicBool::new(false),
            in_block: AtomicBool::new(false),
            park: Mutex::new(()),
            wake: Condvar::new(),
        });
        let pristine = if !d.damped_strings_ring && !sh.strings._own.iter().any(|s| s.awake && s.has_damper) {
            Pristine::Candidate
        } else {
            Pristine::Disabled
        };
        #[allow(unused_mut)]
        let mut piano = Piano {
            sh,
            threads: Vec::new(),
            k,
            fs,
            out_gain: o.output_gain,
            os,
            notes,
            note_of_midi,
            pedal_damping: 1.0,
            sostenuto_down: false,
            never_sleep: d.damped_strings_ring,
            action: d.action,
            soft: false,
            solve_work,
            fir,
            hist,
            hist_pos: 0,
            ext_at: Interp::default(),
            ext_f: 0.0,
            pulses: [Pulse { at: Interp::default(), peak: 0.0, step: 0, length: 0 }; 32],
            pulse_count: 0,
            pedal_at,
            pedal_pos: 0.0,
            pedal_speed: 0.0,
            pedal_hiss: 0.0,
            pedal_step: 0,
            mech_noise: d.mechanism_noise,
            rng: 0x2545F491,
            step_count: 0,
            action_observer: ActionObserver::new(k),
            quiet_checks: 1 << 20,
            hammer_iters_max: 0,
            pristine,
        };
        piano.sh.active.store(false, Ordering::Relaxed);
        Ok(piano)
    }
}

/// Stiff string update. Slices hold nodes -1..=N+1; nodes 1..N-1 are written.
#[inline]
fn string_kernel(u: &[f64], v: &[f64], vn: &mut [f64], c: [f64; 5]) {
    let [cu0, cu1, cu2, cv0, cv1] = c;
    let len = vn.len();
    assert!(len >= 5 && u.len() == len && v.len() == len);
    for l in 2..len - 2 {
        unsafe {
            let (ul, vl) = (|j: usize| *u.get_unchecked(j), |j: usize| *v.get_unchecked(j));
            *vn.get_unchecked_mut(l) = cu2.mul_add(ul(l - 2) + ul(l + 2), cu1.mul_add(ul(l - 1) + ul(l + 1), cu0.mul_add(ul(l), cv1.mul_add(vl(l - 1) + vl(l + 1), cv0 * vl(l)))));
        }
    }
}

/// Plate update over one span: scale the right hand side into the change of the increment,
/// advance increment and displacement, and keep the change for the radiation task.
#[inline]
fn plate_advance(r: &[f64], scale: &[f64], vw: &mut [f64], w: &mut [f64], keep: &mut [f64]) {
    let n = r.len();
    assert!(scale.len() == n && vw.len() == n && w.len() == n && keep.len() == n);
    for t in 0..n {
        let dv = r[t] * scale[t]; // dt^2 * acceleration
        vw[t] += dv;
        w[t] += vw[t];
        keep[t] = dv;
    }
}

impl Piano {
    // Serial access to the shared state. Sound as long as no job is posted, which is the case
    // everywhere outside `step`.
    fn strings(&self) -> &[StringRt] {
        unsafe { self.sh.strings.all() }
    }
    #[allow(clippy::mut_from_ref)]
    fn strings_mut(&self) -> &mut [StringRt] {
        unsafe { self.sh.strings.all_mut() }
    }
    fn vb(&self) -> usize {
        self.sh.vb.load(Ordering::Relaxed) as usize
    }
    /// Nodes 0..=n of a string array.
    #[allow(clippy::mut_from_ref)]
    fn nodes<'a>(&'a self, buf: &'a Buf<f64>, s: &StringRt) -> &'a mut [f64] {
        unsafe { std::slice::from_raw_parts_mut(buf.ptr().add(s.off), s.n + 1) }
    }

    fn wake(&mut self, index: usize) {
        let s = &mut self.strings_mut()[index];
        if s.awake {
            return;
        }
        // Start from the static shape for the current bridge position, so waking is silent.
        let (w, vw) = unsafe { (self.sh.w.all(), self.sh.vw.all()) };
        let (u_n, v_n) = if s.attached { (self.sh.end_at[index].eval(w), self.sh.end_at[index].eval(vw)) } else { (0.0, 0.0) };
        let u = self.nodes(&self.sh.u, s);
        let v = self.nodes(&self.sh.v[self.vb()], s);
        for l in 0..=s.n {
            u[l] = u_n * l as f64 / s.n as f64;
            v[l] = v_n * l as f64 / s.n as f64;
        }
        // A sleeping string lies under its damper, pressed slightly out of line. Wake it in that
        // shape (the string's tension against the damper's weight, solved along the string), so
        // that lifting the damper lets it go.
        let press = s.press_k * s.damp_rate / self.sh.damp_max;
        if press > 0.0 && s.n > 2 {
            let scratch = self.nodes(&self.sh.v[self.vb() ^ 1], s); // overwritten by the next step anyway
            // a_t (-d[l-1] + 2 d[l] - d[l+1]) = -press on the damper's nodes, d = 0 at both ends.
            let load = |l: usize| if l >= s.dmp0 && l < s.dmp1 { -press / s.a_t } else { 0.0 };
            let mut prev_c = 0.0;
            let mut prev_d = 0.0;
            for l in 1..s.n {
                let denom = 2.0 + prev_c;
                prev_c = -1.0 / denom;
                prev_d = (load(l) + prev_d) / denom;
                scratch[l] = prev_c;
                u[l] += prev_d; // the forward-swept right hand side, corrected below
            }
            // Back substitution on the deflection alone: keep it apart from the straight line.
            let mut d_next = 0.0;
            for l in (1..s.n).rev() {
                let line = u_n * l as f64 / s.n as f64;
                let d = (u[l] - line) - scratch[l] * d_next;
                u[l] = line + d;
                d_next = d;
            }
            scratch[..=s.n].fill(0.0);
        }
        // The other buffer may have been used as the damper-shape scratch space. Its
        // endpoints are prescribed too; the string kernel writes only interior nodes.
        for buf in &self.sh.v {
            let v = unsafe { std::slice::from_raw_parts_mut(buf.ptr().add(s.off - 1), s.n + 3) };
            v[1] = 0.0;
            v[s.n + 1] = v_n;
            v[0] = -v[2];
            v[s.n + 2] = 2.0 * v_n - v[s.n];
        }
        s.awake = true;
        unsafe { *self.sh.end_live.at(index) = true };
    }

    fn sleep(&mut self, index: usize) {
        let s = &mut self.strings_mut()[index];
        for buf in [&self.sh.u, &self.sh.v[0], &self.sh.v[1]] {
            unsafe { std::slice::from_raw_parts_mut(buf.ptr().add(s.off - 1), s.n + 3).fill(0.0) };
        }
        s.awake = false;
        if s.sideways {
            unsafe {
                *self.sh.end_live.at(index) = false;
                *self.sh.end_force.at(index) = 0.0;
            }
        }
        s.l_on = false;
        s.l_peak = 0.0;
        s.tension_change = 0.0;
        if s.l_cells > 0 {
            unsafe {
                std::slice::from_raw_parts_mut(self.sh.xi.ptr().add(s.l_off), LONG + 1).fill(0.0);
                std::slice::from_raw_parts_mut(self.sh.vxi.ptr().add(s.l_off), LONG + 1).fill(0.0);
            }
        }
    }

    /// Position (or increment) of string contact `i` of a note along the strike direction.
    fn along(&self, n: &NoteRt, i: usize, buf: &Buf<f64>) -> f64 {
        let strings = self.strings();
        let mut g = 0.0;
        for pol in 0..if n.twin { 2 } else { 1 } {
            let s = &strings[n.first + i + pol * n.count];
            let x = self.nodes(buf, s);
            g += s.ham_w * (s.ham_g[0] * x[s.ham_l] + s.ham_g[1] * x[s.ham_l + 1]);
        }
        g
    }

    /// `speed`: hammer speed at let-off, m/s. Call from the same thread as `process`.
    pub fn note_on(&mut self, midi: i32, speed: f64) {
        let Some(ni) = self.note_of_midi[(midi & 127) as usize] else { return };
        if speed > 0.0 && self.action_observer.focus().is_some() {
            // Real old-focus endpoint BEFORE retarget, pristine/wake/key/head mutation.
            let previous=self.observed_action_pose(self.step_count);
            self.action_observer.end_focus(previous);
        }
        self.pristine = Pristine::Disabled; // Even speed 0 wakes the bent, preloaded string.
        let mut n = self.notes[ni].clone();
        n.key_down = true;
        for i in 0..n.total {
            self.wake(n.first + i);
            let s = &mut self.strings_mut()[n.first + i];
            s.damp_rate = 0.0;
            if s.l_cells > 0 {
                s.l_on = true;
                s.l_peak = 1.0;
            }
        }
        if speed > 0.0 {
            // The key: at let-off it is most of the way down, moving at the hammer's speed over the
            // action ratio, pushed by the force that got the hammer up to that speed.
            let a = &self.action;
            n.key_phase = 1;
            n.ky = a.key_at_let_off;
            n.kv = speed / a.ratio;
            n.k_force = ((a.key_mass + n.h_total * a.ratio * a.ratio) * n.kv * n.kv / (2.0 * a.key_at_let_off) + 0.5).clamp(0.6, 40.0);
            n.returning = false;
            n.checked = false;
            let mut top = -1e30f64;
            for i in 0..n.count {
                top = top.max(self.along(&n, i, &self.sh.u));
            }
            n.eta = top - a.let_off.max(1e-5);
            n.veta = speed * self.k;
            n.xi = n.eta; // head and shank arrive together, the shank unbent
            n.vxi = n.veta;
            n.hammer_active = true;
            for s in &mut self.strings_mut()[n.first..n.first + n.total] {
                s.defer_advance = true;
            }
            for i in 0..n.count {
                let zeta = n.eta - self.along(&n, i, &self.sh.u);
                let zeta_prev = zeta - (n.veta - self.along(&n, i, &self.sh.v[self.vb()]));
                let s = &mut self.strings_mut()[n.first + i];
                s.zeta = zeta;
                s.zeta_prev = zeta_prev;
                s.force = 0.0;
            }
        }
        self.notes[ni] = n;
        self.refresh_damper(ni);
        if speed > 0.0 {
            let n=&self.notes[ni];
            self.action_observer.begin(ni,n.midi,n.count,self.step_count,self.action.let_off,
                self.action.check_distance,self.action.rest_distance,n.h_total);
            if self.action_observer.focus()==Some(ni) {
                let pose=self.observed_action_pose(self.step_count);
                self.action_observer.finish_step(self.step_count,Some(pose));
            }
        }
    }

    /// `release_speed`: how fast the key comes back up, 1 = ordinary. Sets how quickly the
    /// damper lands.
    pub fn note_off(&mut self, midi: i32, release_speed: f64) {
        let Some(ni) = self.note_of_midi[(midi & 127) as usize] else { return };
        if !self.notes[ni].key_down {
            return;
        }
        self.pristine = Pristine::Disabled;
        let release_speed = release_speed.clamp(0.25, 4.0);
        let n = &mut self.notes[ni];
        n.key_down = false;
        // Off starts the existing virtual key's return. Its lifter stays up until
        // the simulated key coordinate reaches the regulated contact point.
        n.key_phase = 3;
        n.kv = -0.15 * release_speed;
        n.k_force = 0.0;
        if n.checked {
            // the backcheck lets the hammer go
            n.checked = false;
            n.returning = true;
        }
        let (first, total, sost) = (n.first, n.total, n.sost);
        if !sost {
            for s in &mut self.strings_mut()[first..first + total] {
                if s.has_damper { s.damp_land = release_speed; }
            }
        }
        self.refresh_damper(ni);
    }

    /// One owner of grounded felt engagement. Key, sustain and sostenuto lifters
    /// compose here, so releasing one control cannot defeat another's support.
    /// This gates the existing grounded brake; it is not a finite-mass felt model.
    fn refresh_damper(&mut self, ni: usize) {
        let n = &self.notes[ni];
        let lifted = n.key_down || n.sost || n.ky > self.action.key_at_damper;
        let target = if lifted { 0.0 } else { self.sh.damp_max * self.pedal_damping };
        let (first, total) = (n.first, n.total);
        for i in first..first + total {
            let s = &self.strings()[i];
            if !s.has_damper { continue; }
            if target < self.sh.damp_max && !s.sideways { self.wake(i); }
            self.strings_mut()[i].damp_target = target;
        }
    }

    pub fn set_sustain(&mut self, down: bool) {
        self.set_pedal(if down { 1.0 } else { 0.0 });
    }

    /// Sustain pedal position, 0 (up) to 1 (down).
    pub fn set_pedal(&mut self, position: f64) {
        if position != self.pedal_pos {
            self.pristine = Pristine::Disabled;
        }
        // Dampers start to leave the strings around 34 % of the pedal travel and are clear by
        // 47 %; in between the felt only grazes the string, modelled as a weaker damper. The
        // range is inferred from recorded performances: players release a bass key with the pedal
        // near 50 % on its way down and expect the note to be caught, and dip to about 40 % to
        // clear the previous harmony.
        let x = ((position - 0.34) / 0.13).clamp(0.0, 1.0);
        let damping = 1.0 - x * x * (3.0 - 2.0 * x);
        // Pedal work: its speed drives a faint friction noise, and it knocks at each end of its travel.
        {
            let seconds = ((self.step_count - self.pedal_step) as f64 * self.k).max(1e-3);
            let speed = (position - self.pedal_pos).abs() / seconds; // travels per second
            if speed > 0.0 {
                self.pedal_speed = speed.min(30.0);
                self.pedal_hiss = self.pedal_hiss.max((self.pedal_speed / 6.0).min(1.5));
                let bottom = position >= 0.97 && self.pedal_pos < 0.97;
                let top = position <= 0.03 && self.pedal_pos > 0.03;
                if bottom || top {
                    self.knock(self.pedal_at, 0.0015 * self.mech_noise * self.pedal_speed * if top { 0.7 } else { 1.0 }, 0.006);
                }
            }
            self.pedal_pos = position;
            self.pedal_step = self.step_count;
        }
        if damping == self.pedal_damping {
            return;
        }
        self.pedal_damping = damping;
        for ni in 0..self.notes.len() {
            let n = &self.notes[ni];
            if !n.key_down && !n.sost {
                let (first, total) = (n.first, n.total);
                for s in &mut self.strings_mut()[first..first + total] {
                    if s.has_damper { s.damp_land = 1.0; }
                }
            }
            self.refresh_damper(ni);
        }
    }

    /// Una corda: the action shifts so each hammer misses one string of its three.
    pub fn set_soft_pedal(&mut self, down: bool) {
        if down != self.soft {
            self.pristine = Pristine::Disabled;
        }
        self.soft = down;
    }

    /// Sostenuto: dampers of the keys held at this moment stay up until the pedal is released.
    pub fn set_sostenuto(&mut self, down: bool) {
        if down == self.sostenuto_down { return; }
        self.pristine = Pristine::Disabled;
        self.sostenuto_down = down;
        for ni in 0..self.notes.len() {
            let n = &mut self.notes[ni];
            let release = !down && n.sost && !n.key_down;
            n.sost = down && n.key_down;
            if release {
                let (first, total) = (n.first, n.total);
                for s in &mut self.strings_mut()[first..first + total] {
                    if s.has_damper { s.damp_land = 1.0; }
                }
            }
            self.refresh_damper(ni);
        }
    }

    /// Time from let-off to the hammer reaching the string, for a given hammer speed.
    pub fn strike_delay(&self, hammer_speed: f64) -> f64 {
        if hammer_speed > 0.0 {
            self.action.let_off / hammer_speed
        } else {
            0.0
        }
    }

    /// A short knock on the case: a half-sine force pulse carrying the given impulse.
    fn knock(&mut self, at: Interp, impulse: f64, seconds: f64) {
        if !(impulse > 0.0) || self.pulse_count >= self.pulses.len() {
            return;
        }
        let len = ((seconds * self.fs) as usize).max(2);
        self.pulses[self.pulse_count] = Pulse { at, peak: impulse * PI / (2.0 * len as f64 * self.k), step: 0, length: len };
        self.pulse_count += 1;
    }

    /// The parts of the action that move after let-off, as bodies meeting felt: the key on its
    /// rails and the hammer on its way back. Felt is a stiffening spring with drag; what it
    /// pushes on is the case, and a share of the changing part of that force reaches the board.
    fn step_action(a: &ActionDesign, k: f64, n: &mut NoteRt, r: &mut [f64]) {
        let mut to_case = 0.0;
        if n.key_phase == 1 || n.key_phase == 3 {
            let bottom = felt(n.ky - a.key_travel, n.kv, a.felt_k, a.felt_p, a.felt_c, RAIL_REFERENCE);
            let rest = felt(-n.ky, -n.kv, a.felt_k, a.felt_p, a.felt_c, RAIL_REFERENCE);
            let f = n.k_force - a.up_weight - bottom + rest;
            n.kv += k * f / a.key_mass;
            n.ky += k * n.kv;
            let stop = bottom + rest;
            n.k_slow += (stop - n.k_slow) * k / 0.02; // the steady part is carried by the case without sound
            to_case += stop - n.k_slow;
            // Settled on a felt: stop following it.
            if n.kv.abs() < 1e-4 && (stop - n.k_slow).abs() < 1e-3 * (1.0 + stop) && (n.ky > a.key_travel || n.ky < 1e-4) {
                n.key_phase = if n.ky > a.key_travel { 2 } else { 0 };
                n.k_slow = 0.0;
            }
        }
        if n.returning {
            let place = if n.key_down { a.check_distance } else { a.rest_distance };
            let stop = felt(n.hy - place, n.hv, a.stop_k, a.stop_p, a.stop_c, STOP_REFERENCE);
            n.hv += k * (if a.gravity { 9.81 } else { 0.0 } - stop / n.h_total);
            n.hy += k * n.hv;
            n.h_slow += (stop - n.h_slow) * k / 0.02;
            to_case += stop - n.h_slow;
            if n.hv.abs() < 1e-3 && n.hy > place && (stop - n.h_slow).abs() < 1e-3 * (1.0 + stop) {
                n.returning = false;
                n.checked = n.key_down;
                n.h_slow = 0.0;
            }
        }
        if n.thump > 0.0 && to_case != 0.0 {
            let f = a.case_share * to_case * k * k;
            for q in 0..4 {
                r[n.thump_at.idx[q]] += n.thump_at.w[q] * f;
            }
        }
    }

    /// Felt contact between one hammer and the strings of its note. The force is the discrete
    /// gradient of the felt potential, which makes the contact exactly energy consistent; the
    /// resulting nonlinear system is monotone and solved by a damped Newton iteration.
    fn step_hammer(&mut self, ni: usize) {
        let mut n = self.notes[ni].clone();
        let k = self.k;
        let ns = n.count;
        let kf = n.h_k * if self.soft { self.action.soft_pedal_hardness } else { 1.0 };
        let p = n.h_p;
        let b = k * k / n.h_mass;
        let skip = if self.soft && ns == 3 { Some(0) } else { None }; // una corda: the shifted hammer misses the first string
        if self.action.gravity {
            n.veta -= 9.81 * k * k;
            if n.s_mass > 0.0 {
                n.vxi -= 9.81 * k * k;
            }
        }
        if n.s_mass > 0.0 {
            // the shank pulls the head and the head pulls the shank
            let f = n.s_k * (n.xi - n.eta) + n.s_c * (n.vxi - n.veta) / k;
            n.veta += k * k * f / n.h_mass;
            n.vxi -= k * k * f / n.s_mass;
            n.xi += n.vxi;
        }
        let vnext = &self.sh.v[self.vb() ^ 1]; // increments the strings would make without the hammer
        let (mut zm, mut zn, mut c, mut a, mut psi, mut r) = ([0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3]);
        let (mut force, mut dforce) = ([0.0f64; 3], [0.0f64; 3]);
        let mut contact = false;
        let mut prev = 0.0;
        for i in 0..ns {
            let s = &self.strings()[n.first + i];
            let gu = self.along(&n, i, &self.sh.u);
            let gv = self.along(&n, i, vnext);
            zn[i] = n.eta - gu;
            zm[i] = s.zeta_prev;
            c[i] = zn[i] + n.veta - gv - zm[i];
            a[i] = s.ham_a;
            psi[i] = n.h_mu * kf * zn[i].max(0.0).powf(p) / (2.0 * k);
            r[i] = c[i] - a[i] * s.force;
            prev += s.force;
            if Some(i) != skip && (zm[i] > 0.0 || zn[i] > 0.0 || zm[i] + c[i] > 0.0) {
                contact = true;
            }
        }
        let mut total = 0.0;
        if contact {
            let phi = |z: f64| if z > 0.0 { kf / (p + 1.0) * z.powf(p + 1.0) } else { 0.0 };
            let dphi = |z: f64| if z > 0.0 { kf * z.powf(p) } else { 0.0 };
            let ddphi = |z: f64| if z > 0.0 { kf * p * z.powf(p - 1.0) } else { 0.0 };
            let eval = |rr: &[f64; 3], g: &mut [f64; 3], force: &mut [f64; 3], dforce: &mut [f64; 3], total: &mut f64| -> f64 {
                *total = 0.0;
                for i in 0..ns {
                    let zp = zm[i] + rr[i];
                    let (sec, dsec) = if rr[i].abs() < 1e-13 {
                        (dphi(zm[i] + 0.5 * rr[i]), 0.5 * ddphi(zm[i] + 0.5 * rr[i]))
                    } else {
                        let sec = (phi(zp) - phi(zm[i])) / rr[i];
                        (sec, (dphi(zp) - sec) / rr[i])
                    };
                    let mut f = sec + psi[i] * rr[i];
                    if Some(i) == skip {
                        f = 0.0;
                    }
                    if f > 0.0 {
                        force[i] = f;
                        dforce[i] = dsec + psi[i];
                    } else {
                        // felt cannot pull on the string
                        force[i] = 0.0;
                        dforce[i] = 0.0;
                    }
                    *total += force[i];
                }
                let mut worst = 0.0f64;
                for i in 0..ns {
                    g[i] = rr[i] + a[i] * force[i] + b * *total - c[i];
                    worst = worst.max(g[i].abs());
                }
                worst
            };
            for i in 0..ns {
                r[i] -= b * prev;
            }
            let (mut g, mut rt, mut gt) = ([0.0; 3], [0.0; 3], [0.0; 3]);
            let mut err = eval(&r, &mut g, &mut force, &mut dforce, &mut total);
            let mut it = 0;
            while it < 60 {
                let mut scale = 0.0f64;
                for i in 0..ns {
                    scale = scale.max(c[i].abs() + r[i].abs());
                }
                // Rounding in the secant of the felt potential limits what can be reached.
                if err <= 1e-11 * scale + 1e-22 {
                    break;
                }
                let (mut t, mut den, mut dd) = (0.0, 1.0, [0.0; 3]);
                for i in 0..ns {
                    dd[i] = 1.0 + a[i] * dforce[i];
                    t += dforce[i] * g[i] / dd[i];
                    den += b * dforce[i] / dd[i];
                }
                let mut step = 1.0;
                let mut e2 = 0.0;
                let mut improved = false;
                for _ in 0..12 {
                    for i in 0..ns {
                        rt[i] = r[i] - step * (g[i] - b * t / den) / dd[i];
                    }
                    e2 = eval(&rt, &mut gt, &mut force, &mut dforce, &mut total);
                    if e2 < err {
                        improved = true;
                        break;
                    }
                    step *= 0.5;
                }
                if !improved {
                    break; // at the rounding floor
                }
                r = rt;
                g = gt;
                err = e2;
                it += 1;
            }
            self.hammer_iters_max = self.hammer_iters_max.max(it);
            eval(&r, &mut g, &mut force, &mut dforce, &mut total);
            for i in 0..ns {
                r[i] = c[i] - a[i] * force[i] - b * total; // exact for the forces used
            }
        } else {
            r = c;
        }

        // Observe the actual solved interval forces before the rebound path clears s.force.
        self.action_observer.force(ni,self.step_count,force,total);
        let mut away = true;
        for i in 0..ns {
            for pol in 0..if n.twin { 2 } else { 1 } {
                let sp = &self.strings()[n.first + i + pol * n.count];
                let v = self.nodes(vnext, sp);
                let q = sp.ham_w * k * k * force[i] / (sp.m_node * (1.0 + sp.s0k));
                v[sp.ham_l] += sp.ham_g[0] * q;
                v[sp.ham_l + 1] += sp.ham_g[1] * q;
            }
            let s = &mut self.strings_mut()[n.first + i];
            s.force = force[i];
            s.zeta_prev = zn[i];
            s.zeta = zm[i] + r[i];
            if s.zeta > -2e-3 {
                away = false;
            }
        }
        n.veta -= b * total;
        n.eta += n.veta;
        if away && n.veta < 0.0 {
            // the hammer is on its way down, clear of the strings
            n.hammer_active = false;
            n.returning = true;
            n.checked = false;
            n.hy = -self.strings()[n.first].zeta;
            n.hv = -n.veta / k;
            let observed_zeta=self.strings()[n.first].zeta;
            self.action_observer.returning(ni,self.step_count+1,n.eta,n.hy,observed_zeta);
            n.h_slow = 0.0;
            for s in &mut self.strings_mut()[n.first..n.first + ns] {
                s.force = 0.0;
            }
        }
        self.notes[ni] = n;
    }

    fn housekeeping(&mut self) {
        // A string that has sat under its damper for a while and has stopped moving on its own
        // is put to sleep. From then on the bridge sees it as a plain dashpot. "On its own" matters:
        // while the board still rings it keeps shaking the damped string by about the bridge's own
        // displacement, and that forced motion is exactly what the dashpot stands for.
        let settle = (0.15 * self.fs / 64.0) as usize;
        let damp_max = self.sh.damp_max;
        let mut sounding = self.pulse_count > 0;
        for i in 0..self.strings().len() {
            let s = &mut self.strings_mut()[i];
            sounding = sounding || (s.awake && s.has_damper);
            if !s.awake || !s.attached {
                continue;
            }
            let hammer = self.notes[s.note].hammer_active;
            let u = self.nodes(&self.sh.u, s);
            let end = u[s.n];
            let deviation = |u: &[f64]| (1..s.n).map(|l| (u[l] - end * l as f64 / s.n as f64).abs()).fold(0.0f64, f64::max);
            if s.sideways {
                // The sideways motion of a string matters while it rings from a strike. Once it has
                // died away it is put aside, damper or no damper, until the next strike.
                if !hammer && deviation(u) < 2e-8 {
                    self.sleep(i);
                }
                continue;
            }
            if !s.has_damper {
                continue;
            }
            if s.damp_rate < damp_max || hammer {
                s.damped_steps = 0;
                continue;
            }
            s.damped_steps += 1;
            if s.damped_steps < settle {
                continue;
            }
            // Under its damper the string rests bent, so judge by motion, not by shape.
            let v = self.nodes(&self.sh.v[self.vb()], s);
            let v_end = v[s.n];
            s.end_peak = v_end.abs().max(0.97 * s.end_peak);
            let moving = (1..s.n).map(|l| (v[l] - v_end * l as f64 / s.n as f64).abs()).fold(0.0f64, f64::max);
            if moving < 1e-6 * self.k + 3.0 * s.end_peak && !self.never_sleep {
                self.sleep(i);
            }
        }
        // Helpers are worth calling in whenever anything is sounding: the plate work is shared too.
        if !self.threads.is_empty() {
            self.quiet_checks = if sounding { 0 } else { self.quiet_checks.saturating_add(1) };
            self.sh.active.store(self.quiet_checks < 1500, Ordering::Release); // about 2 s of ring-out
        }
    }

    /// One internal time step; returns the two ear pressures in pascal at the internal rate.
    pub fn step(&mut self) -> (f64, f64) {
        if self.pristine == Pristine::Zero {
            // All dynamic buffers are an exact zero fixed point, proven by the first
            // real tick. Only clocks advance. The process/FIR loop remains unchanged.
            unsafe { (&mut *self.sh.radiation.get()).advance_pristine_zero() };
            self.sh.vb.fetch_xor(1, Ordering::Relaxed);
            self.step_count += 1;
            if self.step_count & 63 == 0 {
                self.housekeeping();
            }
            return unsafe { *self.sh.rad_out.get() };
        }
        // A plain reference that does not tie up `self`: the shared state outlives this call.
        let sh: &Shared = unsafe { &*Arc::as_ptr(&self.sh) };
        let k = self.k;
        let shared = !self.threads.is_empty() && sh.active.load(Ordering::Relaxed);
        // One shared pass per sample; hammer-contact strings finish serially below.
        let post = || {
            if shared {
                sh.done.store(0, Ordering::Release);
                let generation = (sh.ticket.load(Ordering::Relaxed) >> 34) + 1;
                sh.ticket.store(generation << 34, Ordering::Release);
            } else {
                for c in 0..sh.chunks.len() {
                    unsafe { sh.chunk_work(c) };
                }
            }
        };
        // Finish a shared pass: take whatever chunks are left, then wait for the helpers' ones.
        let finish = || {
            if !shared {
                return;
            }
            while let Some(c) = sh.claim() {
                unsafe { sh.chunk_work(c) };
                sh.done.fetch_add(1, Ordering::Release);
            }
            let all = sh.chunks.len() as i32;
            while sh.done.load(Ordering::Acquire) < all {
                spin();
            }
            sh.ticket.store(NO_JOB | (sh.ticket.load(Ordering::Relaxed) & !0x3_ffff_ffff), Ordering::Release);
        };

        post(); // strings, plate stencil and last step's sound, side by side
        finish();
        // From here to the next post only this thread touches the shared state.
        let (r, w, vw) = unsafe { (sh.r.all_mut(), sh.w.all_mut(), sh.vw.all_mut()) };
        sh.plate.apply_bridges(w, r);
        if sh.air_now.len > 0 {
            // same size: a plain copy, no allocation
            unsafe { sh.air_now.all_mut().copy_from_slice(sh.radiation.get().air_force()) };
        }
        let mut i = 0;
        while i < self.pulse_count {
            let pu = &mut self.pulses[i];
            let f = pu.peak * (PI * (pu.step as f64 + 0.5) / pu.length as f64).sin() * k * k;
            for q in 0..4 {
                r[pu.at.idx[q]] += pu.at.w[q] * f;
            }
            pu.step += 1;
            if pu.step >= pu.length {
                self.pulse_count -= 1;
                self.pulses[i] = self.pulses[self.pulse_count];
            } else {
                i += 1;
            }
        }
        if self.pedal_hiss > 1e-4 {
            // friction in the pedal work: a faint, quickly fading rustle on the case
            self.rng = self.rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let noise = ((self.rng >> 8) as f64 * (1.0 / 8388608.0) - 1.0) * 0.02 * self.mech_noise * self.pedal_hiss;
            for q in 0..4 {
                r[self.pedal_at.idx[q]] += self.pedal_at.w[q] * noise * k * k;
            }
            self.pedal_hiss *= 1.0 - 30.0 * k;
        }
        if self.ext_f != 0.0 {
            for q in 0..4 {
                r[self.ext_at.idx[q]] += self.ext_at.w[q] * self.ext_f * k * k;
            }
            self.ext_f = 0.0;
        }
        {
            let ef = unsafe { sh.end_force.all() };
            for (i, &f) in ef.iter().enumerate() {
                if f == 0.0 {
                    continue;
                }
                let at = &self.sh.end_at[i];
                for q in 0..at.n {
                    r[at.idx[q]] += at.w[q] * f;
                }
            }
        }
        for ni in 0..self.notes.len() {
            if self.notes[ni].hammer_active {
                self.step_hammer(ni);
                let n = &self.notes[ni];
                for s in &mut self.strings_mut()[n.first..n.first + n.total] {
                    unsafe { sh.advance_string(s, self.vb() ^ 1) };
                    s.defer_advance = n.hammer_active;
                }
            }
            // This reference is observed AFTER string advance but BEFORE return integration.
            if self.action_observer.needs_reference(ni) {
                let n=&self.notes[ni];
                let along0=self.along(n,0,&self.sh.u);
                let hy=n.hy;
                self.action_observer.return_reference(along0,hy);
            }
            let n = &mut self.notes[ni];
            let lifted = n.ky > self.action.key_at_damper;
            if n.key_phase == 1 || n.key_phase == 3 || n.returning {
                Self::step_action(&self.action, k, n, r);
            }
            if lifted != (n.ky > self.action.key_at_damper) { self.refresh_damper(ni); }
        }
        sh.vb.fetch_xor(1, Ordering::Relaxed);
        sh.plate.solve_stiff(r, &mut self.solve_work);
        {
            // Advance the plate and keep what radiates for the next step's radiation task.
            let keep = unsafe { sh.rad_in.all_mut() };
            let scale = sh.plate.diag_scale();
            for sp in &sh.plate.spans {
                let range = sp.start..sp.start + sp.len;
                plate_advance(&r[range.clone()], &scale[range.clone()], &mut vw[range.clone()], &mut w[range.clone()], &mut keep[range]);
            }
        }
        let out = unsafe { *sh.rad_out.get() };
        self.step_count += 1;
        if self.step_count & 63 == 0 {
            self.housekeeping();
        }
        if self.pristine == Pristine::Candidate {
            self.pristine = if self.zero_tick(out) { Pristine::Zero } else { Pristine::Disabled };
        }
        // Observe the COMPLETED step, including vb flip, plate work and housekeeping sleep.
        let pose=if self.action_observer.needs_pose(self.step_count) {
            Some(self.observed_action_pose(self.step_count))
        } else { None };
        self.action_observer.finish_step(self.step_count,pose);
        out
    }

    // Called once, after a genuine normal tick exercised the private plate factors,
    // coupling coefficients and radiation filters. Equality rejects NaN/Inf as well as
    // nonzero values, without a separate validation mirror of every static coefficient.
    fn zero_tick(&self, out: (f64, f64)) -> bool {
        let sh = &self.sh;
        let zero = |s: &[f64]| s.iter().all(|&x| x == 0.0);
        let buffers = [&sh.u, &sh.v[0], &sh.v[1], &sh.xi, &sh.vxi, &sh.end_force,
            &sh.w, &sh.vw, &sh.r, &sh.rad_in, &sh.air_now];
        let rad_out = unsafe { *sh.rad_out.get() };
        out.0 == 0.0 && out.1 == 0.0 && rad_out.0 == 0.0 && rad_out.1 == 0.0
            && buffers.iter().all(|b| zero(unsafe { b.all() }))
            && zero(&self.solve_work) && self.hist.iter().all(|h| zero(h))
            && self.notes.iter().all(|n| !n.hammer_active && !n.key_down && !n.sost
                && n.key_phase == 0 && !n.returning && !n.checked
                && zero(&[n.eta, n.veta, n.xi, n.vxi, n.ky, n.kv, n.k_force,
                    n.k_slow, n.hy, n.hv, n.h_slow]))
            && self.strings().iter().all(|s| !(s.awake && s.has_damper)
                && !s.defer_advance && !s.l_on && s.damped_steps == 0
                && s.damp_rate == s.damp_target
                && zero(&[s.zeta, s.zeta_prev, s.force, s.end_peak, s.l_peak, s.tension_change]))
            && self.pulse_count == 0 && self.ext_f == 0.0
            && self.pedal_hiss == 0.0 && self.pedal_speed == 0.0
            && unsafe { (&*sh.radiation.get()).is_pristine_zero() }
    }

    /// Renders stereo frames. Allocation and lock free.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let _flush = FlushDenormals::new();
        // Call the helpers in for the duration of this block.
        let in_block = !self.threads.is_empty() && self.sh.active.load(Ordering::Relaxed);
        if in_block {
            self.sh.in_block.store(true, Ordering::Release);
            self.sh.wake.notify_all();
        }
        for f in 0..left.len().min(right.len()) {
            let (mut l, mut r) = (0.0, 0.0);
            if self.os == 1 {
                (l, r) = self.step();
            } else {
                let len = self.fir.len();
                for _ in 0..self.os {
                    let (a, b) = self.step();
                    self.hist[0][self.hist_pos] = a;
                    self.hist[1][self.hist_pos] = b;
                    self.hist_pos = (self.hist_pos + 1) % len;
                }
                for i in 0..len {
                    let mut j = self.hist_pos + i;
                    if j >= len {
                        j -= len;
                    }
                    l += self.fir[i] * self.hist[0][j];
                    r += self.fir[i] * self.hist[1][j];
                }
            }
            left[f] = (l * self.out_gain) as f32;
            right[f] = (r * self.out_gain) as f32;
        }
        if in_block {
            self.sh.in_block.store(false, Ordering::Release);
        }
    }

    /// Helper threads that share the work of a step. They sleep between blocks and while the
    /// instrument is silent. `on_start` and `on_exit` run once on each helper thread, at its
    /// start and just before it ends (for example to join and leave an audio workgroup).
    pub fn set_helper_threads(&mut self, count: usize, on_start: Option<ThreadHook>, on_exit: Option<ThreadHook>) {
        if !self.threads.is_empty() {
            self.sh.stop.store(true, Ordering::Release);
            drop(self.sh.park.lock());
            self.sh.wake.notify_all();
            for t in self.threads.drain(..) {
                let _ = t.join();
            }
            self.sh.stop.store(false, Ordering::Release);
            self.sh.active.store(false, Ordering::Release);
        }
        for _ in 0..count {
            let sh = self.sh.clone();
            let (on_start, on_exit) = (on_start.clone(), on_exit.clone());
            self.threads.push(std::thread::spawn(move || {
                request_performance_core();
                if let Some(f) = &on_start {
                    f();
                }
                let _flush = FlushDenormals::new();
                while !sh.stop.load(Ordering::Relaxed) {
                    if !sh.active.load(Ordering::Acquire) || !sh.in_block.load(Ordering::Acquire) {
                        // Between blocks, and while there is nothing to share, sleep. A missed
                        // wake-up only means sitting out one block.
                        let guard = sh.park.lock().unwrap();
                        let _ = sh.wake.wait_timeout_while(guard, std::time::Duration::from_millis(50), |_| {
                            !(sh.stop.load(Ordering::Relaxed) || (sh.active.load(Ordering::Relaxed) && sh.in_block.load(Ordering::Relaxed)))
                        });
                        continue;
                    }
                    if let Some(c) = sh.claim() {
                        unsafe { sh.chunk_work(c) };
                        sh.done.fetch_add(1, Ordering::Release);
                    } else {
                        spin();
                    }
                }
                if let Some(f) = &on_exit {
                    f();
                }
            }));
        }
        if count > 0 {
            self.sh.active.store(true, Ordering::Release); // ready for the very first block
            self.quiet_checks = 0;
        }
    }

    pub fn helpers_busy(&self) -> bool {
        !self.threads.is_empty() && self.sh.active.load(Ordering::Relaxed)
    }

    // ------------------------------------------------------------ diagnostics and views

    /// The discrete energy. Evaluated directly from energy densities, independently of the
    /// stepping stencils. With x = 2 q - v (q, v at the newest step): H = KE + (V2(x) - V2(v)) / 8.
    pub fn energy(&self) -> f64 {
        let p = &self.sh.plate;
        let k = self.k;
        let (w, vw) = unsafe { (self.sh.w.all(), self.sh.vw.all()) };
        let xg: Vec<f64> = (0..p.grid_size).map(|g| 2.0 * w[g] - vw[g]).collect();
        let (mut xp, mut vp) = (vec![0.0; p.n], vec![0.0; p.n]);
        p.gather(&xg, &mut xp);
        p.gather(vw, &mut vp);
        let mut kin: f64 = (0..p.n).map(|a| p.mass[a] * vp[a] * vp[a]).sum();
        let (mut v2x, mut v2v) = (p.k.quad(&xp), p.k.quad(&vp));
        kin += 0.25 * k * k * p.k_imp.quad(&vp) - 0.5 * k * p.d.quad(&vp);

        for (i, s) in self.strings().iter().enumerate() {
            if !s.awake {
                continue;
            }
            let n = s.n;
            let u = self.nodes(&self.sh.u, s);
            let v = self.nodes(&self.sh.v[self.vb()], s);
            let (mut x, mut vv) = (vec![0.0; n + 1], vec![0.0; n + 1]);
            for l in 1..n {
                vv[l] = v[l];
                x[l] = 2.0 * u[l] - vv[l];
            }
            if s.attached {
                vv[n] = self.sh.end_at[i].eval(vw);
                x[n] = self.sh.end_at[i].eval(&xg);
            }
            let v2 = |y: &[f64]| {
                let mut e = 0.0;
                for l in 0..n {
                    e += s.t_h * (y[l + 1] - y[l]) * (y[l + 1] - y[l]);
                }
                for l in 1..n {
                    let c = y[l - 1] - 2.0 * y[l] + y[l + 1];
                    e += s.ei_h3 * c * c;
                }
                e
            };
            v2x += v2(&x);
            v2v += v2(&vv);
            for l in 1..n {
                kin += s.m_node * vv[l] * vv[l];
            }
            let dis: f64 = (0..n).map(|l| s.damp_h * (vv[l + 1] - vv[l]) * (vv[l + 1] - vv[l])).sum();
            kin -= 0.5 * k * dis;
        }
        let mut h = 0.5 * kin / (k * k) + (v2x - v2v) / 8.0;
        for n in &self.notes {
            if !n.hammer_active {
                continue;
            }
            h += 0.5 * n.h_mass * (n.veta / k) * (n.veta / k);
            if n.s_mass > 0.0 {
                let d_new = n.xi - n.eta;
                let d_old = d_new - (n.vxi - n.veta);
                h += 0.5 * n.s_mass * (n.vxi / k) * (n.vxi / k) + 0.5 * n.s_k * d_new * d_old;
            }
            let phi = |z: f64| if z > 0.0 { n.h_k / (n.h_p + 1.0) * z.powf(n.h_p + 1.0) } else { 0.0 };
            for s in &self.strings()[n.first..n.first + n.count] {
                h += 0.5 * (phi(s.zeta) + phi(s.zeta_prev));
            }
        }
        h
    }

    pub fn dt(&self) -> f64 {
        self.k
    }
    pub fn plate(&self) -> &Plate {
        &self.sh.plate
    }
    pub fn reverb_time(&self) -> f64 {
        unsafe { self.sh.radiation.get().reverb_time }
    }
    /// Plate displacement, grid storage.
    pub fn plate_u(&self) -> &[f64] {
        unsafe { self.sh.w.all() }
    }
    pub fn plate_u_mut(&mut self) -> &mut [f64] {
        self.pristine = Pristine::Disabled;
        unsafe { self.sh.w.all_mut() }
    }
    pub fn plate_v_mut(&mut self) -> &mut [f64] {
        self.pristine = Pristine::Disabled;
        unsafe { self.sh.vw.all_mut() }
    }
    pub fn string_count(&self) -> usize {
        self.strings().len()
    }
    pub fn awake_strings(&self) -> usize {
        self.strings().iter().filter(|s| s.awake).count()
    }
    pub fn total_string_nodes(&self) -> usize {
        self.strings().iter().map(|s| s.n - 1).sum()
    }
    pub fn hammer_iterations_max(&self) -> usize {
        self.hammer_iters_max
    }
    pub fn hammer_active(&self, midi: i32) -> bool {
        self.note_of_midi[(midi & 127) as usize].map_or(false, |n| self.notes[n].hammer_active)
    }
    /// Displacement at nodes 0..=N of a string.
    pub fn string_u(&self, index: usize) -> &[f64] {
        self.nodes(&self.sh.u, &self.strings()[index])
    }
    pub fn string_u_mut(&mut self, index: usize) -> &mut [f64] {
        self.pristine = Pristine::Disabled;
        self.nodes(&self.sh.u, &self.strings()[index])
    }
    /// Per-step increment at nodes 0..=N of a string.
    pub fn string_v_mut(&mut self, index: usize) -> &mut [f64] {
        self.pristine = Pristine::Disabled;
        self.nodes(&self.sh.v[self.vb()], &self.strings()[index])
    }
    pub fn string_nodes(&self, index: usize) -> usize {
        self.strings()[index].n + 1
    }
    pub fn string_node_mass(&self, index: usize) -> f64 {
        self.strings()[index].m_node
    }
    pub fn string_tension(&self, index: usize) -> f64 {
        self.strings()[index].tension
    }
    pub fn string_midi(&self, index: usize) -> i32 {
        self.notes[self.strings()[index].note].midi
    }
    pub fn string_bridge(&self, index: usize) -> Vec2 {
        self.strings()[index].bridge
    }
    pub fn hammer_force(&self, index: usize) -> f64 {
        self.strings()[index].force
    }
    pub fn tension_change(&self, index: usize) -> f64 {
        self.strings()[index].tension_change
    }
    pub fn longitudinal_cells(&self, index: usize) -> usize {
        let s = &self.strings()[index];
        if s.l_on {
            s.l_cells
        } else {
            0
        }
    }
    /// Indices of the struck strings of a note.
    pub fn strings_of_note(&self, midi: i32) -> Vec<usize> {
        self.note_of_midi[(midi & 127) as usize].map_or(Vec::new(), |n| (self.notes[n].first..self.notes[n].first + self.notes[n].count).collect())
    }
    /// Numerical partial frequency p of a string with rigid ends (closed form of the scheme).
    pub fn discrete_partial(&self, index: usize, p: usize) -> f64 {
        let s = &self.strings()[index];
        let sp = (p as f64 * PI / (2.0 * s.n as f64)).sin().powi(2);
        let x = s.a_t * sp + 4.0 * s.a_b * sp * sp; // sin^2(omega dt / 2)
        if x >= 1.0 {
            0.0
        } else {
            x.sqrt().asin() / (PI * self.k)
        }
    }
    /// The same partial of the continuous stiff string the grid is approximating.
    pub fn continuum_partial(&self, index: usize, p: usize) -> f64 {
        let s = &self.strings()[index];
        p as f64 * s.f0 * ((1.0 + s.inharm * (p * p) as f64) / (1.0 + s.inharm)).sqrt()
    }
    pub fn fit_partial(&self, index: usize) -> usize {
        self.strings()[index].fit_partial
    }
    /// External point force on the board for the next step only (measurement hook).
    pub fn push_board(&mut self, piano_xy: Vec2, newtons: f64) {
        self.pristine = Pristine::Disabled;
        self.ext_at = self.sh.plate.interp(piano_xy);
        self.ext_f = newtons;
    }
    pub fn board_velocity(&self, piano_xy: Vec2) -> f64 {
        self.sh.plate.interp(piano_xy).eval(unsafe { self.sh.vw.all() }) / self.k
    }

    /// This reads only stable serial state, after the corresponding strings/action advance.
    fn observed_action_pose(&self, step:u64) -> ActionPose {
        let n=&self.notes[self.action_observer.focus().expect("observed real stroke")];
        let force=self.action_observer.force_observation();
        let mut along=[0.0;3];
        for (i,value) in along.iter_mut().enumerate().take(n.count) {
            *value=self.along(n,i,&self.sh.u);
        }
        let phase=if n.hammer_active {ActionPhase::Flight} else if n.returning {ActionPhase::Returning}
            else if n.checked {ActionPhase::Checked} else {ActionPhase::Rest};
        let mut flags=0;
        if force.observed { flags|=FORCE_OBSERVED; }
        if n.key_down { flags|=KEY_DOWN; }
        if n.hammer_active && n.s_mass>0.0 { flags|=SHANK_OBSERVED; }
        ActionPose { pose_step:step, force_solve_step:force.step,
            eta_m:n.eta, eta_velocity_mps:n.veta/self.k, xi_m:n.xi, xi_velocity_mps:n.vxi/self.k,
            hy_m:n.hy, hv_mps:n.hv, ky_m:n.ky, kv_mps:n.kv,
            string_along_m:along, force_n:force.wires,
            flags,
            key_phase:n.key_phase, phase }
    }

    pub fn new_snapshot(&self) -> Snapshot {
        let max_n = self.strings().iter().map(|s| s.n + 1).max().unwrap_or(0);
        Snapshot {
            board: vec![0.0; self.sh.plate.grid_size],
            string_level: vec![0.0; self.strings().len()],
            shape: [vec![0.0; max_n], vec![0.0; max_n], vec![0.0; max_n]],
            shape_nodes: [0; 3],
            hammer_force: [0.0; 3],
            focus_midi: -1,
            awake: 0,
            action: ActionTrace::default(),
            damper_lift: 0.0,
            damper_span: [0.0, 0.0],
            key_depression: 0.0,
            shank_flex_m: 0.0,
            felt_compression_m: 0.0,
            key_hammer_lift: [0.0; 88],
            key_damper_lift: [0.0; 88],
        }
    }

    pub fn fill_snapshot(&self, snap: &mut Snapshot, focus_midi: i32) {
        // Independent observed-note identity; copy even if the legacy waveform focus is absent.
        snap.action=self.action_observer.trace;
        let w = unsafe { self.sh.w.all() };
        for (dst, src) in snap.board.iter_mut().zip(w) {
            *dst = *src as f32;
        }
        snap.awake = 0;
        for (i, s) in self.strings().iter().enumerate() {
            let mut m = 0.0f32;
            if s.awake {
                snap.awake += 1;
                let u = self.nodes(&self.sh.u, s);
                for l in 1..s.n {
                    m = m.max(u[l].abs() as f32);
                }
            }
            snap.string_level[i] = m;
        }
        // Populate real per-key hammer & damper elevations for the receding perspective bank.
        snap.key_hammer_lift.fill(0.0);
        snap.key_damper_lift.fill(0.0);
        let rest_dist = self.action.rest_distance.max(1e-4);
        let damp_max = self.sh.damp_max.max(1e-6);
        let strings = self.strings();
        for n in &self.notes {
            if (21..=108).contains(&n.midi) {
                let k = (n.midi - 21) as usize;
                let h_lift = if n.hammer_active {
                    ((n.eta + rest_dist) / rest_dist).clamp(0.0, 1.05) as f32
                } else if n.returning || n.checked {
                    ((rest_dist - n.hy) / rest_dist).clamp(0.0, 1.0) as f32
                } else {
                    0.0
                };
                snap.key_hammer_lift[k] = h_lift;
                if n.count > 0 {
                    let s0 = &strings[n.first];
                    snap.key_damper_lift[k] = if s0.has_damper {
                        (1.0 - s0.damp_rate / damp_max).clamp(0.0, 1.0) as f32
                    } else {
                        1.0
                    };
                }
            }
        }
        snap.focus_midi = focus_midi;
        snap.shape_nodes = [0; 3];
        snap.hammer_force = [0.0; 3];
        snap.damper_lift = 0.0;
        snap.damper_span = [0.0, 0.0];
        snap.key_depression = 0.0;
        snap.shank_flex_m = 0.0;
        snap.felt_compression_m = 0.0;
        let Some(ni) = (if focus_midi >= 0 { self.note_of_midi[(focus_midi & 127) as usize] } else { None }) else { return };
        let n = &self.notes[ni];
        snap.key_depression = (n.ky / self.action.key_travel.max(1e-6)).clamp(0.0, 1.0) as f32;
        if n.hammer_active && n.s_mass > 0.0 {
            snap.shank_flex_m = (n.eta - n.xi) as f32;
        }
        let mut max_comp = 0.0f64;
        for i in 0..n.count {
            let s = &strings[n.first + i];
            let u = self.nodes(&self.sh.u, s);
            snap.shape_nodes[i] = s.n + 1;
            for l in 0..=s.n {
                snap.shape[i][l] = if s.awake { u[l] as f32 } else { 0.0 };
            }
            snap.hammer_force[i] = s.force as f32;
            if i == 0 {
                if s.has_damper && s.n > 0 {
                    snap.damper_lift = (1.0 - s.damp_rate / damp_max).clamp(0.0, 1.0) as f32;
                    snap.damper_span = [s.dmp0 as f32 / s.n as f32, s.dmp1 as f32 / s.n as f32];
                } else {
                    snap.damper_lift = 1.0;
                    snap.damper_span = [0.0, 0.0];
                }
            }
            if n.hammer_active && s.force > 0.0 {
                let y_str = s.ham_g[0] * u[s.ham_l] + s.ham_g[1] * u[s.ham_l + 1];
                max_comp = max_comp.max((n.eta - y_str).max(0.0));
            }
        }
        snap.felt_compression_m = max_comp as f32;
    }
}

impl Drop for Piano {
    fn drop(&mut self) {
        self.set_helper_threads(0, None, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same_state(a: &Piano, b: &Piano) {
        same_physical_state(a, b);
        assert_eq!(a.quiet_checks, b.quiet_checks);
        assert_eq!(a.sh.active.load(Ordering::Relaxed), b.sh.active.load(Ordering::Relaxed));
    }

    fn same_physical_state(a: &Piano, b: &Piano) {
        let bits = |s: &[f64]| s.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        for (x, y) in [(&a.sh.u, &b.sh.u), (&a.sh.v[0], &b.sh.v[0]), (&a.sh.v[1], &b.sh.v[1]),
            (&a.sh.w, &b.sh.w), (&a.sh.vw, &b.sh.vw), (&a.sh.r, &b.sh.r),
            (&a.sh.rad_in, &b.sh.rad_in), (&a.sh.air_now, &b.sh.air_now),
            (&a.sh.end_force, &b.sh.end_force), (&a.sh.xi, &b.sh.xi), (&a.sh.vxi, &b.sh.vxi)] {
            assert_eq!(bits(unsafe { x.all() }), bits(unsafe { y.all() }));
        }
        assert_eq!(a.step_count, b.step_count);
        assert_eq!(a.vb(), b.vb());
        assert_eq!(a.hist_pos, b.hist_pos);
        assert_eq!(bits(&a.solve_work), bits(&b.solve_work));
        for i in 0..2 { assert_eq!(bits(&a.hist[i]), bits(&b.hist[i])); }
        assert_eq!(a.rng, b.rng);
        assert_eq!(a.pedal_step, b.pedal_step);
        assert_eq!(a.sostenuto_down, b.sostenuto_down);
        assert_eq!(a.pulse_count, b.pulse_count);
        assert_eq!(a.awake_strings(), b.awake_strings());
        for (a, b) in a.strings().iter().zip(b.strings()) {
            assert_eq!((a.awake, a.defer_advance, a.l_on, a.damped_steps),
                (b.awake, b.defer_advance, b.l_on, b.damped_steps));
            assert_eq!(bits(&[a.damp_rate, a.damp_target, a.damp_land, a.zeta, a.zeta_prev,
                a.force, a.end_peak, a.l_peak, a.tension_change]),
                bits(&[b.damp_rate, b.damp_target, b.damp_land, b.zeta, b.zeta_prev,
                    b.force, b.end_peak, b.l_peak, b.tension_change]));
        }
        for (a, b) in a.notes.iter().zip(&b.notes) {
            assert_eq!((a.hammer_active, a.key_down, a.sost, a.key_phase, a.returning, a.checked),
                (b.hammer_active, b.key_down, b.sost, b.key_phase, b.returning, b.checked));
            assert_eq!(bits(&[a.eta, a.veta, a.xi, a.vxi, a.ky, a.kv, a.k_force, a.k_slow, a.hy, a.hv, a.h_slow]),
                bits(&[b.eta, b.veta, b.xi, b.vxi, b.ky, b.kv, b.k_force, b.k_slow, b.hy, b.hv, b.h_slow]));
        }
        assert_eq!(a.energy().to_bits(), b.energy().to_bits());
    }

    fn paired_render(fast: &mut Piano, normal: &mut Piano, frames: usize) {
        let (mut a, mut b) = ([vec![0.0; frames], vec![0.0; frames]], [vec![0.0; frames], vec![0.0; frames]]);
        let [l, r] = &mut a;
        fast.process(l, r);
        let [l, r] = &mut b;
        normal.process(l, r);
        for (a, b) in a.iter().flatten().zip(b.iter().flatten()) {
            assert_eq!(a.to_bits(), b.to_bits(), "stereo sample bits");
        }
        same_state(fast, normal);
    }

    fn initial_noops(p: &mut Piano) {
        p.set_pedal(0.0);
        p.set_sustain(false);
        p.set_soft_pedal(false);
        p.set_sostenuto(false);
        for midi in 21..=108 { p.note_off(midi, 1.0); }
    }

    #[test]
    fn pristine_mixed_steps_fir_helpers_and_pedals_match_normal_bits() {
        for os in [1, 2, 4] {
            for (room, air) in [(false, false), (false, true), (true, false), (true, true)] {
                for helpers in [0, 3] {
                    let d = make_grand(&GrandOptions { first_key: 40, last_key: 88,
                        room_enabled: room, air_loading: air, ..GrandOptions::default() });
                    let o = PianoOptions { oversample: os, board_grid_scale: 2.0, ..PianoOptions::default() };
                    let mut fast = Piano::new(&d, &o).unwrap();
                    let mut normal = Piano::new(&d, &o).unwrap();
                    normal.pristine = Pristine::Disabled;
                    for p in [&mut fast, &mut normal] {
                        p.set_helper_threads(helpers, None, None);
                        initial_noops(p);
                    }
                    let lead = if os > 1 && room && air && helpers == 3 { 10003 } else {
                        [0, 1, 63, 64, 65, 4097][(os + helpers + room as usize + air as usize) % 6]
                    };
                    for _ in 0..lead {
                        let a = fast.step();
                        let b = normal.step();
                        assert_eq!((a.0.to_bits(), a.1.to_bits()), (b.0.to_bits(), b.1.to_bits()));
                    }
                    for frames in [1, 7, 65, 3, 127] {
                        paired_render(&mut fast, &mut normal, frames);
                        initial_noops(&mut fast);
                        initial_noops(&mut normal);
                    }
                    assert!(fast.pristine == Pristine::Zero, "verified no-ops disabled fast path");
                    for stage in 0..7 {
                        for p in [&mut fast, &mut normal] {
                            match stage {
                                0 => p.note_on(60, 0.3),
                                1 => p.set_sustain(true),
                                2 => { p.set_sostenuto(true); p.note_on(84, 5.0); }
                                3 => p.note_off(60, 1.0),
                                4 => { p.set_sustain(false); p.set_soft_pedal(true); }
                                5 => { p.set_sostenuto(false); p.note_off(84, 1.0); }
                                _ => p.push_board(Vec2 { x: 0.5, y: 0.5 }, 0.1),
                            }
                        }
                        paired_render(&mut fast, &mut normal, 257);
                        assert!(fast.pristine == Pristine::Disabled);
                    }
                    println!("pristine stereo/state bits: os={os}, room={room}, air={air}, helpers={helpers}");
                }
            }
        }
    }

    #[test]
    fn pristine_mutators_and_slice_exposure_disable_permanently() {
        let d = make_grand(&GrandOptions { first_key: 40, last_key: 88, ..GrandOptions::default() });
        let o = PianoOptions { board_grid_scale: 2.0, ..PianoOptions::default() };
        for proved in [false, true] {
        for case in 0..11 {
            let mut fast = Piano::new(&d, &o).unwrap();
            let mut normal = Piano::new(&d, &o).unwrap();
            normal.pristine = Pristine::Disabled;
            if proved {
                paired_render(&mut fast, &mut normal, 65);
                assert!(fast.pristine == Pristine::Zero);
            }
            for p in [&mut fast, &mut normal] {
                let node = p.plate().gid[0];
                let s = p.strings().iter().position(|s| !s.has_damper).unwrap();
                match case {
                    0 => p.note_on(60, 0.0),
                    1 => p.set_pedal(0.1), // Below damper lift: pedal hiss still advances RNG.
                    2 => p.set_soft_pedal(true),
                    3 => p.set_sostenuto(true),
                    4 => p.push_board(Vec2 { x: 0.5, y: 0.5 }, 1.0),
                    5 => p.plate_u_mut()[node] = 1e-10,
                    6 => p.plate_v_mut()[node] = 1e-10,
                    7 => p.string_u_mut(s)[1] = 1e-10,
                    8 => p.string_v_mut(s)[1] = 1e-10,
                    9 => p.push_board(Vec2 { x: 0.0, y: 0.0 }, 0.0),
                    _ => p.set_sustain(true),
                }
            }
            assert!(fast.pristine == Pristine::Disabled, "mutator {case}");
            paired_render(&mut fast, &mut normal, 257);
            for p in [&mut fast, &mut normal] { p.note_on(60, 2.0); p.note_off(60, 1.0); }
            paired_render(&mut fast, &mut normal, 257);
            assert!(fast.pristine == Pristine::Disabled);
        }
        }
        // Even a no-write slice borrow permanently invalidates; it is not a zero test.
        let mut p = Piano::new(&d, &o).unwrap();
        let _ = p.string_u_mut(0);
        p.process(&mut [0.0; 4], &mut [0.0; 4]);
        assert!(p.pristine == Pristine::Disabled);
    }

    #[test]
    fn pristine_rejects_awake_dampers_nonfinite_and_nonzero_tick() {
        for (ring, press, attached) in [(true, 0.1, true), (true, 0.0, true), (false, 0.1, false), (false, 0.0, false)] {
            let mut d = make_grand(&GrandOptions { first_key: 40, last_key: 40,
                damped_strings_ring: ring, damper_press_n: press, ..GrandOptions::default() });
            d.notes[0].strings[0].attached = attached;
            let mut p = Piano::new(&d, &PianoOptions::default()).unwrap();
            assert!(p.pristine == Pristine::Disabled);
            p.step();
            assert!(p.pristine == Pristine::Disabled);
        }
        let d = make_grand(&GrandOptions { first_key: 40, last_key: 40, ..GrandOptions::default() });
        for value in [1e-20, f64::NAN, f64::INFINITY] {
            let mut p = Piano::new(&d, &PianoOptions::default()).unwrap();
            // Simulate a bad derived coefficient/result before the one-time proof.
            unsafe { *p.sh.rad_out.get() = (value, 0.0) };
            assert!(!p.zero_tick((value, 0.0)));
            unsafe { p.sh.w.all_mut()[p.plate().gid[0]] = value };
            p.step();
            assert!(p.pristine == Pristine::Disabled);
        }
    }

    #[test]
    fn pristine_silence_matches_normal_ticks_and_first_note_bits() {
        let d = make_grand(&GrandOptions { first_key: 40, last_key: 88, ..GrandOptions::default() });
        let o = PianoOptions { board_grid_scale: 2.0, ..PianoOptions::default() };
        let mut fast = Piano::new(&d, &o).unwrap();
        let mut normal = Piano::new(&d, &o).unwrap();
        normal.pristine = Pristine::Disabled;
        for i in 0..10000 {
            let before = std::time::Instant::now();
            let a = fast.step();
            let fast_wall = before.elapsed();
            let before = std::time::Instant::now();
            let b = normal.step();
            if i == 0 {
                println!("one cold first tick: real tick + zero proof {:?}; forced-normal {:?} (wall snapshot, not a CPU sum)", fast_wall, before.elapsed());
            }
            assert_eq!((a.0.to_bits(), a.1.to_bits()), (b.0.to_bits(), b.1.to_bits()), "silent tick {i}");
            if [0, 1, 62, 63, 64, 9999].contains(&i) { same_state(&fast, &normal); }
        }
        assert!(fast.pristine == Pristine::Zero);
        for p in [&mut fast, &mut normal] {
            // Helpers may be installed after an arbitrary amount of pristine stepping.
            p.set_helper_threads(3, None, None);
            p.note_on(60, 2.0);
        }
        for i in 0..5000 {
            let a = fast.step();
            let b = normal.step();
            assert_eq!((a.0.to_bits(), a.1.to_bits()), (b.0.to_bits(), b.1.to_bits()), "active tick {i}");
        }
        same_state(&fast, &normal);
    }

    #[test]
    fn action_contact_coupled_strokes_are_finite_and_bounded() {
        use std::io::Write;
        for fs in [44100.0, 48000.0, 96000.0] {
            for rapid in [false, true] {
                let mut p = Piano::new(&make_grand(&GrandOptions::default()),
                    &PianoOptions { sample_rate: fs, board_grid_scale: 1.5, ..PianoOptions::default() }).unwrap();
                let ni = p.note_of_midi[60].unwrap();
                let mut peak = 0.0f64;
                let mut compression = [0.0f64; 3];
                let mut force_peak = [0.0f64; 3];
                let mut contacts = [0usize; 3];
                let mut previous = [false; 3];
                let mut diagnostic = std::env::var_os("FELT_ACTION_CONTACT_DIAGNOSTICS").map(|dir| {
                    let dir = std::path::PathBuf::from(dir);
                    std::fs::create_dir_all(&dir).unwrap();
                    let file = std::fs::File::create(dir.join(format!("{fs}-rapid-{rapid}.csv"))).unwrap();
                    let mut writer = std::io::BufWriter::new(file);
                    writeln!(writer, "time_s,lower_N,upper_N,hammer_N,case_N").unwrap();
                    writer
                });
                for frame in 0..(0.6 * fs) as usize {
                    if rapid {
                        for i in 0..4 {
                            if frame == (i as f64 * 0.05 * fs).round() as usize { p.note_on(60, 2.0); }
                            if frame == ((i as f64 * 0.05 + 0.025) * fs).round() as usize { p.note_off(60, 1.0); }
                        }
                    } else {
                        if frame == 0 { p.note_on(60, 2.0); }
                        if frame == (0.18 * fs).round() as usize { p.note_off(60, 1.0); }
                    }
                    let n = &p.notes[ni];
                    assert!([n.ky, n.kv, n.k_slow, n.hy, n.hv, n.h_slow].iter().all(|x| x.is_finite()));
                    let moving_key = n.key_phase == 1 || n.key_phase == 3;
                    let place = if n.key_down { p.action.check_distance } else { p.action.rest_distance };
                    let z = [if moving_key { n.ky - p.action.key_travel } else { 0.0 },
                        if moving_key { -n.ky } else { 0.0 }, if n.returning { n.hy - place } else { 0.0 }];
                    let f = [felt(z[0], n.kv, p.action.felt_k, p.action.felt_p, p.action.felt_c, RAIL_REFERENCE),
                        felt(z[1], -n.kv, p.action.felt_k, p.action.felt_p, p.action.felt_c, RAIL_REFERENCE),
                        felt(z[2], n.hv, p.action.stop_k, p.action.stop_p, p.action.stop_c, STOP_REFERENCE)];
                    if let Some(writer) = &mut diagnostic {
                        let key = if moving_key { (f[0] + f[1] - n.k_slow) * (1.0 - p.k / 0.02) } else { 0.0 };
                        let hammer = if n.returning { (f[2] - n.h_slow) * (1.0 - p.k / 0.02) } else { 0.0 };
                        let case_force = if n.thump > 0.0 { p.action.case_share * (key + hammer) } else { 0.0 };
                        writeln!(writer, "{},{},{},{},{}", frame as f64 / fs, f[0], f[1], f[2], case_force).unwrap();
                    }
                    for i in 0..3 {
                        assert!(f[i].is_finite());
                        compression[i] = compression[i].max(z[i]);
                        force_peak[i] = force_peak[i].max(f[i]);
                        let contact = z[i] > 0.0;
                        if contact && !previous[i] { contacts[i] += 1; }
                        previous[i] = contact;
                    }
                    let (l, r) = p.step();
                    assert!(l.is_finite() && r.is_finite());
                    peak = peak.max(l.abs()).max(r.abs());
                    if frame % 256 == 0 { assert!(p.energy().is_finite()); }
                }
                println!("coupled fs={fs}, rapid={rapid}: raw pressure peak={peak:.6} Pa, compression={compression:?} m, contact force peaks={force_peak:?} N, entries(lower,upper,hammer)={contacts:?}");
                assert!(peak < 100.0); // Pressure before output_gain; no output limiter.
                assert!(compression.iter().all(|&z| z < 0.003));
                assert!(force_peak.iter().all(|&f| f < 200.0));
                assert!(contacts.iter().all(|&n| n < 32), "unbounded recontacts");
            }
        }
    }

    #[test]
    fn string_kernel_leaves_prescribed_endpoints_and_ghosts_alone() {
        for n in [2, 3, 8, 200] {
            let u: Vec<f64> = (0..n + 3).map(|i| (i as f64).sin()).collect();
            let v = vec![0.0; n + 3];
            let mut next = vec![123.0; n + 3];
            string_kernel(&u, &v, &mut next, [1.0, 0.0, 0.0, 0.0, 0.0]);
            assert_eq!(&next[2..n + 1], &u[2..n + 1]);
            for i in [0, 1, n + 1, n + 2] {
                assert_eq!(next[i], 123.0, "N={n}, prescribed/ghost index {i}");
            }
        }
    }

    fn key_return_test_piano(first: i32, last: i32, fs: f64) -> Piano {
        let d = make_grand(&GrandOptions { first_key: first, last_key: last, key_thump: 0.0,
            mechanism_noise: 0.0, room_enabled: false, ..GrandOptions::default() });
        Piano::new(&d, &PianoOptions { sample_rate: fs, board_grid_scale: 1.5, ..PianoOptions::default() }).unwrap()
    }

    #[test]
    fn key_return_engagement_follows_geometry_and_release_velocity() {
        for midi in [21, 36, 60, 84, 89] {
            let mut delays = Vec::new();
            for release in [0.25, 1.0, 4.0] {
                let mut p = key_return_test_piano(midi - 20, midi - 20, 48000.0);
                p.note_on(midi, 2.0);
                for _ in 0..1440 { let (l, r) = p.step(); assert!(l.is_finite() && r.is_finite()); }
                let initial = p.notes[0].ky;
                p.note_off(midi, release);
                assert_eq!(p.notes[0].ky.to_bits(), initial.to_bits(), "Off must not teleport the key");
                let first = p.notes[0].first;
                assert!(p.strings()[first].has_damper);
                assert_eq!(p.strings()[first].damp_target, 0.0);
                let mut contact = None;
                for frame in 0..7200 {
                    let (l, r) = p.step(); assert!(l.is_finite() && r.is_finite());
                    assert_eq!(p.strings()[first].damp_land, release, "geometry refresh must preserve contact-rate proxy");
                    if p.strings()[first].damp_target > 0.0 {
                        assert!(p.notes[0].ky <= p.action.key_at_damper);
                        contact = Some((frame + 1) as f64 * p.k);
                        break;
                    }
                    assert!(p.notes[0].ky > p.action.key_at_damper);
                }
                let delay = contact.expect("regulated contact must be reached during real key return");
                assert!(delay > 0.0 && delay < 0.15);
                delays.push(delay);
                println!("keyreturn midi={midi} release={release} Offkey={initial:.9}m engagement={:.3}ms (grounded post-contact law unchanged)", delay * 1000.0);
            }
            assert!(delays[0] > delays[1] && delays[1] > delays[2]);
        }
    }

    #[test]
    fn key_return_support_ownership_and_sostenuto_edges_compose() {
        let mut p = key_return_test_piano(40, 42, 48000.0);
        p.note_on(60, 2.0);
        p.set_sostenuto(true);
        p.note_off(60, 1.0);
        p.set_sostenuto(true); // no recapture of the now-released key mask
        p.note_on(62, 2.0);
        p.set_sostenuto(true); // a newly pressed key is NOT captured on repeated true
        assert!(p.notes[0].sost && !p.notes[2].sost);
        for _ in 0..7200 { p.step(); }
        assert_eq!(p.strings()[p.notes[0].first].damp_target, 0.0);
        p.set_pedal(1.0);
        p.set_sostenuto(false); // sustain still owns support
        assert_eq!(p.strings()[p.notes[0].first].damp_target, 0.0);
        p.set_pedal(0.0);
        assert_eq!(p.strings()[p.notes[0].first].damp_target, p.sh.damp_max);
        assert_eq!(p.strings()[p.notes[2].first].damp_target, 0.0); // key still down
        p.note_off(62, 1.0);
        p.set_pedal(1.0); p.set_pedal(0.0); // cannot defeat the returning key lifter
        assert_eq!(p.strings()[p.notes[2].first].damp_target, 0.0);
        p.set_pedal(0.41);
        for _ in 0..7200 { let (l, r) = p.step(); assert!(l.is_finite() && r.is_finite()); }
        assert_eq!(p.strings()[p.notes[2].first].damp_target, p.sh.damp_max * p.pedal_damping);
        assert!(p.pedal_damping > 0.0 && p.pedal_damping < 1.0);
    }

    #[test]
    fn key_return_same_block_tap_keeps_flight_and_retrigger_clears_contact() {
        let mut p = key_return_test_piano(40, 40, 48000.0);
        p.note_on(60, 2.0);
        let flight = (p.notes[0].eta.to_bits(), p.notes[0].veta.to_bits(), p.notes[0].ky.to_bits());
        p.note_off(60, 1.0);
        assert!(p.notes[0].hammer_active);
        assert_eq!(flight, (p.notes[0].eta.to_bits(), p.notes[0].veta.to_bits(), p.notes[0].ky.to_bits()));
        let mut hit = false;
        for _ in 0..1440 {
            let (l, r) = p.step(); assert!(l.is_finite() && r.is_finite());
            hit |= p.strings().iter().any(|s| s.force > 0.0);
        }
        assert!(hit, "Off must not cancel a hammer already in flight");
        p.note_on(60, 1.0);
        for s in &p.strings()[p.notes[0].first..p.notes[0].first + p.notes[0].total] {
            if s.has_damper { assert_eq!((s.damp_target, s.damp_rate), (0.0, 0.0)); }
        }
    }

    #[test]
    fn key_return_undamped_release_remains_exact_with_noise_off() {
        for fs in [44100.0, 48000.0, 96000.0] {
            let mut held = key_return_test_piano(70, 70, fs);
            let mut off = key_return_test_piano(70, 70, fs);
            held.note_on(90, 2.0); off.note_on(90, 2.0);
            assert!(!off.strings()[off.notes[0].first].has_damper);
            for frame in 0..(fs * 0.3) as usize {
                if frame == (fs * 0.03) as usize { off.note_off(90, 1.0); }
                let a = held.step(); let b = off.step();
                assert_eq!((a.0.to_bits(), a.1.to_bits()), (b.0.to_bits(), b.1.to_bits()));
            }
        }
    }

    // Frozen grounded seating recurrence from the 203232a control. This is a
    // test oracle, not a second production contact law or a modal-decay claim.
    fn expected_grounded_rate(rate: f64, target: f64, land: f64, k: f64) -> f64 {
        if rate == target { return rate; }
        let approach = if target > rate { 0.004 * land } else { 0.006 };
        let rate = rate + (target - rate) * approach * (48000.0 * k);
        if (rate - target).abs() < 1e-3 { target } else { rate }
    }

    #[test]
    fn combined_quiet_short_off_preserves_geometry_flight_and_grounded_recurrence() {
        let mut cases = 0;
        for midi in [21, 60, 89] {
            for velocity in [1, 8, 16] {
                for off_ms in [0, 10, 30] {
                    for release in [0.25, 1.0, 4.0] {
                        let mut p = key_return_test_piano(midi - 20, midi - 20, 48000.0);
                        // Small frozen immediate-contact controls only for the three
                        // ordinary-release/MIDI16/Off30 cases; no second shipping path.
                        let mut legacy = if velocity == 16 && off_ms == 30 && release == 1.0 {
                            Some(key_return_test_piano(midi - 20, midi - 20, 48000.0))
                        } else { None };
                        let speed = midi_velocity_to_hammer_speed(velocity);
                        p.note_on(midi, speed);
                        if let Some(q) = &mut legacy { q.note_on(midi, speed); }
                        for _ in 0..off_ms * 48 {
                            let (l, r) = p.step(); assert!(l.is_finite() && r.is_finite());
                            if let Some(q) = &mut legacy { assert_eq!((l.to_bits(), r.to_bits()), { let x = q.step(); (x.0.to_bits(), x.1.to_bits()) }); }
                        }
                        if let Some(q) = &legacy { same_state(&p, q); }
                        let flight = (p.notes[0].hammer_active, p.notes[0].eta.to_bits(), p.notes[0].veta.to_bits(),
                            p.notes[0].xi.to_bits(), p.notes[0].vxi.to_bits(), p.notes[0].ky.to_bits());
                        p.note_off(midi, release);
                        assert_eq!(flight, (p.notes[0].hammer_active, p.notes[0].eta.to_bits(), p.notes[0].veta.to_bits(),
                            p.notes[0].xi.to_bits(), p.notes[0].vxi.to_bits(), p.notes[0].ky.to_bits()));
                        let first = p.notes[0].first;
                        assert_eq!(p.strings()[first].damp_target, 0.0);
                        if let Some(q) = &mut legacy {
                            q.note_off(midi, release);
                            q.notes[0].ky = q.notes[0].ky.min(q.action.key_at_damper); // frozen legacy Off assumption
                            q.refresh_damper(0);
                            assert_eq!(q.strings()[first].damp_target, q.sh.damp_max);
                        }
                        let mut onset = None;
                        for frame in 0..7200 {
                            let s = &p.strings()[first];
                            let expected = expected_grounded_rate(s.damp_rate, s.damp_target, s.damp_land, p.k);
                            let (l, r) = p.step(); assert!(l.is_finite() && r.is_finite());
                            let s = &p.strings()[first];
                            assert_eq!(s.damp_rate.to_bits(), expected.to_bits(), "quiet={velocity} Off={off_ms} release={release} midi={midi}");
                            assert_eq!(s.damp_land, release);
                            if let Some(q) = &mut legacy {
                                let s = &q.strings()[first];
                                let expected = expected_grounded_rate(s.damp_rate, s.damp_target, s.damp_land, q.k);
                                let (l, r) = q.step(); assert!(l.is_finite() && r.is_finite());
                                assert_eq!(q.strings()[first].damp_rate.to_bits(), expected.to_bits());
                            }
                            if s.damp_target > 0.0 {
                                assert!(p.notes[0].ky <= p.action.key_at_damper);
                                if onset.is_none() { onset = Some(frame); }
                            } else { assert!(p.notes[0].ky > p.action.key_at_damper); }
                            if onset.is_some_and(|start| frame >= start + 8) { break; }
                        }
                        let onset = onset.expect("finite key return must reach contact");
                        println!("COMBINED midi={midi} velocity={velocity} Off={off_ms}ms release={release}: geometry={:.3}ms afterOff; recurrenceEXACT, flightcontinued(notallcasesrequiredtohit)", (onset + 1) as f64 * p.k * 1000.0);
                        cases += 1;
                    }
                }
            }
        }
        assert_eq!(cases, 81);
    }

    #[test]
    fn combined_quiet_full_compass_helpers_preserve_physical_state() {
        let mut a = key_return_test_piano(1, 88, 48000.0);
        let mut b = key_return_test_piano(1, 88, 48000.0);
        b.set_helper_threads(3, None, None);
        let edges = [0, 1, 479, 480, 681, 1001, 1202, 2002, 2303, 4096];
        for pair in edges.windows(2) {
            for p in [&mut a, &mut b] {
                match pair[0] {
                    0 => for (i, midi) in [21, 36, 60, 84, 89, 90, 108].into_iter().enumerate() {
                        p.note_on(midi, midi_velocity_to_hammer_speed([1, 8, 16][i % 3]));
                    },
                    1 => p.note_off(21, 4.0),
                    479 => p.note_off(60, 0.25),
                    480 => p.note_off(36, 1.0),
                    681 => p.set_pedal(0.41),
                    1001 | 1202 => p.set_sostenuto(true),
                    2002 => { for midi in [84, 89, 90, 108] { p.note_off(midi, 1.0); } p.set_pedal(0.0); },
                    2303 => p.set_sostenuto(false),
                    _ => unreachable!(),
                }
            }
            let frames = pair[1] - pair[0];
            let (mut al, mut ar, mut bl, mut br) = (vec![0.0f32; frames], vec![0.0f32; frames], vec![0.0f32; frames], vec![0.0f32; frames]);
            a.process(&mut al, &mut ar); b.process(&mut bl, &mut br);
            assert!(al.iter().chain(&ar).all(|x| x.is_finite()));
            assert_eq!(al, bl); assert_eq!(ar, br);
            // Scheduler activity/counters intentionally differ with0vs3 helpers;
            // every physical state, control ownership and output must still match.
            same_physical_state(&a, &b);
        }
        b.set_helper_threads(0, None, None);
    }

    #[test]
    fn fused_strings_match_helpers_through_retrigger_pedals_and_wake() {
        let d = make_grand(&GrandOptions { first_key: 64, last_key: 64, ..GrandOptions::default() });
        let o = PianoOptions { board_grid_scale: 1.5, ..PianoOptions::default() };
        let mut engines = [Piano::new(&d, &o).unwrap(), Piano::new(&d, &o).unwrap()];
        engines[1].set_helper_threads(3, None, None);
        let mut output = [[[0.0f32; 64]; 2]; 2];
        for block in 0..600 {
            for (p, out) in engines.iter_mut().zip(&mut output) {
                match block {
                    0 => p.note_on(84, 3.0),
                    5 => p.note_on(84, 2.0),
                    20 | 70 | 280 => p.note_off(84, 1.0),
                    30 => p.set_sustain(true),
                    50 => p.set_sostenuto(true),
                    60 => p.note_on(84, 2.0),
                    80 => { p.set_soft_pedal(true); p.set_sustain(false); }
                    90 => p.set_sostenuto(false),
                    250 => p.note_on(84, 0.0),
                    255 => p.note_on(84, 0.3),
                    _ => {}
                }
                let [left, right] = out;
                p.process(left, right);
                let n = &p.notes[0];
                for (i, s) in p.strings().iter().enumerate() {
                    let struck = i >= n.first && i < n.first + n.total;
                    assert_eq!(s.defer_advance, struck && n.hammer_active, "block {block}, string {i}");
                }
            }
            assert_eq!(output[0], output[1], "stereo output at block {block}");
            assert_eq!(engines[0].sostenuto_down, engines[1].sostenuto_down);
            for (a, b) in engines[0].strings().iter().zip(engines[1].strings()) {
                assert_eq!(a.awake, b.awake);
                assert_eq!(engines[0].nodes(&engines[0].sh.u, a), engines[1].nodes(&engines[1].sh.u, b));
                assert_eq!(engines[0].nodes(&engines[0].sh.v[engines[0].vb()], a), engines[1].nodes(&engines[1].sh.v[engines[1].vb()], b));
            }
        }
    }

    #[test]
    fn string_bridge_increment_survives_wake_and_both_buffer_parities() {
        let d = make_grand(&GrandOptions { first_key: 40, last_key: 40, ..GrandOptions::default() });
        let mut piano = Piano::new(&d, &PianoOptions::default()).unwrap();
        for pass in 0..2 {
            piano.sleep(0);
            unsafe { piano.sh.vw.all_mut().fill(1e-9 * (pass + 1) as f64) };
            piano.wake(0);
            let s = &piano.strings()[0];
            let expected = piano.sh.end_at[0].eval(unsafe { piano.sh.vw.all() });
            assert!(expected.abs() > 0.0);
            for buf in &piano.sh.v {
                assert_eq!(piano.nodes(buf, s)[0], 0.0);
                assert_eq!(piano.nodes(buf, s)[s.n], expected);
            }
            for vb in 0..2 {
                piano.sh.vb.store(vb, Ordering::Relaxed);
                unsafe { piano.sh.step_string(0) };
                let next = piano.nodes(&piano.sh.v[vb as usize ^ 1], s);
                assert_eq!(next[0], 0.0);
                assert_eq!(next[s.n], expected);
            }
        }
    }
    fn observer_test_piano(fs:f64, os:usize) -> Piano {
        let d=make_grand(&GrandOptions { first_key:40,last_key:42,key_thump:0.0,
            mechanism_noise:0.0,room_enabled:false,..GrandOptions::default() });
        Piano::new(&d,&PianoOptions { sample_rate:fs,oversample:os,board_grid_scale:1.5,
            ..PianoOptions::default() }).unwrap()
    }

    #[test]
    fn action_observer_preserves_exact_audio_and_whole_existing_physical_oracle() {
        for fs in [44100.0,48000.0] {
            for os in [1,2] {
                let mut observed=observer_test_piano(fs,os);
                let mut bypassed=observer_test_piano(fs,os);
                // Same real physical solver. Only observer writes/reads are bypassed.
                bypassed.action_observer.enabled=false;
                paired_render(&mut observed,&mut bypassed,67);
                assert!(!observed.action_observer.trace.valid);
                for p in [&mut observed,&mut bypassed] {
                    p.note_on(60,2.0);
                    p.note_off(60,1.0); // actual key return while hammer is still in flight
                }
                for n in [1,7,64,257,1024] { paired_render(&mut observed,&mut bypassed,n); }
                for p in [&mut observed,&mut bypassed] {
                    p.set_soft_pedal(true); p.note_on(62,3.0); p.note_on(60,1.0);
                    p.set_sustain(true); p.set_sostenuto(true);
                }
                for n in [3,127,2048] { paired_render(&mut observed,&mut bypassed,n); }
                for p in [&mut observed,&mut bypassed] {
                    p.note_off(60,0.25); p.note_off(62,4.0);
                    p.set_sustain(false); p.set_sostenuto(false);
                }
                paired_render(&mut observed,&mut bypassed,4096);
                let mut snap=observed.new_snapshot();
                observed.fill_snapshot(&mut snap,-1);
                assert!(snap.action.valid);
                assert_eq!(snap.focus_midi,-1);
                assert_eq!(snap.action.current_stroke().unwrap().midi,60); // independent from legacy waveform-focus lookup
                same_state(&observed,&bypassed); // unchanged physical oracle, excludes observer
                assert!(observed.pristine==bypassed.pristine);
            }
        }
    }

    #[test]
    fn action_observer_aggregate_contacts_and_return_basis_use_real_solver_steps() {
        for soft in [false,true] {
            let mut p=observer_test_piano(48000.0,1);
            p.set_soft_pedal(soft);
            p.note_on(60,2.0);
            let initial=*p.action_observer.trace.poses().last().unwrap();
            let n=&p.notes[p.note_of_midi[60].unwrap()];
            assert_eq!(initial.pose.eta_m.to_bits(),n.eta.to_bits());
            assert_eq!(initial.pose.eta_velocity_mps.to_bits(),(n.veta/p.k).to_bits());
            assert_eq!(initial.pose.xi_velocity_mps.to_bits(),(n.vxi/p.k).to_bits());
            p.note_off(60,1.0);
            assert!(p.notes[p.note_of_midi[60].unwrap()].hammer_active);
            let mut was_positive=false;
            let mut expected_contacts=0;
            let mut expected_peak=0.0f64;
            let mut saw_return=false;
            let mut saw_active_key_return=false;
            for _ in 0..14400 {
                let solve_step=p.step_count;
                p.step();
                let ni=p.note_of_midi[60].unwrap();
                let n=&p.notes[ni];
                let mut force=[0.0;3];
                for i in 0..n.count { force[i]=p.strings()[n.first+i].force; }
                let total=force.iter().sum::<f64>();
                if total>0.0 {
                    if !was_positive {expected_contacts+=1; expected_peak=0.0;}
                    expected_peak=expected_peak.max(total);
                    let c=p.action_observer.trace.contacts().last().unwrap();
                    assert!(c.open);
                    assert_eq!(c.last_positive_solve_step,solve_step);
                    assert_eq!(c.peak_total_force_n.to_bits(),expected_peak.to_bits());
                    assert_eq!(c.peak_pose.pose_step,c.peak_solve_step+1);
                    assert_eq!(c.peak_pose.force_solve_step,c.peak_solve_step);
                    assert!(c.peak_pose.flags&FORCE_OBSERVED!=0);
                    if soft { assert_eq!(c.peak_pose.force_n[0],0.0); }
                } else if was_positive {
                    let c=p.action_observer.trace.contacts().last().unwrap();
                    assert!(!c.open && c.exit_force_observed);
                    assert_eq!(c.exit_solve_step,solve_step);
                    assert_eq!(c.exit_pose.pose_step,solve_step+1);
                }
                was_positive=total>0.0;
                saw_active_key_return|=p.action_observer.trace.poses().any(|r|
                    r.pose.phase==ActionPhase::Flight && r.pose.key_phase==3);
                assert_eq!(p.action_observer.trace.contact_sequence,expected_contacts);
                let origin=p.action_observer.trace.current_stroke().unwrap().return_origin;
                if origin.reference_observed && !saw_return {
                    saw_return=true;
                    assert_eq!(origin.hy_seed_m.to_bits(),(-origin.zeta_at_transition_m).to_bits());
                    assert_eq!(origin.hy_before_action_m.to_bits(),origin.hy_seed_m.to_bits());
                    assert_eq!(origin.origin_m.to_bits(),(origin.eta_at_transition_m+origin.hy_seed_m).to_bits());
                    // zeta_end = eta_next - along_next. Compare, never repair or hide a discrepancy.
                    let scale=origin.eta_at_transition_m.abs()+origin.hy_seed_m.abs()
                        +origin.zeta_at_transition_m.abs()+origin.along0_after_advance_m.abs();
                    assert!((origin.origin_m-origin.along0_after_advance_m).abs()
                        <=256.0*f64::EPSILON*scale.max(f64::MIN_POSITIVE),
                        "return reference disagrees with real post-advance along0: {origin:?}");
                    assert_eq!(origin.transition_pose_step,p.step_count);
                    assert!(!n.hammer_active);
                    assert!(n.returning || (!n.checked && n.hy>=p.action.rest_distance));
                }
            }
            assert!(expected_contacts>0 && saw_return,"actual tap must reach contact and return");
            assert!(saw_active_key_return,"key return must not override observed live flight");
            let mut snap=p.new_snapshot(); p.fill_snapshot(&mut snap,60);
            assert!(snap.hammer_force.iter().all(|f|*f==0.0));
            assert!(snap.action.contacts().any(|c|c.peak_total_force_n>0.0));
            assert!(snap.action.poses().any(|r|r.pose.phase==ActionPhase::Returning || r.pose.phase==ActionPhase::Rest));
        }
    }

    #[test]
    fn action_observer_retrigger_focus_overwrite_and_checked_release_are_observations() {
        let mut p=observer_test_piano(48000.0,1);
        let mut snap=p.new_snapshot(); p.fill_snapshot(&mut snap,60);
        assert!(!snap.action.valid && snap.action.poses().next().is_none());
        p.note_on(60,0.0);
        assert!(!p.action_observer.trace.valid,"zero-speed key is not an invented strike");
        p.note_on(60,2.0);
        let first=p.action_observer.trace.stroke_sequence;
        let mut checked=false;
        for _ in 0..48000 {
            p.step();
            if p.notes[p.note_of_midi[60].unwrap()].checked {checked=true;break;}
        }
        assert!(checked,"real held hammer must reach the backcheck");
        let origin=p.action_observer.trace.current_stroke().unwrap().return_origin.origin_m;
        let checked_hy=p.notes[p.note_of_midi[60].unwrap()].hy;
        p.note_off(60,1.0);
        let n=&p.notes[p.note_of_midi[60].unwrap()];
        assert!(n.returning && !n.checked);
        assert_eq!(n.hy.to_bits(),checked_hy.to_bits());
        assert_eq!(p.action_observer.trace.current_stroke().unwrap().return_origin.origin_m.to_bits(),origin.to_bits());
        for _ in 0..7000 {p.step();}
        assert!(p.action_observer.trace.pose_overwrites>0);
        assert!(p.action_observer.trace.poses().map(|r|r.pose.pose_step).collect::<Vec<_>>().windows(2)
            .all(|w|w[0]<w[1]));
        p.note_on(62,3.0);
        let t=&p.action_observer.trace;
        assert_eq!(t.stroke_sequence,first+1);
        assert_eq!(t.current_stroke().unwrap().midi,62);
        assert_eq!(t.poses().filter(|r|r.stroke_sequence==t.stroke_sequence).count(),1);
        assert_eq!(t.contacts().filter(|r|r.stroke_sequence==t.stroke_sequence).count(),0);
        assert!(!t.current_stroke().unwrap().return_origin.valid);
        assert!(t.poses().any(|r|r.stroke_sequence==first),"old measured history is retained");
        assert_eq!(t.poses().last().unwrap().pose.phase,ActionPhase::Flight);
        let mut new=p.new_snapshot();
        p.fill_snapshot(&mut new,-1);
        assert_eq!(new.action.stroke_sequence,t.stroke_sequence);
        assert_eq!(new.action.current_stroke().unwrap().midi,62);
        let replacement=observer_test_piano(48000.0,1);
        assert!(!replacement.new_snapshot().action.valid,"new generation starts invalid");
    }

    #[test]
    fn action_observer_matches_completed_pose_including_real_housekeeping_sleep() {
        let mut p=observer_test_piano(48000.0,1);
        // Test-only read frequency, NOT a physical scheduler/sleep mutation.
        p.action_observer.trace.sample_stride_steps=1;
        p.note_on(60,1.0); p.note_off(60,1.0);
        let ni=p.note_of_midi[60].unwrap();
        let mut saw_sleep=false;
        let mut saw_nonzero_sleep=false;
        for _ in 0..96000 {
            let n=&p.notes[ni];
            let mut awake=[false;6];
            let mut before_along=[0.0;3];
            for i in 0..n.total {awake[i]=p.strings()[n.first+i].awake;}
            for i in 0..n.count {before_along[i]=p.along(n,i,&p.sh.u);}
            p.step();
            let n=&p.notes[ni];
            let pose=p.action_observer.trace.poses().last().unwrap().pose;
            assert_eq!(pose.pose_step,p.step_count);
            assert_eq!(p.action_observer.trace.latest_step,p.step_count);
            for i in 0..n.count {
                assert_eq!(pose.string_along_m[i].to_bits(),p.along(n,i,&p.sh.u).to_bits(),
                    "must observe AFTER all completed-step work, including sleep");
            }
            assert_eq!(pose.eta_m.to_bits(),n.eta.to_bits());
            assert_eq!(pose.hy_m.to_bits(),n.hy.to_bits());
            assert_eq!(pose.ky_m.to_bits(),n.ky.to_bits());
            assert_eq!(pose.kv_mps.to_bits(),n.kv.to_bits());
            assert_eq!(pose.key_down(),n.key_down);
            assert_eq!(pose.shank_observed(),n.hammer_active && n.s_mass>0.0);
            if pose.force_observed() {assert_eq!(pose.force_solve_step+1,pose.pose_step);}
            for i in 0..n.total {
                if awake[i] && !p.strings()[n.first+i].awake {
                    saw_sleep=true;
                    assert_eq!(p.step_count&63,0,"actual housekeeping boundary");
                    let contact=i%n.count;
                    saw_nonzero_sleep|=before_along[contact]!=0.0
                        && pose.string_along_m[contact].to_bits()!=before_along[contact].to_bits();
                }
            }
            if saw_sleep && saw_nonzero_sleep {break;}
        }
        assert!(saw_sleep && saw_nonzero_sleep,"real stroke must exercise a nonzero-to-sleep endpoint");
    }

    #[test]
    fn action_observer_canonical_note_and_requested_let_off_do_not_mislabel_state() {
        let mut d=make_grand(&GrandOptions {first_key:40,last_key:40,..GrandOptions::default()});
        d.action.let_off=0.0; // real design input; solver's existing 1e-5 floor is unchanged
        let mut p=Piano::new(&d,&PianoOptions {board_grid_scale:1.5,..PianoOptions::default()}).unwrap();
        p.note_on(60+128,2.0);
        let ni=p.note_of_midi[60].unwrap();
        let n=&p.notes[ni];
        let trace=&p.action_observer.trace;
        assert_eq!(trace.current_stroke().unwrap().midi,n.midi as i16);
        assert_eq!(trace.current_stroke().unwrap().midi,60);
        assert_eq!(trace.current_stroke().unwrap().let_off_requested_m,0.0);
        assert_eq!(trace.current_stroke().unwrap().let_off_effective_m(),1e-5);
        let mut top=-1e30f64;
        for i in 0..n.count {top=top.max(p.along(n,i,&p.sh.u));}
        assert_eq!(n.eta.to_bits(),(top-trace.current_stroke().unwrap().let_off_effective_m()).to_bits());
        assert_eq!(trace.poses().next().unwrap().pose.eta_m.to_bits(),n.eta.to_bits());
        p.note_off(60+128,1.0);
        assert!(!p.notes[ni].key_down);
    }

    #[test]
    fn action_observer_full_88_default_physics_helpers_preserve_bits() {
        let d=make_grand(&GrandOptions::default());
        assert_eq!(d.notes.len(),88);
        for helpers in [0,3] {
            let mut a=Piano::new(&d,&PianoOptions::default()).unwrap();
            let mut b=Piano::new(&d,&PianoOptions::default()).unwrap();
            b.action_observer.enabled=false;
            for p in [&mut a,&mut b] {p.set_helper_threads(helpers,None,None);}
            paired_render(&mut a,&mut b,32);
            for p in [&mut a,&mut b] {p.note_on(60,2.0);p.note_off(60,1.0);}
            for frames in [64,256] {paired_render(&mut a,&mut b,frames);}
            for p in [&mut a,&mut b] {p.note_on(36,2.0);p.note_on(84,3.0);}
            paired_render(&mut a,&mut b,512);
            for p in [&mut a,&mut b] {p.note_off(36,1.0);p.note_off(84,1.0);}
            paired_render(&mut a,&mut b,2048);
            assert!(a.pristine==b.pristine);
        }
    }

    #[test]
    fn action_retention_closes_observation_with_real_pre_mutation_endpoint() {
        let mut p=observer_test_piano(48000.0,1);
        p.note_on(60,2.0);
        let old=p.action_observer.trace.stroke_sequence;
        let mut hit=false;
        for _ in 0..512 {
            p.step();
            if p.action_observer.trace.contacts().any(|c|c.stroke_sequence==old && c.open) {hit=true;break;}
        }
        assert!(hit,"real old focus must enter solved contact");
        let step=p.step_count;
        let endpoint=p.observed_action_pose(step);
        let last_positive=p.action_observer.trace.contacts().last().unwrap().last_positive_solve_step;
        p.note_on(62,3.0);
        let t=&p.action_observer.trace;
        let meta=t.stroke(old).unwrap();
        assert!(!meta.observing);
        assert_eq!(meta.observation_end_step,step);
        assert_eq!(meta.last_observed_pose_step,step);
        let ended=t.poses().filter(|r|r.stroke_sequence==old).last().unwrap();
        assert_eq!(ended.pose,endpoint,"must record OLD actual pose before any note mutation");
        let seed=t.poses().last().unwrap();
        assert_ne!(seed.stroke_sequence,old);
        assert_eq!(seed.pose.pose_step,ended.pose.pose_step);
        assert!(seed.sequence>ended.sequence,"same-step ordering is explicit and deterministic");
        assert_eq!(t.current_stroke().unwrap().midi,62);
        let contact=t.contacts().find(|c|c.stroke_sequence==old).unwrap();
        assert!(!contact.open && contact.observation_ended && !contact.exit_force_observed);
        assert_eq!(contact.observed_exit_solve_step(),None);
        assert_eq!(contact.termination_pose_step(),Some(step));
        assert_eq!(contact.exit_pose,endpoint);
        for _ in 0..512 {p.step();}
        let t=&p.action_observer.trace;
        assert_eq!(t.contacts().find(|c|c.stroke_sequence==old).unwrap().last_positive_solve_step,last_positive,
            "unfocused future contact is not observed or invented");
        assert_eq!(t.stroke(old).unwrap().observation_end_step,step);
    }

    #[test]
    fn action_retention_bounds_history_and_metadata_without_claiming_polyphony() {
        let mut p=observer_test_piano(48000.0,1);
        for launch in 0..8 {
            p.note_on(60,2.0);p.note_off(60,1.0);
            for _ in 0..960 {p.step();}
            if launch==1 {
                let t=&p.action_observer.trace;
                assert!(t.poses().any(|r|r.stroke_sequence==1));
                assert!(t.contacts().any(|c|c.stroke_sequence==1 && c.peak_total_force_n>0.0),
                    "recorded contact survives a later launch before publication");
                assert_eq!(t.stroke(1).unwrap().midi,60);
                assert!(!t.stroke(1).unwrap().observing);
            }
        }
        let t=&p.action_observer.trace;
        assert_eq!(t.strokes().count(),8);
        assert!(t.pose_overwrites>0);
        let cursor=t.latest_step-(0.060/t.internal_dt_s).round() as u64;
        assert!(t.strokes().any(|meta| {
            let rows=t.poses().filter(|r|r.stroke_sequence==meta.stroke_sequence).collect::<Vec<_>>();
            rows.windows(2).any(|w|w[0].pose.pose_step<=cursor && cursor<=w[1].pose.pose_step
                && w[0].pose.pose_step<w[1].pose.pose_step && w[0].pose.phase==w[1].pose.phase)
        }),"global delayed cursor has an ACTUAL same-stroke bracket after dense launches");
        for _ in 0..17 {p.note_on(60,2.0);} // real simultaneous launches, not seeded counters
        let t=&p.action_observer.trace;
        assert_eq!(t.strokes().count(),16);
        assert_eq!(t.metadata_overwrites,9);
        assert!(t.stroke(1).is_none());
        assert!(t.unresolved_pose_count()>0 && t.unresolved_contact_count()>0,
            "missing metadata is explicit, never reused from a recycled slot");
        assert!(t.poses().map(|r|r.sequence).collect::<Vec<_>>().windows(2).all(|w|w[0]<w[1]));
        // Real same-step chord. Earlier voices continue physically but only latest is observed.
        let mut ids=[0u64;3];
        for (i,midi) in [60,61,62].into_iter().enumerate() {
            p.note_on(midi,2.0);ids[i]=p.action_observer.trace.stroke_sequence;
        }
        let mut physical_contacts=[false;3];
        for _ in 0..512 {
            p.step();
            for (i,midi) in [60,61,62].into_iter().enumerate() {
                let n=&p.notes[p.note_of_midi[midi as usize].unwrap()];
                physical_contacts[i]|=(0..n.count).any(|j|p.strings()[n.first+j].force>0.0);
            }
        }
        assert!(physical_contacts.into_iter().all(|contact|contact),"real physical chord contacts");
        let t=&p.action_observer.trace;
        assert!(t.contacts().any(|c|c.stroke_sequence==ids[2] && c.peak_total_force_n>0.0));
        for id in &ids[..2] {
            assert!(!t.stroke(*id).unwrap().observing);
            assert!(!t.contacts().any(|c|c.stroke_sequence==*id),
                "same-step let-off seeds are NOT evidence of unobserved polyphonic contact");
        }
    }

    #[test]
    fn action_retention_real_contact_ring_overflow_has_exact_counts_and_ownership() {
        let mut p=observer_test_piano(48000.0,1);
        let mut expected=Vec::<(u64,u64)>::new();
        for launch in 0..20 {
            p.note_on(60,2.0);p.note_off(60,1.0);
            let id=p.action_observer.trace.stroke_sequence;
            assert_eq!(id,launch+1);
            let before=expected.len();
            let mut positive=false;
            for _ in 0..960 {
                let solve_step=p.step_count;
                p.step();
                let n=&p.notes[p.note_of_midi[60].unwrap()];
                let total=(0..n.count).map(|i|p.strings()[n.first+i].force).sum::<f64>();
                if total>0.0 && !positive {expected.push((id,solve_step));}
                positive=total>0.0;
                assert_eq!(p.action_observer.trace.contact_sequence,expected.len() as u64,
                    "aggregate episodes counted from REAL solved force transitions");
            }
            assert!(expected.len()>before,"each real20ms stroke must actually contact");
        }
        let t=&p.action_observer.trace;
        assert!(expected.len()>16,"exercise actual contact overwrite, not only pose/metadata overwrite");
        assert_eq!(t.contacts().count(),16);
        assert_eq!(t.contact_overwrites,expected.len() as u64-16);
        assert_eq!(t.contact_sequence,expected.len() as u64);
        let retained:Vec<_>=t.contacts().collect();
        let first=expected.len()-16;
        for (i,contact) in retained.iter().enumerate() {
            let (id,solve_step)=expected[first+i];
            assert_eq!(contact.sequence,(first+i+1) as u64);
            assert_eq!(contact.stroke_sequence,id);
            assert_eq!(contact.entry_solve_step,solve_step);
            assert_eq!(contact.entry_pose.pose_step,solve_step+1);
            assert!(contact.entry_pose.force_observed());
            assert!(contact.entry_pose.force_n.iter().any(|f|*f>0.0));
            let metadata=t.stroke(id).expect("retained16 episodes have retained actual stroke metadata");
            assert_eq!(metadata.midi,60);
            assert!(contact.peak_pose.pose_step>=metadata.onset_step);
            assert!(!contact.open);
            assert_ne!(contact.exit_force_observed,contact.observation_ended,
                "measured force exit and stopped force observation are not the same event");
        }
        assert!(t.current_stroke().unwrap().observing,"pose observation may continue after contact exit");
        assert!(retained.iter().any(|c|c.stroke_sequence==t.stroke_sequence && c.exit_force_observed));
    }

}
