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

use crate::design::{point_in_polygon, LidDesign, PianoDesign, RoomDesign, RoomShape, Vec2};
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

/// Exact volume (m^3) and total surface area (m^2) for any room shape and sloped ceiling.
pub fn room_geometry_metrics(room: &RoomDesign) -> (f64, f64) {
    if room.is_plain_shoebox() {
        let volume = room.lx * room.ly * room.lz;
        let surface = 2.0 * (room.lx * room.ly + room.lx * room.lz + room.ly * room.lz);
        return (volume, surface);
    }
    let poly = room.floor_polygon();
    let n = poly.len();
    let mut area2 = 0.0;
    let mut cx_num = 0.0;
    let mut wall_area = 0.0;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let cross = a.x * b.y - b.x * a.y;
        area2 += cross;
        cx_num += (a.x + b.x) * cross;
        let len = (b.x - a.x).hypot(b.y - a.y);
        wall_area += len * 0.5 * (room.ceiling_z_at(a.x) + room.ceiling_z_at(b.x));
    }
    let floor_area = (0.5 * area2).abs().max(1e-3);
    let cx = if area2.abs() > 1e-9 { cx_num / (3.0 * area2) } else { 0.5 * room.lx };
    let slope = room.ceiling_slope_tan();
    let ceiling_area = floor_area * slope.hypot(1.0);
    let volume = (floor_area * room.ceiling_z_at(cx)).max(1e-3);
    let surface = (floor_area + ceiling_area + wall_area).max(1e-3);
    (volume, surface)
}

#[derive(Clone, Copy)]
enum FacetKind {
    Floor,
    Ceiling,
    Wall { a: Vec2, b: Vec2, len2: f64 },
}

#[derive(Clone, Copy)]
struct Facet {
    /// Unit inward normal.
    n: [f64; 3],
    /// Plane equation: n . p = d.
    d: f64,
    kind: FacetKind,
}

impl Facet {
    #[inline]
    fn signed_dist(&self, p: [f64; 3]) -> f64 {
        self.n[0] * p[0] + self.n[1] * p[1] + self.n[2] * p[2] - self.d
    }

    #[inline]
    fn mirror(&self, p: [f64; 3]) -> [f64; 3] {
        let dist = self.signed_dist(p);
        [p[0] - 2.0 * dist * self.n[0], p[1] - 2.0 * dist * self.n[1], p[2] - 2.0 * dist * self.n[2]]
    }

    #[inline]
    fn intersect_segment(&self, p0: [f64; 3], p1: [f64; 3]) -> Option<(f64, [f64; 3])> {
        let s0 = self.signed_dist(p0);
        let s1 = self.signed_dist(p1);
        if s0 * s1 >= 0.0 {
            return None;
        }
        let denom = s0 - s1;
        if denom.abs() < 1e-12 {
            return None;
        }
        let u = s0 / denom;
        if u <= 1e-6 || u >= 1.0 - 1e-6 {
            return None;
        }
        Some((u, [p0[0] + u * (p1[0] - p0[0]), p0[1] + u * (p1[1] - p0[1]), p0[2] + u * (p1[2] - p0[2])]))
    }

    fn contains(&self, p: [f64; 3], poly: &[Vec2], room: &RoomDesign) -> bool {
        match self.kind {
            FacetKind::Floor | FacetKind::Ceiling => point_in_polygon(poly, Vec2 { x: p[0], y: p[1] }),
            FacetKind::Wall { a, b, len2 } => {
                if len2 < 1e-12 {
                    return false;
                }
                let t = ((p[0] - a.x) * (b.x - a.x) + (p[1] - a.y) * (b.y - a.y)) / len2;
                if !(-1e-4..=1.0 + 1e-4).contains(&t) {
                    return false;
                }
                let cz = room.ceiling_z_at(p[0]);
                p[2] >= -1e-4 && p[2] <= cz + 1e-4
            }
        }
    }
}

fn build_room_facets(room: &RoomDesign, poly: &[Vec2]) -> Vec<Facet> {
    let mut facets = Vec::with_capacity(poly.len() + 2);
    // Floor: z = 0, inward normal +z.
    facets.push(Facet { n: [0.0, 0.0, 1.0], d: 0.0, kind: FacetKind::Floor });
    // Ceiling: z = lz + (x - 0.5*lx) * s -> s*x - z = 0.5*lx*s - lz, inward normal (s, 0, -1)/norm.
    let s = room.ceiling_slope_tan();
    let inv = 1.0 / s.hypot(1.0);
    let cn = [s * inv, 0.0, -inv];
    let cd = cn[0] * (0.5 * room.lx) + cn[2] * room.lz;
    facets.push(Facet { n: cn, d: cd, kind: FacetKind::Ceiling });
    // Vertical walls (CCW polygon -> inward normal (-dy, dx)/len).
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len2 = dx * dx + dy * dy;
        let len = len2.sqrt();
        if len < 1e-9 {
            continue;
        }
        let n = [-dy / len, dx / len, 0.0];
        let d = n[0] * a.x + n[1] * a.y;
        facets.push(Facet { n, d, kind: FacetKind::Wall { a, b, len2 } });
    }
    facets
}

fn segment_occluded(
    p0: [f64; 3],
    p1: [f64; 3],
    facets: &[Facet],
    poly: &[Vec2],
    room: &RoomDesign,
    ignore_a: usize,
    ignore_b: usize,
) -> bool {
    for (idx, facet) in facets.iter().enumerate() {
        if idx == ignore_a || idx == ignore_b {
            continue;
        }
        if let Some((u, hit)) = facet.intersect_segment(p0, p1) {
            if u > 1e-4 && u < 1.0 - 1e-4 && facet.contains(hit, poly, room) {
                return true;
            }
        }
    }
    false
}

/// First-order geometric reflection bounce points (room coordinates, metres) for UI visualization,
/// computed using the exact same planar mirror equations as `Radiation::new`.
#[derive(Clone, Debug, Default)]
pub struct RoomPreviewRays {
    pub wall_hits: Vec<[f64; 3]>,
    pub floor_hit: Option<[f64; 3]>,
    pub ceiling_hit: Option<[f64; 3]>,
    pub lid_hit: Option<[f64; 3]>,
}

pub fn room_preview_rays(
    room: &RoomDesign,
    lid: &LidDesign,
    source_room: [f64; 3],
    listener_room: [f64; 3],
) -> RoomPreviewRays {
    let mut out = RoomPreviewRays::default();
    if room.enabled {
        let poly = room.floor_polygon();
        let facets = build_room_facets(room, &poly);
        for (idx, facet) in facets.iter().enumerate() {
            if facet.signed_dist(listener_room) <= 1e-6 || facet.signed_dist(source_room) <= 1e-6 {
                continue;
            }
            let mir = facet.mirror(listener_room);
            let Some((_, hit)) = facet.intersect_segment(source_room, mir) else { continue; };
            if !facet.contains(hit, &poly, room) {
                continue;
            }
            if segment_occluded(source_room, hit, &facets, &poly, room, idx, usize::MAX)
                || segment_occluded(hit, listener_room, &facets, &poly, room, idx, usize::MAX)
            {
                continue;
            }
            match facet.kind {
                FacetKind::Floor => out.floor_hit = Some(hit),
                FacetKind::Ceiling => out.ceiling_hit = Some(hit),
                FacetKind::Wall { .. } => out.wall_hits.push(hit),
            }
        }
    }
    if lid.open {
        let th = lid.angle_deg * PI / 180.0;
        let (nxl, nzl) = (-th.sin(), th.cos());
        let ex = listener_room[0] - room.piano_x;
        let ey = listener_room[1] - room.piano_y;
        let ez = listener_room[2] - room.board_height;
        let sx = source_room[0] - room.piano_x;
        let sy = source_room[1] - room.piano_y;
        let dist = nxl * ex + nzl * (ez - lid.hinge_height);
        if dist < 0.0 {
            let (mx, mz) = (ex - 2.0 * dist * nxl, ez - 2.0 * dist * nzl);
            let sd = nxl * sx + nzl * (0.0 - lid.hinge_height);
            let denom = nxl * (mx - sx) + nzl * mz;
            if denom.abs() > 1e-9 {
                let u = -sd / denom;
                if u > 0.0 && u < 1.0 {
                    let hx = sx + u * (mx - sx);
                    let hy = sy + u * (ey - sy);
                    let hz = u * mz;
                    if hx >= 0.0 && hx <= lid.width * th.cos() && hy >= 0.0 && hy <= lid.length {
                        out.lid_hit = Some([hx + room.piano_x, hy + room.piano_y, hz + room.board_height]);
                    }
                }
            }
        }
    }
    out
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
    // Optional Woodworth contralateral head-shadow split (empty when disabled).
    binaural: bool,
    node_shadow: [Bins; 2],
    img_shadow_gain: [Vec<f64>; 2],
    ring_shadow: [Vec<f64>; 2],
    head_lp: [f64; 2],
    head_lp_coef: f64,
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
        rad.binaural = room.binaural_head;
        rad.head_lp_coef = 1.0 - (-2.0 * PI * 1400.0 / fs).exp();
        // Outward unit normal for each ear (left ear points toward ear[0]-ear[1], right opposite).
        let ear_axis = [d.ears[1].x - d.ears[0].x, d.ears[1].y - d.ears[0].y, d.ears[1].z - d.ears[0].z];
        let ear_axis_len = (ear_axis[0] * ear_axis[0] + ear_axis[1] * ear_axis[1] + ear_axis[2] * ear_axis[2]).sqrt().max(1e-6);
        let ear_outward = [
            [-ear_axis[0] / ear_axis_len, -ear_axis[1] / ear_axis_len, -ear_axis[2] / ear_axis_len],
            [ear_axis[0] / ear_axis_len, ear_axis[1] / ear_axis_len, ear_axis[2] / ear_axis_len],
        ];
        let shadow_frac = |e: usize, dx: f64, dy: f64, dz: f64, r: f64| -> f64 {
            if !rad.binaural || r < 1e-6 {
                return 0.0;
            }
            let n = ear_outward[e];
            let cos_t = ((dx * n[0] + dy * n[1] + dz * n[2]) / r).clamp(-1.0, 1.0);
            ((1.0 - cos_t) * 0.5).powf(1.4) * 0.58
        };

        for e in 0..2 {
            let ear = d.ears[e];
            // Direct sound, node by node. Delays are rounded to the nearest step (a path error
            // under 4 mm at 48 kHz, well below the grid spacing).
            let (mut delay, mut src, mut gain) = (Vec::with_capacity(p.n), Vec::with_capacity(p.n), Vec::with_capacity(p.n));
            let mut sgain = if rad.binaural { Vec::with_capacity(p.n) } else { Vec::new() };
            for a in 0..p.n {
                let pos = p.node_position(a);
                let (dx, dy, dz) = (pos.x - ear.x, pos.y - ear.y, -ear.z);
                let r = (dx * dx + dy * dy + dz * dz).sqrt();
                let g_full = strength / r * if ear.z >= 0.0 { 1.0 } else { -1.0 };
                let sh = shadow_frac(e, dx, dy, dz, r);
                delay.push(delay_of(r));
                src.push(p.gid[a]);
                if rad.binaural {
                    gain.push(g_full * (1.0 - sh));
                    sgain.push(g_full * sh);
                } else {
                    gain.push(g_full);
                }
                max_delay = max_delay.max(delay[a]);
            }
            rad.node[e] = Bins::build(&delay, &src, &gain);
            if rad.binaural {
                rad.node_shadow[e] = Bins::build(&delay, &src, &sgain);
            }

            let (mut idelay, mut isrc, mut igain): (Vec<usize>, Vec<usize>, Vec<f64>) = (Vec::new(), Vec::new(), Vec::new());
            let mut isgain: Vec<f64> = Vec::new();
            let mut push_img = |dl: usize, pi: usize, g_full: f64, dx: f64, dy: f64, dz: f64, r: f64| {
                idelay.push(dl);
                isrc.push(pi);
                if rad.binaural {
                    let sh = shadow_frac(e, dx, dy, dz, r);
                    igain.push(g_full * (1.0 - sh));
                    isgain.push(g_full * sh);
                } else {
                    igain.push(g_full);
                }
            };
            if room.enabled {
                let ear_room = [ear.x + room.piano_x, ear.y + room.piano_y, ear.z + room.board_height];
                let refl = (1.0 - room.absorption).sqrt();
                if room.is_plain_shoebox() {
                    // Shoebox images of the ear, up to two reflections. Room coordinates put the floor
                    // at z = 0; the board sits at board_height. A path that leaves upward comes from
                    // the top side, downward from the underside.
                    let l = [room.lx, room.ly, room.lz];
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
                                    let (dx, dy, dz) = (cp.x - ex, cp.y - ey, -ez);
                                    let r = (dx * dx + dy * dy + dz * dz).sqrt();
                                    let dl = delay_of(r);
                                    push_img(dl, pi, strength / r * amplitude * if ez >= 0.0 { 1.0 } else { -1.0 }, dx, dy, dz, r);
                                    max_delay = max_delay.max(dl);
                                }
                                images += 1;
                            }
                        }
                    }
                } else {
                    // Generalized planar image-source method (Borish) for non-rectangular rooms
                    // and sloped ceilings, up to 2nd order with visibility & occlusion checks.
                    let poly = room.floor_polygon();
                    let facets = build_room_facets(room, &poly);
                    let nf = facets.len();
                    let non_convex = room.shape == RoomShape::LAlcove;
                    for i in 0..nf {
                        if facets[i].signed_dist(ear_room) <= 1e-6 {
                            continue;
                        }
                        let im1 = facets[i].mirror(ear_room);
                        let (ex1, ey1, ez1) = (im1[0] - room.piano_x, im1[1] - room.piano_y, im1[2] - room.board_height);
                        let mut any1 = false;
                        for (pi, cp) in centre.iter().enumerate() {
                            let src_pt = [cp.x + room.piano_x, cp.y + room.piano_y, room.board_height];
                            let Some((_, hit)) = facets[i].intersect_segment(src_pt, im1) else { continue; };
                            if !facets[i].contains(hit, &poly, room) {
                                continue;
                            }
                            if non_convex && (segment_occluded(src_pt, hit, &facets, &poly, room, i, usize::MAX)
                                || segment_occluded(hit, ear_room, &facets, &poly, room, i, usize::MAX)) {
                                continue;
                            }
                            let r = ((cp.x - ex1).powi(2) + (cp.y - ey1).powi(2) + ez1 * ez1).sqrt().max(1e-3);
                            let dl = delay_of(r);
                            let (hdx, hdy, hdz) = (hit[0] - ear_room[0], hit[1] - ear_room[1], hit[2] - ear_room[2]);
                            let hr = (hdx * hdx + hdy * hdy + hdz * hdz).sqrt().max(1e-3);
                            push_img(dl, pi, strength / r * refl * if hit[2] >= room.board_height { 1.0 } else { -1.0 }, hdx, hdy, hdz, hr);
                            max_delay = max_delay.max(dl);
                            any1 = true;
                        }
                        if any1 {
                            images += 1;
                        }
                        // 2nd-order image: ear reflected across i, then across j != i.
                        for j in 0..nf {
                            if j == i || facets[j].signed_dist(im1) <= 1e-6 {
                                continue;
                            }
                            let im2 = facets[j].mirror(im1);
                            let (ex2, ey2, ez2) = (im2[0] - room.piano_x, im2[1] - room.piano_y, im2[2] - room.board_height);
                            let amp2 = refl * refl;
                            let mut any2 = false;
                            for (pi, cp) in centre.iter().enumerate() {
                                let src_pt = [cp.x + room.piano_x, cp.y + room.piano_y, room.board_height];
                                let Some((_, hit2)) = facets[j].intersect_segment(src_pt, im2) else { continue; };
                                if !facets[j].contains(hit2, &poly, room) {
                                    continue;
                                }
                                let Some((_, hit1)) = facets[i].intersect_segment(hit2, im1) else { continue; };
                                if !facets[i].contains(hit1, &poly, room) {
                                    continue;
                                }
                                if non_convex && (segment_occluded(src_pt, hit2, &facets, &poly, room, j, usize::MAX)
                                    || segment_occluded(hit2, hit1, &facets, &poly, room, i, j)
                                    || segment_occluded(hit1, ear_room, &facets, &poly, room, i, usize::MAX)) {
                                    continue;
                                }
                                let r = ((cp.x - ex2).powi(2) + (cp.y - ey2).powi(2) + ez2 * ez2).sqrt().max(1e-3);
                                let dl = delay_of(r);
                                let (hdx, hdy, hdz) = (hit1[0] - ear_room[0], hit1[1] - ear_room[1], hit1[2] - ear_room[2]);
                                let hr = (hdx * hdx + hdy * hdy + hdz * hdz).sqrt().max(1e-3);
                                push_img(dl, pi, strength / r * amp2 * if hit2[2] >= room.board_height { 1.0 } else { -1.0 }, hdx, hdy, hdz, hr);
                                max_delay = max_delay.max(dl);
                                any2 = true;
                            }
                            if any2 {
                                images += 1;
                            }
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
                        let dl = delay_of(r);
                        let (hdx, hdy, hdz) = (hx - ear.x, hy - ear.y, u * mz - ear.z);
                        let hr = (hdx * hdx + hdy * hdy + hdz * hdz).sqrt().max(1e-3);
                        push_img(dl, pi, strength / r * d.lid.reflection, hdx, hdy, hdz, hr);
                        max_delay = max_delay.max(dl);
                    }
                    images += 1;
                }
            }
            rad.img_delay[e] = idelay;
            rad.img_patch[e] = isrc;
            rad.img_gain[e] = igain;
            rad.img_shadow_gain[e] = isgain;
        }
        rad.image_count = images / 2;

        let ring_len = (max_delay + 4).next_power_of_two();
        rad.ring_mask = ring_len - 1;
        rad.ring = [vec![0.0; ring_len], vec![0.0; ring_len]];
        if rad.binaural {
            rad.ring_shadow = [vec![0.0; ring_len], vec![0.0; ring_len]];
        }

        // Late field. Sabine: T60 = 0.161 V / A. A steady source of volume acceleration q gives a
        // diffuse field of mean square pressure rho^2 q^2 / (pi A); the network is scaled so that
        // its response to white input has that energy, less the part the mirror images above
        // already carry (everything arriving within about two mean free paths).
        rad.reverb = room.enabled;
        if rad.reverb {
            let (volume, surface) = room_geometry_metrics(room);
            let absorb = room.absorption * surface;
            rad.reverb_time = if room.is_plain_shoebox() {
                room_decay_time(room.lx, room.ly, room.lz, room.absorption)
            } else {
                0.161 * volume / absorb
            };
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
        self.ring.iter().all(|s| zero(s))
            && self.ring_shadow.iter().all(|s| zero(s))
            && zero(&self.head_lp)
            && self.line.iter().all(|s| zero(s))
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
            if self.binaural {
                let sring = &mut self.ring_shadow[e];
                let sb = &self.node_shadow[e];
                let base = pos + sb.first;
                for k in 0..sb.ptr.len() - 1 {
                    let (lo, hi) = (sb.ptr[k], sb.ptr[k + 1]);
                    let (idx, g) = (&sb.idx[lo..hi], &sb.gain[lo..hi]);
                    let mut s = 0.0;
                    for q in 0..idx.len() {
                        s += g[q] * r[idx[q]];
                    }
                    sring[(base + k) & mask] += s;
                }
                let sg = &self.img_shadow_gain[e];
                for q in 0..dl.len() {
                    sring[(pos + dl[q]) & mask] += sg[q] * self.patch_acc[src[q]];
                }
            }
        }
        let mut left = std::mem::take(&mut self.ring[0][pos]);
        let mut right = std::mem::take(&mut self.ring[1][pos]);
        if self.binaural {
            let sl = std::mem::take(&mut self.ring_shadow[0][pos]);
            let sr = std::mem::take(&mut self.ring_shadow[1][pos]);
            self.head_lp[0] += self.head_lp_coef * (sl - self.head_lp[0]);
            self.head_lp[1] += self.head_lp_coef * (sr - self.head_lp[1]);
            left += self.head_lp[0];
            right += self.head_lp[1];
        }
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

    #[test]
    fn non_rectangular_rooms_and_sloped_ceilings_produce_valid_reflections_and_metrics() {
        let fs = 48000.0;
        let base_opts = GrandOptions::default();
        let base_d = make_grand(&base_opts);
        let plate = Plate::new(&base_d.board, &[], 1.0 / fs, 1.5).unwrap();
        let base_rad = Radiation::new(&plate, &base_d, fs);
        for (shape, slope) in [
            (1.0, 0.0),
            (2.0, 0.0),
            (3.0, 0.0),
            (0.0, 8.0),
            (1.0, 6.0),
            (3.0, -7.0),
        ] {
            let mut o = base_opts;
            o.room_shape = shape;
            o.ceiling_slope_deg = slope;
            let d = make_grand(&o);
            let (vol, surf) = room_geometry_metrics(&d.room);
            assert!(vol.is_finite() && vol > 10.0 && surf.is_finite() && surf > 20.0);
            let mut rad = Radiation::new(&plate, &d, fs);
            assert!(rad.image_count >= 4, "shape={shape} slope={slope} images={}", rad.image_count);
            assert!(rad.reverb_time.is_finite() && rad.reverb_time > 0.1);
            let mut input = vec![0.0; plate.grid_size];
            let zeros = vec![0.0; plate.grid_size];
            input[plate.gid[0]] = 1e-9;
            let mut energy = 0.0;
            for tick in 0..4096 {
                let (l, r) = rad.step(if tick == 0 { &input } else { &zeros });
                assert!(l.is_finite() && r.is_finite());
                energy += l * l + r * r;
            }
            assert!(energy > 0.0);
            let rays = room_preview_rays(
                &d.room,
                &d.lid,
                [o.piano_x + 0.7, o.piano_y + 0.7, d.room.board_height],
                [o.listener_x, o.listener_y, o.ear_height_m],
            );
            assert!(rays.floor_hit.is_some() && rays.ceiling_hit.is_some());
            assert!(!rays.wall_hits.is_empty());
        }
        // Test wall splay, wall skew, and binaural head shadow
        let mut bo = base_opts;
        bo.wall_splay = 0.22;
        bo.wall_skew = -0.15;
        bo.binaural_head = true;
        let bd = make_grand(&bo);
        let mut brad = Radiation::new(&plate, &bd, fs);
        let mut b_energy = 0.0;
        let mut inp = vec![0.0; plate.grid_size];
        let zeros = vec![0.0; plate.grid_size];
        inp[plate.gid[0]] = 1e-9;
        for tick in 0..4096 {
            let (l, r) = brad.step(if tick == 0 { &inp } else { &zeros });
            assert!(l.is_finite() && r.is_finite());
            b_energy += l * l + r * r;
        }
        assert!(b_energy > 0.0 && brad.image_count >= 4);

        // Rectangular + 0 slope keeps exact default volume/surface/reverb time.
        let (v0, s0) = room_geometry_metrics(&base_d.room);
        assert_eq!(v0.to_bits(), (base_opts.room_lx * base_opts.room_ly * base_opts.room_lz).to_bits());
        assert_eq!(s0.to_bits(), (2.0 * (base_opts.room_lx * base_opts.room_ly + base_opts.room_lx * base_opts.room_lz + base_opts.room_ly * base_opts.room_lz)).to_bits());
        assert!(base_rad.reverb_time > 0.1);
    }
}
