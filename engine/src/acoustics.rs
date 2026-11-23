//! From board motion to sound at the listener's ears.
//!
//! Each side of the soundboard radiates into its own half space (Rayleigh integral), the top
//! side with one sign and the underside with the opposite one. Sound then reaches each ear:
//!   - directly, summed node by node;
//!   - by way of the floor, the open lid, the walls and the ceiling, as mirror images up to
//!     second order, summed over patches of the board;
//!   - as a late reverberant field, from a feedback delay network whose decay time and level
//!     follow Sabine's formula for the same room.
//! The first two are geometry; the third is a statistical stand-in, not a simulation.
//! Diffraction around the case and the lid's shadow are not modelled.
//!
//! The same patches carry the reaction of the air on the board: each patch feels its own
//! motion (a piston: pressure rho c [v(t) - v(t - a/c)] at its centre, which is mass-like at
//! low and resistive at high frequency) and the delayed acceleration of every other patch.
//! Both faces of the board count.

use crate::design::{PianoDesign, Vec2};
use crate::plate::Plate;
use std::f64::consts::PI;

const LINES: usize = 8;

/// Sabine late-decay time in seconds for an enabled rectangular room.
/// Pure geometry/material estimate; preserves the radiation calibration's operation order.
#[inline]
pub fn room_decay_time(lx: f64, ly: f64, lz: f64, absorption: f64) -> f64 {
    let volume = lx * ly * lz;
    let surface = 2.0 * (lx * ly + lx * lz + ly * lz);
    let absorb = absorption * surface;
    0.161 * volume / absorb
}

#[inline]
fn mutual_history_index(base: usize, offset: usize, len: usize) -> usize {
    // Both base and offset are below len. One ring length cancels unsigned underflow;
    // len (time slots * patches) need not be a power of two.
    let index = base.wrapping_sub(offset);
    if index >= len { index.wrapping_add(len) } else { index }
}

/// Sources grouped by their delay to one ear.
#[derive(Default)]
struct Bins {
    first: usize,
    ptr: Vec<usize>,
    idx: Vec<usize>,
    gain: Vec<f64>,
}

impl Bins {
    fn build(delay: &[usize], source: &[usize], g: &[f64]) -> Bins {
        if delay.is_empty() {
            return Bins { first: 0, ptr: vec![0], idx: Vec::new(), gain: Vec::new() };
        }
        let lo = *delay.iter().min().unwrap();
        let hi = *delay.iter().max().unwrap();
        let mut ptr = vec![0usize; hi - lo + 2];
        for &d in delay {
            ptr[d - lo + 1] += 1;
        }
        for b in 0..hi - lo + 1 {
            ptr[b + 1] += ptr[b];
        }
        let mut fill = ptr[..ptr.len() - 1].to_vec();
        let mut idx = vec![0usize; delay.len()];
        let mut gain = vec![0.0; delay.len()];
        for i in 0..delay.len() {
            let q = fill[delay[i] - lo];
            fill[delay[i] - lo] += 1;
            idx[q] = source[i];
            gain[q] = g[i];
        }
        Bins { first: lo, ptr, idx, gain }
    }
}

#[derive(Default)]
pub struct Radiation {
    /// Sabine, seconds (0 when there is no room).
    pub reverb_time: f64,
    pub image_count: usize,
    pub patch_count: usize,

    node: [Bins; 2],
    // Reflections: one entry per patch and mirror image.
    img_delay: [Vec<usize>; 2],
    img_patch: [Vec<usize>; 2],
    img_gain: [Vec<f64>; 2],
    patch_ptr: Vec<usize>,
    patch_node: Vec<usize>,
    patch_acc: Vec<f64>,
    ring: [Vec<f64>; 2],
    ring_pos: usize,
    ring_mask: usize,

    // Air loading.
    air: bool,
    air_force: Vec<f64>,
    patch_of_grid: Vec<u32>,
    patch_vel: Vec<f64>,
    inv_count: Vec<f64>,
    // [time][patch], small enough to stay in the first-level cache.
    vel_hist: Vec<f32>,
    acc_hist: Vec<f32>,
    vel_mask: usize,
    self_delay: Vec<usize>,
    // delay * patch_count - source: positive because every delay is at least one tick.
    mut_offset: Vec<usize>,
    mut_ptr: Vec<usize>,
    mut_gain: Vec<f32>,
    self_gain: f64,
    hist_pos: usize,
    hist_mask: usize,

    // Late field.
    reverb: bool,
    line: [Vec<f64>; LINES],
    line_pos: [usize; LINES],
    line_gain: [f64; LINES],
    line_lp: [f64; LINES],
    lp_coef: f64,
    reverb_gain: f64,
    hp_state: f64,
    hp_coef: f64,
}

impl Radiation {
    /// Reaction of the air on the board, per patch, as dt^2 * force on each node of the patch,
    /// from the motion up to the step last passed to `step`. Empty when air loading is off.
    pub fn air_force(&self) -> &[f64] {
        &self.air_force
    }

    /// Grid storage index -> patch.
    pub fn patch_of_grid(&self) -> &[u32] {
        &self.patch_of_grid
    }

    pub fn new(p: &Plate, d: &PianoDesign, fs: f64) -> Radiation {
        let mut rad = Radiation::default();
        let room = &d.room;
        let k = 1.0 / fs;
        let (c, rho) = (d.air_c, d.air_rho);
        let ds = p.hx * p.hy;
        let strength = rho * ds / (2.0 * PI) / (k * k); // half-space monopole, input is dt^2 * accel
        let delay_of = |r: f64| (r / c * fs).round() as usize;

        // Patches of about a quarter metre, used for everything but the direct sound. Reflected
        // paths are metres long, so this resolves them up to roughly 700 Hz across a patch; above
        // that the reflections are effectively blurred, as real surfaces blur them.
        let mut patch_of = vec![0usize; p.n];
        let mut centre: Vec<Vec2> = Vec::new();
        {
            let bi = ((0.25 / p.hx).round() as usize).max(1);
            let bj = ((0.25 / p.hy).round() as usize).max(1);
            let mut id: Vec<Option<usize>> = vec![None; (p.nx / bi + 1) * (p.ny / bj + 1)];
            let mut count: Vec<usize> = Vec::new();
            for a in 0..p.n {
                let cell = p.gi[a] / bi + (p.nx / bi + 1) * (p.gj[a] / bj);
                let pid = *id[cell].get_or_insert_with(|| {
                    centre.push(Vec2::default());
                    count.push(0);
                    centre.len() - 1
                });
                patch_of[a] = pid;
                let pos = p.node_position(a);
                centre[pid].x += pos.x;
                centre[pid].y += pos.y;
                count[pid] += 1;
            }
            for (cp, n) in centre.iter_mut().zip(&count) {
                cp.x /= *n as f64;
                cp.y /= *n as f64;
            }
            let np = centre.len();
            rad.patch_count = np;
            rad.patch_ptr = vec![0; np + 1];
            for a in 0..p.n {
                rad.patch_ptr[patch_of[a] + 1] += 1;
            }
            for q in 0..np {
                rad.patch_ptr[q + 1] += rad.patch_ptr[q];
            }
            let mut fill = rad.patch_ptr[..np].to_vec();
            rad.patch_node = vec![0; p.n];
            for a in 0..p.n {
                rad.patch_node[fill[patch_of[a]]] = p.gid[a];
                fill[patch_of[a]] += 1;
            }
            rad.patch_acc = vec![0.0; np];
            rad.patch_of_grid = vec![0; p.grid_size];
            for a in 0..p.n {
                rad.patch_of_grid[p.gid[a]] = patch_of[a] as u32;
            }
        }
        let np = rad.patch_count;

        rad.air = d.air_loading;
        if rad.air {
            rad.air_force = vec![0.0; np];
            rad.patch_vel = vec![0.0; np];
            rad.inv_count = vec![0.0; np];
            rad.self_delay = vec![0; np];
            let (mut far, mut near_max) = (1usize, 1usize);
            rad.mut_ptr = vec![0];
            for a in 0..np {
                let count = rad.patch_ptr[a + 1] - rad.patch_ptr[a];
                rad.inv_count[a] = 1.0 / count as f64;
                let radius = (count as f64 * ds / PI).sqrt();
                rad.self_delay[a] = delay_of(radius).max(1);
                near_max = near_max.max(rad.self_delay[a]);
                for b in 0..np {
                    if b == a {
                        continue;
                    }
                    let r = (centre[a].x - centre[b].x).hypot(centre[a].y - centre[b].y);
                    let dl = delay_of(r).max(1);
                    rad.mut_offset.push(dl.checked_mul(np).expect("air history offset overflow") - b);
                    // Both faces; acts on the patch's summed dt^2 * accel.
                    rad.mut_gain.push((-rho * ds * ds / (PI * r)) as f32);
                    far = far.max(dl);
                }
                rad.mut_ptr.push(rad.mut_offset.len());
            }
            rad.self_gain = -2.0 * rho * c * k * ds; // both faces; acts on the mean increment per step
            let len = (far + 2).next_power_of_two();
            rad.hist_mask = len - 1;
            let vlen = (near_max + 2).next_power_of_two();
            rad.vel_mask = vlen - 1;
            rad.vel_hist = vec![0.0; np * vlen];
            rad.acc_hist = vec![0.0; np.checked_mul(len).expect("air history size overflow")];
            debug_assert!(rad.mut_offset.iter().all(|&offset| offset < rad.acc_hist.len()));
        }

        let mut max_delay = 0usize;
        let mut images = 0usize;
        for e in 0..2 {
            let ear = d.ears[e];
            // Direct sound, node by node. Delays are rounded to the nearest step (a path error
            // under 4 mm at 48 kHz, well below the grid spacing).
            let (mut delay, mut src, mut gain) = (Vec::with_capacity(p.n), Vec::with_capacity(p.n), Vec::with_capacity(p.n));
            for a in 0..p.n {
                let pos = p.node_position(a);
                let r = ((pos.x - ear.x).powi(2) + (pos.y - ear.y).powi(2) + ear.z * ear.z).sqrt();
                delay.push(delay_of(r));
                src.push(p.gid[a]);
                gain.push(strength / r * if ear.z >= 0.0 { 1.0 } else { -1.0 });
                max_delay = max_delay.max(delay[a]);
            }
            rad.node[e] = Bins::build(&delay, &src, &gain);

            let (mut idelay, mut isrc, mut igain): (Vec<usize>, Vec<usize>, Vec<f64>) = (Vec::new(), Vec::new(), Vec::new());
            if room.enabled {
                // Shoebox images of the ear, up to two reflections. Room coordinates put the floor
                // at z = 0; the board sits at board_height. A path that leaves upward comes from
                // the top side, downward from the underside.
                let l = [room.lx, room.ly, room.lz];
                let ear_room = [ear.x + room.piano_x, ear.y + room.piano_y, ear.z + room.board_height];
                let refl = (1.0 - room.absorption).sqrt();
                for nx in -2i32..=2 {
                    for ny in -2i32..=2 {
                        for nz in -2i32..=2 {
                            let order = nx.abs() + ny.abs() + nz.abs();
                            if order == 0 || order > 2 {
                                continue;
                            }
                            let n = [nx, ny, nz];
                            let mut im = [0.0; 3];
                            for q in 0..3 {
                                im[q] = n[q] as f64 * l[q] + if n[q] & 1 != 0 { l[q] - ear_room[q] } else { ear_room[q] };
                            }
                            let (ex, ey, ez) = (im[0] - room.piano_x, im[1] - room.piano_y, im[2] - room.board_height);
                            let amplitude = refl.powi(order);
                            for (pi, cp) in centre.iter().enumerate() {
                                let r = ((cp.x - ex).powi(2) + (cp.y - ey).powi(2) + ez * ez).sqrt();
                                idelay.push(delay_of(r));
                                isrc.push(pi);
                                igain.push(strength / r * amplitude * if ez >= 0.0 { 1.0 } else { -1.0 });
                                max_delay = max_delay.max(delay_of(r));
                            }
                            images += 1;
                        }
                    }
                }
            }
            if d.lid.open {
                // The lid is a plane hinged along the bass side, rising toward the treble. Mirror
                // the ear in it; keep the image only for patches whose reflection point is on the lid.
                let th = d.lid.angle_deg * PI / 180.0;
                let (nxl, nzl) = (-th.sin(), th.cos());
                let dist = nxl * ear.x + nzl * (ear.z - d.lid.hinge_height); // signed distance of the ear from the lid plane
                if dist < 0.0 {
                    // the ear is on the underside of the lid, where the reflection goes
                    let (mx, mz) = (ear.x - 2.0 * dist * nxl, ear.z - 2.0 * dist * nzl);
                    for (pi, cp) in centre.iter().enumerate() {
                        // Where the straight line from the patch to the mirrored ear crosses the lid.
                        let sd = nxl * cp.x + nzl * (0.0 - d.lid.hinge_height);
                        let denom = nxl * (mx - cp.x) + nzl * mz;
                        if denom.abs() < 1e-9 {
                            continue;
                        }
                        let u = -sd / denom;
                        if u <= 0.0 || u >= 1.0 {
                            continue;
                        }
                        let (hx, hy) = (cp.x + u * (mx - cp.x), cp.y + u * (ear.y - cp.y));
                        if hx < 0.0 || hx > d.lid.width * th.cos() || hy < 0.0 || hy > d.lid.length {
                            continue;
                        }
                        let r = ((cp.x - mx).powi(2) + (cp.y - ear.y).powi(2) + mz * mz).sqrt();
                        idelay.push(delay_of(r));
                        isrc.push(pi);
                        igain.push(strength / r * d.lid.reflection);
                        max_delay = max_delay.max(delay_of(r));
                    }
                    images += 1;
                }
            }
            rad.img_delay[e] = idelay;
            rad.img_patch[e] = isrc;
            rad.img_gain[e] = igain;
        }
        rad.image_count = images / 2;

        let ring_len = (max_delay + 4).next_power_of_two();
        rad.ring_mask = ring_len - 1;
        rad.ring = [vec![0.0; ring_len], vec![0.0; ring_len]];

        // Late field. Sabine: T60 = 0.161 V / A. A steady source of volume acceleration q gives a
        // diffuse field of mean square pressure rho^2 q^2 / (pi A); the network is scaled so that
        // its response to white input has that energy, less the part the mirror images above
        // already carry (everything arriving within about two mean free paths).
        rad.reverb = room.enabled;
        if rad.reverb {
            let volume = room.lx * room.ly * room.lz;
            let surface = 2.0 * (room.lx * room.ly + room.lx * room.lz + room.ly * room.lz);
            let absorb = room.absorption * surface;
            rad.reverb_time = room_decay_time(room.lx, room.ly, room.lz, room.absorption);
            let base = [0.0297, 0.0371, 0.0411, 0.0437, 0.0533, 0.0617, 0.0683, 0.0739];
            for i in 0..LINES {
                let len = (base[i] * fs) as usize | 1;
                rad.line[i] = vec![0.0; len];
                rad.line_gain[i] = 10.0f64.powf(-3.0 * len as f64 / (fs * rad.reverb_time));
            }
            rad.lp_coef = 1.0 - (-2.0 * PI * 6000.0 / fs).exp(); // air and surfaces take the top off the tail
            rad.reverb_gain = 1.0;
            // Calibrate with the high-pass bypassed: feed the network directly.
            rad.hp_coef = -1.0;
            let mut energy = 0.0;
            let steps = (3.0 * rad.reverb_time * fs) as usize;
            for n in 0..steps {
                let (l, r) = rad.reverb_tick(if n == 0 { 1.0 } else { 0.0 });
                energy += 0.5 * (l * l + r * r);
            }
            for i in 0..LINES {
                rad.line[i].iter_mut().for_each(|x| *x = 0.0);
                rad.line_lp[i] = 0.0;
                rad.line_pos[i] = 0;
            }
            rad.hp_coef = (-2.0 * PI * 120.0 / fs).exp(); // the two sides of the board cancel at low frequency
            rad.hp_state = 0.0;
            let mean_free_path = 4.0 * volume / surface;
            let early = (-13.8 * (2.0 * mean_free_path / c) / rad.reverb_time).exp();
            let target = rho * rho / (PI * absorb) * early; // per unit volume acceleration squared
            rad.reverb_gain = (target / energy).sqrt() * ds / (k * k); // input is the sum of dt^2 * accel
        }
        rad
    }

    fn reverb_tick(&mut self, input: f64) -> (f64, f64) {
        let mut x = input;
        if self.hp_coef >= 0.0 {
            // one-pole high-pass
            self.hp_state = self.hp_coef * self.hp_state + (1.0 - self.hp_coef) * input;
            x = input - self.hp_state;
        }
        let mut y = [0.0; LINES];
        let mut sum = 0.0;
        for i in 0..LINES {
            let out = self.line[i][self.line_pos[i]] * self.line_gain[i];
            self.line_lp[i] += self.lp_coef * (out - self.line_lp[i]);
            y[i] = self.line_lp[i];
            sum += y[i];
        }
        sum *= 2.0 / LINES as f64; // Householder reflection: lossless mixing of the lines
        for i in 0..LINES {
            let pos = self.line_pos[i];
            self.line[i][pos] = y[i] - sum + if i & 1 == 1 { -x } else { x };
            self.line_pos[i] = if pos + 1 == self.line[i].len() { 0 } else { pos + 1 };
        }
        let g = self.reverb_gain;
        (g * (y[0] - y[1] + y[2] - y[3] + y[4] - y[5] + y[6] - y[7]), g * (y[0] + y[1] - y[2] - y[3] + y[4] + y[5] - y[6] - y[7]))
    }

    /// One-time proof after a real zero-input tick, not a tail/silence threshold.
    pub(crate) fn is_pristine_zero(&self) -> bool {
        let zero = |s: &[f64]| s.iter().all(|&v| v == 0.0);
        self.ring.iter().all(|s| zero(s)) && self.line.iter().all(|s| zero(s))
            && zero(&self.patch_acc) && zero(&self.patch_vel) && zero(&self.air_force)
            && self.vel_hist.iter().chain(&self.acc_hist).all(|&v| v == 0.0)
            && zero(&self.line_lp) && self.hp_state == 0.0
    }

    /// Only the constructor-proven zero fixed point may use this. Histories remain
    /// zero, but all delay clocks retain the same phase as ordinary stepping.
    pub(crate) fn advance_pristine_zero(&mut self) {
        self.ring_pos = (self.ring_pos + 1) & self.ring_mask;
        if self.air {
            self.hist_pos = (self.hist_pos + 1) & self.hist_mask;
        }
        if self.reverb {
            for i in 0..LINES {
                self.line_pos[i] = if self.line_pos[i] + 1 == self.line[i].len() { 0 } else { self.line_pos[i] + 1 };
            }
        }
    }

    /// `r`: change of the plate increment this step (dt^2 * acceleration), grid storage.
    /// Returns the pressure at the two ears. Allocation free.
    pub fn step(&mut self, r: &[f64]) -> (f64, f64) {
        // Patch sums: total motion of each patch.
        let np = self.patch_count;
        let mut total = 0.0;
        for p in 0..np {
            let nodes = &self.patch_node[self.patch_ptr[p]..self.patch_ptr[p + 1]];
            let (mut s0, mut s1) = (0.0, 0.0);
            let mut pairs = nodes.chunks_exact(2);
            for pair in &mut pairs {
                unsafe {
                    s0 += *r.get_unchecked(pair[0]);
                    s1 += *r.get_unchecked(pair[1]);
                }
            }
            if let [last] = pairs.remainder() {
                s0 += r[*last];
            }
            self.patch_acc[p] = s0 + s1;
            total += s0 + s1;
        }
        if self.air {
            let now = self.hist_pos;
            let (vm, hm) = (self.vel_mask, self.hist_mask);
            for p in 0..np {
                self.patch_vel[p] += self.patch_acc[p] * self.inv_count[p];
                self.vel_hist[(now & vm) * np + p] = self.patch_vel[p] as f32;
                self.acc_hist[now * np + p] = self.patch_acc[p] as f32;
            }
            let ah = &self.acc_hist;
            for p in 0..np {
                let f = self.self_gain * (self.patch_vel[p] - self.vel_hist[(now.wrapping_sub(self.self_delay[p]) & vm) * np + p] as f64);
                let (mut f0, mut f1) = (0.0f32, 0.0f32);
                let (lo, hi) = (self.mut_ptr[p], self.mut_ptr[p + 1]);
                let (offset, mg) = (&self.mut_offset[lo..hi], &self.mut_gain[lo..hi]);
                let base = now * np;
                let history_index = |offset: usize| mutual_history_index(base, offset, ah.len());
                let mut q = 0;
                unsafe {
                    while q + 1 < offset.len() {
                        f0 += mg.get_unchecked(q) * ah.get_unchecked(history_index(*offset.get_unchecked(q)));
                        f1 += mg.get_unchecked(q + 1) * ah.get_unchecked(history_index(*offset.get_unchecked(q + 1)));
                        q += 2;
                    }
                    if q < offset.len() {
                        f0 += mg.get_unchecked(q) * ah.get_unchecked(history_index(*offset.get_unchecked(q)));
                    }
                }
                self.air_force[p] = f + (f0 + f1) as f64;
            }
            self.hist_pos = (now + 1) & hm;
        }
        let mask = self.ring_mask;
        let pos = self.ring_pos;
        for e in 0..2 {
            let ring = &mut self.ring[e];
            {
                // Direct sound: many nodes share each delay, so sum per delay.
                let b = &self.node[e];
                let base = pos + b.first;
                for k in 0..b.ptr.len() - 1 {
                    let (lo, hi) = (b.ptr[k], b.ptr[k + 1]);
                    let (idx, g) = (&b.idx[lo..hi], &b.gain[lo..hi]);
                    // Four independent partial sums, so the additions do not wait on each other.
                    let (mut s0, mut s1, mut s2, mut s3) = (0.0, 0.0, 0.0, 0.0);
                    let mut q = 0;
                    unsafe {
                        while q + 3 < idx.len() {
                            s0 += g.get_unchecked(q) * r.get_unchecked(*idx.get_unchecked(q));
                            s1 += g.get_unchecked(q + 1) * r.get_unchecked(*idx.get_unchecked(q + 1));
                            s2 += g.get_unchecked(q + 2) * r.get_unchecked(*idx.get_unchecked(q + 2));
                            s3 += g.get_unchecked(q + 3) * r.get_unchecked(*idx.get_unchecked(q + 3));
                            q += 4;
                        }
                        while q < idx.len() {
                            s0 += g.get_unchecked(q) * r.get_unchecked(*idx.get_unchecked(q));
                            q += 1;
                        }
                    }
                    ring[(base + k) & mask] += (s0 + s1) + (s2 + s3);
                }
            }
            // Reflections: delays are spread thin, so add each one where it lands.
            let (dl, src, g) = (&self.img_delay[e], &self.img_patch[e], &self.img_gain[e]);
            for q in 0..dl.len() {
                ring[(pos + dl[q]) & mask] += g[q] * self.patch_acc[src[q]];
            }
        }
        let mut left = std::mem::take(&mut self.ring[0][pos]);
        let mut right = std::mem::take(&mut self.ring[1][pos]);
        self.ring_pos = (pos + 1) & mask;
        if self.reverb {
            let (l, rr) = self.reverb_tick(total);
            left += l;
            right += rr;
        }
        (left, right)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::{make_grand, GrandOptions};

    #[test]
    fn room_decay_preserves_calibration_operation_order() {
        for (lx, ly, lz, absorption) in [
            (9.0, 7.0, 4.0, 0.18),
            (3.0, 3.0, 2.2, 0.03),
            (30.0, 25.0, 15.0, 0.7),
            (12.345, 8.765, 3.456, 0.234),
        ] {
            // Frozen pre-extraction calculation: catch numerically different reassociation.
            let volume = lx * ly * lz;
            let surface = 2.0 * (lx * ly + lx * lz + ly * lz);
            let absorb = absorption * surface;
            let previous: f64 = 0.161 * volume / absorb;
            assert_eq!(room_decay_time(lx, ly, lz, absorption).to_bits(), previous.to_bits());
        }
    }

    fn same_radiation(a: &Radiation, b: &Radiation) {
        let bits = |s: &[f64]| s.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        for (a, b) in a.ring.iter().chain(&a.line).zip(b.ring.iter().chain(&b.line)) {
            assert_eq!(bits(a), bits(b));
        }
        let arrays: [(&[f64], &[f64]); 4] = [(&a.patch_acc, &b.patch_acc), (&a.patch_vel, &b.patch_vel),
            (&a.air_force, &b.air_force), (&a.line_lp, &b.line_lp)];
        for (a, b) in arrays {
            assert_eq!(bits(a), bits(b));
        }
        for (a, b) in a.vel_hist.iter().chain(&a.acc_hist).zip(b.vel_hist.iter().chain(&b.acc_hist)) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        assert_eq!(a.hp_state.to_bits(), b.hp_state.to_bits());
        assert_eq!(a.ring_pos, b.ring_pos);
        assert_eq!(a.hist_pos, b.hist_pos);
        assert_eq!(a.line_pos, b.line_pos);
    }

    #[test]
    fn pristine_zero_clocks_match_full_radiation_across_all_wraps() {
        let fs = 8000.0;
        for room in [false, true] {
            for air in [false, true] {
                let d = make_grand(&GrandOptions { room_enabled: room, air_loading: air, ..GrandOptions::default() });
                let plate = Plate::new(&d.board, &[], 1.0 / fs, 2.0).unwrap();
                let mut fast = Radiation::new(&plate, &d, fs);
                let mut normal = Radiation::new(&plate, &d, fs);
                let mut input = vec![0.0; plate.grid_size];
                assert_eq!(fast.step(&input), normal.step(&input));
                assert!(fast.is_pristine_zero());
                let longest = fast.ring[0].len().max(fast.hist_mask + 1)
                    .max(fast.line.iter().map(Vec::len).max().unwrap());
                for tick in 0..2 * longest + 3 {
                    fast.advance_pristine_zero();
                    let out = normal.step(&input);
                    assert_eq!((out.0.to_bits(), out.1.to_bits()), (0.0f64.to_bits(), 0.0f64.to_bits()));
                    assert_eq!(fast.ring_pos, normal.ring_pos, "ring tick {tick}");
                    assert_eq!(fast.hist_pos, normal.hist_pos, "air tick {tick}");
                    assert_eq!(fast.line_pos, normal.line_pos, "FDN tick {tick}");
                }
                assert!(fast.is_pristine_zero() && normal.is_pristine_zero());
                same_radiation(&fast, &normal);
                for tick in 0..2 * longest + 3 {
                    input[plate.gid[0]] = if tick == 0 { 1e-12 } else { 0.0 };
                    let a = fast.step(&input);
                    let b = normal.step(&input);
                    assert_eq!((a.0.to_bits(), a.1.to_bits()), (b.0.to_bits(), b.1.to_bits()), "room={room} air={air} active tick {tick}");
                    for (a, b) in fast.air_force.iter().zip(&normal.air_force) {
                        assert_eq!(a.to_bits(), b.to_bits());
                    }
                }
                same_radiation(&fast, &normal);
                let mut pristine = Radiation::new(&plate, &d, fs);
                pristine.hp_state = f64::NAN;
                assert!(!pristine.is_pristine_zero());
                pristine.hp_state = 0.0;
                if let Some(slot) = pristine.acc_hist.last_mut() {
                    *slot = f32::INFINITY;
                    assert!(!pristine.is_pristine_zero());
                }
            }
        }
    }

    #[test]
    fn flattened_history_all_positions_delays_and_sources() {
        for np in [1usize, 3, 8, 40] {
            for slots in [2usize, 4, 16, 128] {
                for now in 0..slots {
                    for delay in 1..slots {
                        for src in 0..np {
                            let actual = mutual_history_index(now * np, delay * np - src, slots * np);
                            let expected = (now.wrapping_sub(delay) & (slots - 1)) * np + src;
                            assert_eq!(actual, expected);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn mutual_air_flat_offsets_match_time_patch_reference_across_wraps() {
        let mut d = make_grand(&GrandOptions::default());
        d.room.enabled = false;
        d.air_loading = true;
        let plate = Plate::new(&d.board, &[], 1.0 / 48000.0, 1.0).unwrap();
        let mut rad = Radiation::new(&plate, &d, 48000.0);
        let np = rad.patch_count;
        assert!(np > 1);
        let mut input = vec![0.0; plate.grid_size];
        for tick in 0..2 * (rad.hist_mask + 1) + 3 {
            for (i, x) in input.iter_mut().enumerate() {
                *x = ((i + tick) as f64 * 0.31).sin() * 1e-12;
            }
            let now = rad.hist_pos;
            rad.step(&input);
            for p in 0..np {
                let self_force = rad.self_gain * (rad.patch_vel[p]
                    - rad.vel_hist[(now.wrapping_sub(rad.self_delay[p]) & rad.vel_mask) * np + p] as f64);
                let (mut f0, mut f1) = (0.0f32, 0.0f32);
                for (lane, q) in (rad.mut_ptr[p]..rad.mut_ptr[p + 1]).enumerate() {
                    // Original source order is 0..np, excluding p; derive delay only, then
                    // evaluate the original time-slot mask and source indexing independently.
                    let src = if lane < p { lane } else { lane + 1 };
                    let delay = (rad.mut_offset[q] + src) / np;
                    let value = rad.mut_gain[q]
                        * rad.acc_hist[(now.wrapping_sub(delay) & rad.hist_mask) * np + src];
                    if lane & 1 == 0 { f0 += value; } else { f1 += value; }
                }
                assert_eq!(rad.air_force[p].to_bits(), (self_force + (f0 + f1) as f64).to_bits());
            }
        }
    }
}
