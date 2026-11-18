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

use crate::acoustics::Radiation;
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
    /// How fast the damper comes down, relative to ordinary.
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
    end_u: Buf<f64>,
    end_v: Buf<f64>,
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
    phase_chunks: [Vec<usize>; 2],

    // One word holds the whole job: generation, phase and the next unclaimed chunk. A chunk is
    // claimed by compare-and-swap on that word, so a thread can never pair a chunk of a new
    // job with the phase of an old one.
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
    fn claim(&self) -> Option<(usize, usize)> {
        let mut t = self.ticket.load(Ordering::Acquire);
        loop {
            let idx = (t & 0xffff_ffff) as usize;
            let phase = ((t >> 32) & 3) as usize;
            if phase > 1 || idx >= self.phase_chunks[phase].len() {
                return None;
            }
            match self.ticket.compare_exchange_weak(t, t + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some((phase, idx)),
                Err(now) => t = now,
            }
        }
    }

    /// Phase 0 computes forces from the state of the previous step (strings, plate stencil) and
    /// the sound of the previous step. Phase 1 advances the string displacements.
    ///
    /// # Safety
    /// Must be called once per (job, index); the chunks of a phase touch disjoint state.
    unsafe fn chunk_work(&self, phase: usize, index: usize) {
        match self.chunks[self.phase_chunks[phase][index]] {
            Chunk::Radiation => {
                if phase == 0 {
                    *self.rad_out.get() = self.radiation.get().step(self.rad_in.all());
                }
            }
            Chunk::PlateSpans(from, to) => {
                if phase == 0 {
                    let p = &self.plate;
                    p.apply_stencil(self.w.ptr(), self.vw.ptr(), self.r.ptr(), from, to);
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
            }
            Chunk::Strings(from, to) => {
                if phase == 0 {
                    for i in from..to {
                        self.step_string(i);
                    }
                } else {
                    let vb = self.vb.load(Ordering::Relaxed) as usize;
                    for i in from..to {
                        let s = &*self.strings.ptr().add(i);
                        if !s.awake {
                            continue;
                        }
                        let u = std::slice::from_raw_parts_mut(self.u.ptr().add(s.off + 1), s.n - 1);
                        let v = std::slice::from_raw_parts(self.v[vb].ptr().add(s.off + 1), s.n - 1);
                        for (ul, vl) in u.iter_mut().zip(v) {
                            *ul += *vl;
                        }
                    }
                }
            }
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
        let u_n = *self.end_u.ptr().add(index);
        let v_n = *self.end_v.ptr().add(index);
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
#[derive(Clone, Default)]
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
}

pub type ThreadHook = Arc<dyn Fn() + Send + Sync>;

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
    quiet_checks: u32,
    hammer_iters_max: usize,
    /// How each string's end follows the board (never changes).
    end_at: Vec<Coupling>,
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
        let phase0: Vec<usize> = (0..chunks.len()).collect();
        let phase1: Vec<usize> = (0..chunks.len()).filter(|&c| matches!(chunks[c], Chunk::Strings(..))).collect();

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
            end_u: Buf::from_vec(vec![0.0; ns]),
            end_v: Buf::from_vec(vec![0.0; ns]),
            end_force: Buf::from_vec(vec![0.0; ns]),
            w: Buf::from_vec(vec![0.0; grid]),
            vw: Buf::from_vec(vec![0.0; grid]),
            r: Buf::from_vec(vec![0.0; grid]),
            rad_in: Buf::from_vec(vec![0.0; grid]),
            radiation: Cell::new(radiation),
            rad_out: Cell::new((0.0, 0.0)),
            air_now: Buf::from_vec(air_now),
            chunks,
            phase_chunks: [phase0, phase1],
            ticket: AtomicU64::new(NO_JOB),
            done: AtomicI32::new(0),
            stop: AtomicBool::new(false),
            active: AtomicBool::new(false),
            in_block: AtomicBool::new(false),
            park: Mutex::new(()),
            wake: Condvar::new(),
        });
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
            quiet_checks: 1 << 20,
            hammer_iters_max: 0,
            end_at,
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
    for l in 2..len - 1 {
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

fn felt(z: f64, zdot: f64, k: f64, p: f64, drag: f64) -> f64 {
    if z <= 0.0 {
        return 0.0;
    }
    (k * z.powf(p) + drag * zdot).max(0.0)
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
        let (u_n, v_n) = if s.attached { (self.end_at[index].eval(w), self.end_at[index].eval(vw)) } else { (0.0, 0.0) };
        let u = self.nodes(&self.sh.u, s);
        let v = self.nodes(&self.sh.v[self.vb()], s);
        for l in 0..=s.n {
            u[l] = u_n * l as f64 / s.n as f64;
            v[l] = v_n * l as f64 / s.n as f64;
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
        let mut n = self.notes[ni].clone();
        n.key_down = true;
        for i in 0..n.total {
            self.wake(n.first + i);
            let s = &mut self.strings_mut()[n.first + i];
            s.damp_target = 0.0; // the key lifts the damper well before the hammer arrives
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
    }

    /// `release_speed`: how fast the key comes back up, 1 = ordinary. Sets how quickly the
    /// damper lands.
    pub fn note_off(&mut self, midi: i32, release_speed: f64) {
        let Some(ni) = self.note_of_midi[(midi & 127) as usize] else { return };
        if !self.notes[ni].key_down {
            return;
        }
        let release_speed = release_speed.clamp(0.25, 4.0);
        let target = self.sh.damp_max * self.pedal_damping;
        let n = &mut self.notes[ni];
        n.key_down = false;
        let (first, total, sost) = (n.first, n.total, n.sost);
        // The key on its way up: a note-off is taken as the moment the damper meets the string,
        // which happens about half way up. The key then travels on to its rest.
        n.key_phase = 3;
        n.ky = n.ky.min(self.action.key_at_damper);
        n.kv = -0.15 * release_speed;
        n.k_force = 0.0;
        if n.checked {
            // the backcheck lets the hammer go
            n.checked = false;
            n.returning = true;
        }
        for s in &mut self.strings_mut()[first..first + total] {
            if s.has_damper && !sost {
                s.damp_target = target;
                s.damp_land = release_speed;
            }
        }
    }

    pub fn set_sustain(&mut self, down: bool) {
        self.set_pedal(if down { 1.0 } else { 0.0 });
    }

    /// Sustain pedal position, 0 (up) to 1 (down).
    pub fn set_pedal(&mut self, position: f64) {
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
        let damp_max = self.sh.damp_max;
        for ni in 0..self.notes.len() {
            let (first, total, held) = (self.notes[ni].first, self.notes[ni].total, self.notes[ni].key_down || self.notes[ni].sost);
            for i in first..first + total {
                let s = &self.strings()[i];
                if !s.has_damper {
                    continue;
                }
                if damping < 1.0 && !s.sideways {
                    self.wake(i);
                }
                if !held {
                    let s = &mut self.strings_mut()[i];
                    s.damp_target = damp_max * damping;
                    s.damp_land = 1.0;
                }
            }
        }
    }

    /// Una corda: the action shifts so each hammer misses one string of its three.
    pub fn set_soft_pedal(&mut self, down: bool) {
        self.soft = down;
    }

    /// Sostenuto: dampers of the keys held at this moment stay up until the pedal is released.
    pub fn set_sostenuto(&mut self, down: bool) {
        let target = self.sh.damp_max * self.pedal_damping;
        for ni in 0..self.notes.len() {
            let n = &mut self.notes[ni];
            if down {
                n.sost = n.key_down;
                continue;
            }
            let release = n.sost && !n.key_down;
            n.sost = false;
            if release {
                let (first, total) = (n.first, n.total);
                for s in &mut self.strings_mut()[first..first + total] {
                    if s.has_damper {
                        s.damp_target = target;
                        s.damp_land = 1.0;
                    }
                }
            }
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
            let bottom = felt(n.ky - a.key_travel, n.kv, a.felt_k, a.felt_p, a.felt_c);
            let rest = felt(-n.ky, -n.kv, a.felt_k, a.felt_p, a.felt_c);
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
            let stop = felt(n.hy - place, n.hv, a.stop_k, a.stop_p, a.stop_c);
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
            s.end_peak = end.abs().max(0.97 * s.end_peak);
            if deviation(u) < 1e-8 + 3.0 * s.end_peak && !self.never_sleep {
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
        // A plain reference that does not tie up `self`: the shared state outlives this call.
        let sh: &Shared = unsafe { &*Arc::as_ptr(&self.sh) };
        let k = self.k;
        let shared = !self.threads.is_empty() && sh.active.load(Ordering::Relaxed);
        // Start a pass over all chunks of a phase. With helpers active the caller may do other
        // work before finishing it; without them the pass runs here and now.
        let post = |phase: usize| {
            if shared {
                sh.done.store(0, Ordering::Release);
                let generation = (sh.ticket.load(Ordering::Relaxed) >> 34) + 1;
                sh.ticket.store((generation << 34) | ((phase as u64) << 32), Ordering::Release);
            } else {
                for c in 0..sh.phase_chunks[phase].len() {
                    unsafe { sh.chunk_work(phase, c) };
                }
            }
        };
        // Finish a shared pass: take whatever chunks are left, then wait for the helpers' ones.
        let finish = |phase: usize| {
            if !shared {
                return;
            }
            while let Some((ph, c)) = sh.claim() {
                unsafe { sh.chunk_work(ph, c) };
                sh.done.fetch_add(1, Ordering::Release);
            }
            let all = sh.phase_chunks[phase].len() as i32;
            while sh.done.load(Ordering::Acquire) < all {
                spin();
            }
            sh.ticket.store(NO_JOB | (sh.ticket.load(Ordering::Relaxed) & !0x3_ffff_ffff), Ordering::Release);
        };

        post(0); // strings, plate stencil and last step's sound, side by side
        finish(0);
        // From here to the next post only this thread touches the shared state.
        let (r, w, vw) = unsafe { (sh.r.all_mut(), sh.w.all_mut(), sh.vw.all_mut()) };
        sh.plate.apply_extras(w, r, &mut self.solve_work);
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
                let at = &self.end_at[i];
                for q in 0..at.n {
                    r[at.idx[q]] += at.w[q] * f;
                }
            }
        }
        for ni in 0..self.notes.len() {
            if self.notes[ni].hammer_active {
                self.step_hammer(ni);
            }
            let n = &mut self.notes[ni];
            if n.key_phase == 1 || n.key_phase == 3 || n.returning {
                Self::step_action(&self.action, k, n, r);
            }
        }
        sh.vb.fetch_xor(1, Ordering::Relaxed);
        post(1); // advance the string displacements while this thread solves for the stiffeners
        sh.plate.solve_stiff(r, &mut self.solve_work);
        finish(1);
        {
            // Advance the plate, keep what radiates for the next step's radiation task, and read
            // off where every string's end now is.
            let keep = unsafe { sh.rad_in.all_mut() };
            let scale = sh.plate.diag_scale();
            for sp in &sh.plate.spans {
                let range = sp.start..sp.start + sp.len;
                plate_advance(&r[range.clone()], &scale[range.clone()], &mut vw[range.clone()], &mut w[range.clone()], &mut keep[range]);
            }
            let (live, eu, ev) = unsafe { (sh.end_live.all(), sh.end_u.all_mut(), sh.end_v.all_mut()) };
            for i in 0..live.len() {
                if !live[i] {
                    continue;
                }
                let at = &self.end_at[i];
                let (mut su, mut sv) = (0.0, 0.0);
                for q in 0..at.n {
                    unsafe {
                        su += at.w[q] * *w.get_unchecked(at.idx[q]);
                        sv += at.w[q] * *vw.get_unchecked(at.idx[q]);
                    }
                }
                eu[i] = su;
                ev[i] = sv;
            }
        }
        let out = unsafe { *sh.rad_out.get() };
        self.step_count += 1;
        if self.step_count & 63 == 0 {
            self.housekeeping();
        }
        out
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
                    if let Some((phase, c)) = sh.claim() {
                        unsafe { sh.chunk_work(phase, c) };
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
                vv[n] = self.end_at[i].eval(vw);
                x[n] = self.end_at[i].eval(&xg);
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
        unsafe { self.sh.w.all_mut() }
    }
    pub fn plate_v_mut(&mut self) -> &mut [f64] {
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
        self.nodes(&self.sh.u, &self.strings()[index])
    }
    /// Per-step increment at nodes 0..=N of a string.
    pub fn string_v_mut(&mut self, index: usize) -> &mut [f64] {
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
        self.ext_at = self.sh.plate.interp(piano_xy);
        self.ext_f = newtons;
    }
    pub fn board_velocity(&self, piano_xy: Vec2) -> f64 {
        self.sh.plate.interp(piano_xy).eval(unsafe { self.sh.vw.all() }) / self.k
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
        }
    }

    pub fn fill_snapshot(&self, snap: &mut Snapshot, focus_midi: i32) {
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
        snap.focus_midi = focus_midi;
        snap.shape_nodes = [0; 3];
        snap.hammer_force = [0.0; 3];
        let Some(ni) = (if focus_midi >= 0 { self.note_of_midi[(focus_midi & 127) as usize] } else { None }) else { return };
        let n = &self.notes[ni];
        for i in 0..n.count {
            let s = &self.strings()[n.first + i];
            let u = self.nodes(&self.sh.u, s);
            snap.shape_nodes[i] = s.n + 1;
            for l in 0..=s.n {
                snap.shape[i][l] = if s.awake { u[l] as f32 } else { 0.0 };
            }
            snap.hammer_force[i] = s.force as f32;
        }
    }
}

impl Drop for Piano {
    fn drop(&mut self) {
        self.set_helper_threads(0, None, None);
    }
}
