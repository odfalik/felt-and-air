//! Soundboard: an orthotropic Kirchhoff plate on a grain-aligned grid, with ribs and bridges
//! as glued-on beams. The discrete stiffness is assembled from a discrete strain energy, so
//! it is symmetric positive semidefinite by construction for any outline and stiffener layout.
//!
//! Time stepping (done by `Piano`) is explicit for the bare plate and implicit (theta = 1/4)
//! for the stiffeners, whose fast bending waves would otherwise dictate a coarse grid.

use crate::design::{BoardDesign, Vec2};
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Default)]
pub struct Csr {
    pub n: usize,
    pub row_ptr: Vec<usize>,
    pub col: Vec<usize>,
    pub val: Vec<f64>,
}

impl Csr {
    /// y = A x
    pub fn mul(&self, x: &[f64], y: &mut [f64]) {
        for i in 0..self.n {
            let mut s = 0.0;
            for p in self.row_ptr[i]..self.row_ptr[i + 1] {
                s += self.val[p] * x[self.col[p]];
            }
            y[i] = s;
        }
    }
    /// x' A x
    pub fn quad(&self, x: &[f64]) -> f64 {
        let mut s = 0.0;
        for i in 0..self.n {
            let mut r = 0.0;
            for p in self.row_ptr[i]..self.row_ptr[i + 1] {
                r += self.val[p] * x[self.col[p]];
            }
            s += r * x[i];
        }
        s
    }
}

/// Bilinear interpolation onto plate nodes; dropped weights are zero.
#[derive(Clone, Copy, Debug, Default)]
pub struct Interp {
    pub idx: [usize; 4],
    pub w: [f64; 4],
}

impl Interp {
    pub fn eval(&self, f: &[f64]) -> f64 {
        self.w[0] * f[self.idx[0]] + self.w[1] * f[self.idx[1]] + self.w[2] * f[self.idx[2]] + self.w[3] * f[self.idx[3]]
    }
}

/// Something bolted to the board: lumped mass and a viscous dashpot that may be switched on
/// at run time. Both enter the stability bound. The attached point moves as
/// sum(coefficient * board displacement at point).
#[derive(Clone, Debug)]
pub struct Attachment {
    pub points: Vec<(Vec2, f64)>,
    pub mass: f64,
    pub dashpot: f64,
}

/// The same combination expressed on plate nodes (grid storage index).
#[derive(Clone, Copy, Debug, Default)]
pub struct Coupling {
    pub n: usize,
    pub idx: [usize; 8],
    pub w: [f64; 8],
}

impl Coupling {
    pub fn eval(&self, f: &[f64]) -> f64 {
        let mut s = 0.0;
        for q in 0..self.n {
            s += self.w[q] * f[self.idx[q]];
        }
        s
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: usize,
    pub len: usize,
}

struct RibRun {
    start: usize,
    len: usize,
    coef: usize,
}

struct BeamForm {
    idx: [usize; 12],
    c: [f64; 12],
    coef: f64,
}

pub struct Plate {
    /// Active nodes.
    pub n: usize,
    pub nx: usize,
    pub ny: usize,
    pub hx: f64,
    pub hy: f64,
    /// nx*ny -> active index.
    pub node_index: Vec<Option<usize>>,
    /// Active index -> grid coordinates.
    pub gi: Vec<usize>,
    pub gj: Vec<usize>,

    // Run time fields live on the padded rectangular grid, stored column by column (the
    // across-grain direction is contiguous), with zeros at every node outside the board.
    pub stride: usize,
    pub grid_size: usize,
    /// Active index -> grid storage index.
    pub gid: Vec<usize>,
    /// Contiguous runs of active nodes.
    pub spans: Vec<Span>,

    /// Lumped mass per node, kg.
    pub mass: Vec<f64>,
    /// Mass proportional damping per node, kg/s.
    pub c0: Vec<f64>,
    /// Full stiffness, N/m.
    pub k: Csr,
    /// The part of K that is integrated implicitly.
    pub k_imp: Csr,
    /// Stiffness-like damping, kg/s.
    pub d: Csr,

    /// Gershgorin upper bound on eig(M^-1 (dt^2 Kexp + 2 dt D)); < 4 is stable.
    pub stability_bound: f64,
    /// Power iteration estimate of the same quantity.
    pub stability_estimate: f64,
    pub stiff_nodes: usize,
    pub factor_nnz: usize,
    pub rib_columns: Vec<usize>,

    ex: [f64; 2],
    ey: [f64; 2],
    gx0: f64,
    gy0: f64,
    bridge_nodes: Vec<usize>,

    // Kernel data, all premultiplied by dt^2 (stiffness) or dt (damping).
    a0: f64,
    a1x: f64,
    a1y: f64,
    a2x: f64,
    a2y: f64,
    a11: f64,
    b0: f64,
    bx: f64,
    by: f64,
    c0dt: Vec<f64>,
    corr_row: Vec<usize>,
    corr_ptr: Vec<usize>,
    corr_col: Vec<usize>,
    corr_val: Vec<f64>,
    rib_runs: Vec<RibRun>,
    rib_coef: Vec<f64>,
    longest_rib: usize,
    beams: Vec<BeamForm>,

    // Cholesky factor of the stiffened block of A. Rib segments that hang off a bridge are
    // eliminated first; each is a pentadiagonal chain, and all chains are swept together, one
    // position per step, so the sweep vectorises across ribs. What is left (the bridges and
    // their junctions with the ribs) is a small general sparse factor.
    inv_diag: Vec<f64>,
    lanes: usize,
    levels: usize,
    slots: usize,
    core_n: usize,
    f1: Vec<f64>,
    f2: Vec<f64>,
    fd: Vec<f64>,
    g1: Vec<f64>,
    g2: Vec<f64>,
    gd: Vec<f64>,
    chain_slot: Vec<usize>,
    chain_gid: Vec<usize>,
    virt_slot: Vec<usize>,
    virt_core: Vec<usize>,
    core_gid: Vec<usize>,
    cf_ptr: Vec<usize>,
    cf_col: Vec<usize>,
    cf_val: Vec<f64>,
    c_inv_diag: Vec<f64>,
    cb_ptr: Vec<usize>,
    cb_row: Vec<usize>,
    cb_val: Vec<f64>,
}

/// A linear functional of the nodal values: sum c_k w[idx_k].
#[derive(Default, Clone)]
struct Form(Vec<(usize, f64)>);

impl Form {
    fn add(&mut self, id: Option<usize>, v: f64) {
        let Some(id) = id else { return };
        if v == 0.0 {
            return;
        }
        for e in self.0.iter_mut() {
            if e.0 == id {
                e.1 += v;
                return;
            }
        }
        self.0.push((id, v));
    }
    fn add_interp(&mut self, it: &Interp, v: f64) {
        for k in 0..4 {
            if it.w[k] != 0.0 {
                self.add(Some(it.idx[k]), v * it.w[k]);
            }
        }
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}

#[derive(Default)]
struct Triplets(Vec<(usize, usize, f64)>);

impl Triplets {
    /// Adds coef * (f g') to the matrix.
    fn outer(&mut self, f: &Form, g: &Form, coef: f64) {
        for a in &f.0 {
            for b in &g.0 {
                self.0.push((a.0, b.0, coef * a.1 * b.1));
            }
        }
    }
    fn quad(&mut self, f: &Form, coef: f64) {
        self.outer(f, f, coef);
    }
    fn compress(mut self, n: usize) -> Csr {
        self.0.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        let t = &self.0;
        let mut m = Csr { n, row_ptr: vec![0; n + 1], col: Vec::new(), val: Vec::new() };
        let mut i = 0;
        while i < t.len() {
            let mut j = i;
            let mut s = 0.0;
            while j < t.len() && t[j].0 == t[i].0 && t[j].1 == t[i].1 {
                s += t[j].2;
                j += 1;
            }
            m.col.push(t[i].1);
            m.val.push(s);
            m.row_ptr[t[i].0 + 1] += 1;
            i = j;
        }
        for i in 0..n {
            m.row_ptr[i + 1] += m.row_ptr[i];
        }
        m
    }
}

/// Longest rib run `apply_local` can hold on its stack.
const RIB_SCRATCH: usize = 1024;

/// The 13 point plate stencil on w and its 5 point damping twin on v, for one span.
#[inline]
fn span_kernel(w: &[f64], v: &[f64], c0: &[f64], rs: &mut [f64], start: usize, s: usize, c: &[f64; 9]) {
    let [a0, a1x, a1y, a2x, a2y, a11, b0, bx, by] = *c;
    assert!(start >= 2 * s + 2 && start + rs.len() + 2 * s + 2 <= w.len() && v.len() == w.len() && c0.len() == w.len());
    for (t, out) in rs.iter_mut().enumerate() {
        let i = start + t;
        unsafe {
            let g = |x: &[f64], j: usize| *x.get_unchecked(j);
            let kw = a11.mul_add(
                g(w, i - s - 1) + g(w, i - s + 1) + g(w, i + s - 1) + g(w, i + s + 1),
                a2y.mul_add(g(w, i - 2) + g(w, i + 2), a2x.mul_add(g(w, i - 2 * s) + g(w, i + 2 * s), a1y.mul_add(g(w, i - 1) + g(w, i + 1), a1x.mul_add(g(w, i - s) + g(w, i + s), a0 * g(w, i))))),
            );
            let dv = by.mul_add(g(v, i - 1) + g(v, i + 1), bx.mul_add(g(v, i - s) + g(v, i + s), (b0 + g(c0, i)) * g(v, i)));
            *out = -(kw + dv);
        }
    }
}

fn resample(line: &[Vec2], ds: f64) -> Vec<Vec2> {
    let mut cum = vec![0.0; line.len()];
    for i in 1..line.len() {
        cum[i] = cum[i - 1] + (line[i].x - line[i - 1].x).hypot(line[i].y - line[i - 1].y);
    }
    let total = cum[line.len() - 1];
    let m = ((total / ds).floor() as usize + 1).max(3);
    let mut out = Vec::with_capacity(m);
    let mut seg = 0;
    for i in 0..m {
        let s = total * i as f64 / (m - 1) as f64;
        while seg + 2 < line.len() && cum[seg + 1] < s {
            seg += 1;
        }
        let len = cum[seg + 1] - cum[seg];
        let t = if len > 0.0 { (s - cum[seg]) / len } else { 0.0 };
        out.push(Vec2 { x: line[seg].x + t * (line[seg + 1].x - line[seg].x), y: line[seg].y + t * (line[seg + 1].y - line[seg].y) });
    }
    out
}

fn inside(outline: &[Vec2], p: Vec2) -> bool {
    let mut inside = false;
    let n = outline.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (outline[i], outline[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

impl Plate {
    /// `dt`: time step. The grid spacing is chosen as fine as the stability bound allows;
    /// `grid_scale` >= 1 coarsens the grid beyond that limit.
    pub fn new(board: &BoardDesign, attachments: &[Attachment], dt: f64, grid_scale: f64) -> Result<Plate, String> {
        let mut p = Plate {
            n: 0,
            nx: 0,
            ny: 0,
            hx: 0.0,
            hy: 0.0,
            node_index: Vec::new(),
            gi: Vec::new(),
            gj: Vec::new(),
            stride: 0,
            grid_size: 0,
            gid: Vec::new(),
            spans: Vec::new(),
            mass: Vec::new(),
            c0: Vec::new(),
            k: Csr::default(),
            k_imp: Csr::default(),
            d: Csr::default(),
            stability_bound: 0.0,
            stability_estimate: 0.0,
            stiff_nodes: 0,
            factor_nnz: 0,
            rib_columns: Vec::new(),
            ex: [board.grain_angle.cos(), board.grain_angle.sin()],
            ey: [-board.grain_angle.sin(), board.grain_angle.cos()],
            gx0: 0.0,
            gy0: 0.0,
            bridge_nodes: Vec::new(),
            a0: 0.0,
            a1x: 0.0,
            a1y: 0.0,
            a2x: 0.0,
            a2y: 0.0,
            a11: 0.0,
            b0: 0.0,
            bx: 0.0,
            by: 0.0,
            c0dt: Vec::new(),
            corr_row: Vec::new(),
            corr_ptr: Vec::new(),
            corr_col: Vec::new(),
            corr_val: Vec::new(),
            rib_runs: Vec::new(),
            rib_coef: Vec::new(),
            longest_rib: 0,
            beams: Vec::new(),
            inv_diag: Vec::new(),
            lanes: 0,
            levels: 0,
            slots: 0,
            core_n: 0,
            f1: Vec::new(),
            f2: Vec::new(),
            fd: Vec::new(),
            g1: Vec::new(),
            g2: Vec::new(),
            gd: Vec::new(),
            chain_slot: Vec::new(),
            chain_gid: Vec::new(),
            virt_slot: Vec::new(),
            virt_core: Vec::new(),
            core_gid: Vec::new(),
            cf_ptr: Vec::new(),
            cf_col: Vec::new(),
            cf_val: Vec::new(),
            c_inv_diag: Vec::new(),
            cb_ptr: Vec::new(),
            cb_row: Vec::new(),
            cb_val: Vec::new(),
        };

        // First guess from the constant coefficient stability condition, then correct the
        // spacing until the rigorous bound of the assembled operator sits just under 4.
        let h = board.thickness;
        let rho_h = board.rho * h;
        let nuyx = board.nuxy * board.ey / board.ex;
        let den = 12.0 * (1.0 - board.nuxy * nuyx);
        let dx = board.ex * h * h * h / den;
        let dy = board.ey * h * h * h / den;
        let d1 = nuyx * dx;
        let dk = board.gxy * h * h * h / 12.0;
        let ratio = (dy / dx).powf(0.25);
        p.hx = (4.0 * dt * dt / rho_h * (2.0 * dx + 2.0 * (d1 + 2.0 * dk) / (ratio * ratio))).powf(0.25) * 1.02;
        p.hy = ratio * p.hx;

        let target = 3.96;
        for _ in 0..12 {
            p.build(board, attachments, dt);
            if p.stability_bound <= target && p.stability_bound > target - 0.08 {
                break;
            }
            let s = (p.stability_bound / (target - 0.03)).powf(0.25);
            p.hx *= s;
            p.hy *= s;
        }
        if grid_scale > 1.0 {
            // coarser than the limit is always stable
            p.hx *= grid_scale;
            p.hy *= grid_scale;
            p.build(board, attachments, dt);
        }
        if p.stability_bound >= 4.0 {
            return Err("plate grid failed the stability bound".into());
        }
        if p.longest_rib + 2 > RIB_SCRATCH {
            return Err("a rib is longer than the kernel allows for".into());
        }
        p.factor(dt)?;
        Ok(p)
    }

    fn to_grid(&self, p: Vec2) -> Vec2 {
        Vec2 { x: p.x * self.ex[0] + p.y * self.ex[1], y: p.x * self.ey[0] + p.y * self.ey[1] }
    }

    /// Piano coordinates of an active node.
    pub fn node_position(&self, node: usize) -> Vec2 {
        let gx = self.gx0 + self.gi[node] as f64 * self.hx;
        let gy = self.gy0 + self.gj[node] as f64 * self.hy;
        Vec2 { x: gx * self.ex[0] + gy * self.ey[0], y: gx * self.ex[1] + gy * self.ey[1] }
    }

    fn id(&self, i: isize, j: isize) -> Option<usize> {
        if i < 0 || i >= self.nx as isize || j < 0 || j >= self.ny as isize {
            None
        } else {
            self.node_index[i as usize + self.nx * j as usize]
        }
    }

    /// Interpolation weights on active node indices.
    fn interp_active(&self, piano: Vec2) -> Interp {
        let g = self.to_grid(piano);
        let fx = (g.x - self.gx0) / self.hx;
        let fy = (g.y - self.gy0) / self.hy;
        let (i, j) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - i, fy - j);
        let (i, j) = (i as isize, j as isize);
        let mut it = Interp::default();
        let di = [0, 1, 0, 1];
        let dj = [0, 0, 1, 1];
        let w = [(1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty];
        for k in 0..4 {
            if let Some(id) = self.id(i + di[k], j + dj[k]) {
                it.idx[k] = id;
                it.w[k] = w[k];
            }
        }
        it
    }

    /// Interpolation weights on grid storage indices.
    pub fn interp(&self, piano: Vec2) -> Interp {
        let mut it = self.interp_active(piano);
        for k in 0..4 {
            it.idx[k] = if it.w[k] != 0.0 { self.gid[it.idx[k]] } else { self.gid[0] };
        }
        it
    }

    pub fn couple(&self, points: &[(Vec2, f64)]) -> Coupling {
        let mut c: BTreeMap<usize, f64> = BTreeMap::new();
        for (pos, coef) in points {
            let it = self.interp_active(*pos);
            for k in 0..4 {
                if it.w[k] != 0.0 {
                    *c.entry(self.gid[it.idx[k]]).or_insert(0.0) += coef * it.w[k];
                }
            }
        }
        let mut out = Coupling::default();
        for (idx, w) in c {
            if out.n < 8 && w != 0.0 {
                out.idx[out.n] = idx;
                out.w[out.n] = w;
                out.n += 1;
            }
        }
        out
    }

    pub fn gather(&self, grid: &[f64], active: &mut [f64]) {
        for a in 0..self.n {
            active[a] = grid[self.gid[a]];
        }
    }

    pub fn correction_nnz(&self) -> usize {
        self.corr_col.len()
    }

    /// Scratch space `solve_stiff` needs, so that it allocates nothing.
    pub fn solve_work_len(&self) -> usize {
        self.slots + self.core_n
    }

    fn build(&mut self, board: &BoardDesign, att: &[Attachment], dt: f64) {
        // Bounding box of the outline in grid coordinates.
        let (mut minx, mut maxx, mut miny, mut maxy) = (1e30f64, -1e30f64, 1e30f64, -1e30f64);
        for p in &board.outline {
            let g = self.to_grid(*p);
            minx = minx.min(g.x);
            maxx = maxx.max(g.x);
            miny = miny.min(g.y);
            maxy = maxy.max(g.y);
        }
        let (hx, hy) = (self.hx, self.hy);
        self.gx0 = minx - hx;
        self.gy0 = miny - hy;
        self.nx = ((maxx - minx) / hx).ceil() as usize + 3;
        self.ny = ((maxy - miny) / hy).ceil() as usize + 3;
        let (nx, ny) = (self.nx, self.ny);

        self.node_index = vec![None; nx * ny];
        self.gi.clear();
        self.gj.clear();
        for j in 0..ny {
            for i in 0..nx {
                let gx = self.gx0 + i as f64 * hx;
                let gy = self.gy0 + j as f64 * hy;
                let p = Vec2 { x: gx * self.ex[0] + gy * self.ey[0], y: gx * self.ex[1] + gy * self.ey[1] };
                if inside(&board.outline, p) {
                    self.node_index[i + nx * j] = Some(self.gi.len());
                    self.gi.push(i);
                    self.gj.push(j);
                }
            }
        }
        let n = self.gi.len();
        self.n = n;
        self.stride = ny + 4;
        self.grid_size = (nx + 4) * self.stride;
        let stride = self.stride;
        self.gid = (0..n).map(|a| (self.gi[a] + 2) * stride + self.gj[a] + 2).collect();

        let h = board.thickness;
        let rho_h = board.rho * h;
        let area = hx * hy;
        let nuyx = board.nuxy * board.ey / board.ex;
        let den = 12.0 * (1.0 - board.nuxy * nuyx);
        let dx = board.ex * h * h * h / den;
        let dy = board.ey * h * h * h / den;
        let d1 = nuyx * dx;
        let dk = board.gxy * h * h * h / 12.0;
        let shell = if board.crown_radius > 0.0 { (board.ex * board.ey).sqrt() * h / (board.crown_radius * board.crown_radius) } else { 0.0 };

        let mut mass = vec![rho_h * area; n];
        // Full stiffness, implicit part, damping, bare plate.
        let (mut tk, mut ti, mut td, mut tp) = (Triplets::default(), Triplets::default(), Triplets::default(), Triplets::default());
        let mut rib_ei = vec![0.0; n];
        self.beams.clear();

        // Ribs: columns of the grid (they run across the grain, which is the grid y direction).
        self.rib_columns.clear();
        let mut is_rib = vec![false; nx];
        if board.ribs {
            let mut x = self.gx0 + hx + 0.5 * board.rib_spacing;
            while x < self.gx0 + (nx as f64 - 2.0) * hx {
                let col = ((x - self.gx0) / hx).round() as usize;
                if col > 0 && col < nx - 1 && !is_rib[col] {
                    is_rib[col] = true;
                    self.rib_columns.push(col);
                }
                x += board.rib_spacing;
            }
        }
        // Distance of each node to the end of its column inside the outline, for the rib taper.
        let mut end_dist = vec![0.0f64; n];
        for i in 0..nx {
            if !is_rib[i] {
                continue;
            }
            let mut run = 0;
            for j in 0..ny {
                let a = self.id(i as isize, j as isize);
                run = if a.is_some() { run + 1 } else { 0 };
                if let Some(a) = a {
                    end_dist[a] = run as f64 * hy;
                }
            }
            run = 0;
            for j in (0..ny).rev() {
                let a = self.id(i as isize, j as isize);
                run = if a.is_some() { run + 1 } else { 0 };
                if let Some(a) = a {
                    end_dist[a] = end_dist[a].min(run as f64 * hy);
                }
            }
        }

        for a in 0..n {
            let (i, j) = (self.gi[a] as isize, self.gj[a] as isize);
            let (mut fx, mut fy) = (Form::default(), Form::default());
            fx.add(self.id(i - 1, j), 1.0 / (hx * hx));
            fx.add(Some(a), -2.0 / (hx * hx));
            fx.add(self.id(i + 1, j), 1.0 / (hx * hx));
            fy.add(self.id(i, j - 1), 1.0 / (hy * hy));
            fy.add(Some(a), -2.0 / (hy * hy));
            fy.add(self.id(i, j + 1), 1.0 / (hy * hy));
            for t in [&mut tk, &mut tp] {
                if shell > 0.0 {
                    let mut own = Form::default();
                    own.add(Some(a), 1.0);
                    t.quad(&own, shell * area);
                }
                t.quad(&fx, dx * area);
                t.quad(&fy, dy * area);
                t.outer(&fx, &fy, d1 * area);
                t.outer(&fy, &fx, d1 * area);
            }
            if is_rib[i as usize] {
                // Ribs are feathered toward the rim. The rib sits under the board, so part of the
                // parallel-axis stiffness is recovered through the (soft, cross-grain) board acting
                // as a flange; phi is the usual composite-section factor.
                let taper = (end_dist[a] / 0.15).clamp(0.35, 1.0);
                let rh = board.rib_height * taper;
                let bw = board.rib_width;
                let ea_rib = board.rib_e * bw * rh;
                let ea_flange = board.ey * h * board.rib_spacing;
                let phi = ea_flange / (ea_flange + ea_rib);
                let e = 0.5 * (rh + h);
                let ei = board.rib_e * (bw * rh * rh * rh / 12.0 + bw * rh * e * e * phi);
                tk.quad(&fy, ei * hy);
                ti.quad(&fy, ei * hy);
                rib_ei[a] = ei;
                mass[a] += board.rib_rho * bw * rh * hy;
            }
        }
        // Clamped rim: the slope is zero at the edge, so the board bends right at the rim. With
        // the mirror condition the curvature at a rim point is twice the value a zero outside
        // would give, over half a cell.
        if board.rim_clamp > 0.0 {
            for j in -1..=ny as isize {
                for i in -1..=nx as isize {
                    if self.id(i, j).is_some() {
                        continue;
                    }
                    let (mut fx, mut fy) = (Form::default(), Form::default());
                    fx.add(self.id(i - 1, j), 1.0 / (hx * hx));
                    fx.add(self.id(i + 1, j), 1.0 / (hx * hx));
                    fy.add(self.id(i, j - 1), 1.0 / (hy * hy));
                    fy.add(self.id(i, j + 1), 1.0 / (hy * hy));
                    for t in [&mut tk, &mut tp] {
                        if fx.len() == 1 {
                            t.quad(&fx, 2.0 * dx * area * board.rim_clamp);
                        }
                        if fy.len() == 1 {
                            t.quad(&fy, 2.0 * dy * area * board.rim_clamp);
                        }
                    }
                }
            }
        }
        // Twist energy lives on cells.
        for j in -1..ny as isize {
            for i in -1..nx as isize {
                let mut f = Form::default();
                let s = 1.0 / (hx * hy);
                f.add(self.id(i, j), s);
                f.add(self.id(i + 1, j), -s);
                f.add(self.id(i, j + 1), -s);
                f.add(self.id(i + 1, j + 1), s);
                if f.len() > 0 {
                    tk.quad(&f, 4.0 * dk * area);
                    tp.quad(&f, 4.0 * dk * area);
                }
            }
        }
        // Damping proportional to slope rate: gives a decay rate proportional to frequency in a
        // plate, i.e. a constant loss factor.
        let s1x = 0.5 * board.loss_factor * (dx / rho_h).sqrt();
        let s1y = 0.5 * board.loss_factor * (dy / rho_h).sqrt();
        for j in -1..ny as isize {
            for i in -1..nx as isize {
                let (mut f, mut g) = (Form::default(), Form::default());
                f.add(self.id(i, j), -1.0 / hx);
                f.add(self.id(i + 1, j), 1.0 / hx);
                g.add(self.id(i, j), -1.0 / hy);
                g.add(self.id(i, j + 1), 1.0 / hy);
                if f.len() > 0 {
                    td.quad(&f, 2.0 * s1x * rho_h * area);
                }
                if g.len() > 0 {
                    td.quad(&g, 2.0 * s1y * rho_h * area);
                }
            }
        }

        // Bridges: Euler-Bernoulli beams sampled along their polylines, tied to the board by
        // interpolation.
        self.bridge_nodes.clear();
        let b_area = board.bridge_width * board.bridge_height;
        let b_ei = board.bridge_e * board.bridge_width * board.bridge_height.powi(3) / 12.0;
        for line in &board.bridges {
            if line.len() < 2 {
                continue;
            }
            let pts = resample(line, hx.max(hy));
            let ds = (pts[1].x - pts[0].x).hypot(pts[1].y - pts[0].y);
            let its: Vec<Interp> = pts.iter().map(|p| self.interp_active(*p)).collect();
            for m in 0..pts.len() {
                let w = if m == 0 || m + 1 == pts.len() { 0.5 } else { 1.0 };
                for k in 0..4 {
                    if its[m].w[k] != 0.0 {
                        mass[its[m].idx[k]] += its[m].w[k] * board.bridge_rho * b_area * ds * w;
                        self.bridge_nodes.push(its[m].idx[k]);
                    }
                }
                if m > 0 && m + 1 < pts.len() {
                    let mut f = Form::default();
                    f.add_interp(&its[m - 1], 1.0 / (ds * ds));
                    f.add_interp(&its[m], -2.0 / (ds * ds));
                    f.add_interp(&its[m + 1], 1.0 / (ds * ds));
                    tk.quad(&f, b_ei * ds);
                    ti.quad(&f, b_ei * ds);
                    let first = self.gid[f.0[0].0];
                    let mut bf = BeamForm { idx: [first; 12], c: [0.0; 12], coef: dt * dt * b_ei * ds };
                    for (q, e) in f.0.iter().enumerate() {
                        bf.idx[q] = self.gid[e.0];
                        bf.c[q] = e.1;
                    }
                    self.beams.push(bf);
                }
            }
        }
        self.bridge_nodes.sort_unstable();
        self.bridge_nodes.dedup();

        let mut dash = vec![0.0; n];
        for a in att {
            // A combination with mixed signs (a slope) cannot be lumped weight by weight. By
            // Cauchy-Schwarz, mass * S * |c_j| on node j with S = sum |c| is never less than the
            // true kinetic energy, which is what the stability argument needs.
            let mut c: BTreeMap<usize, f64> = BTreeMap::new();
            for (pos, coef) in &a.points {
                let it = self.interp_active(*pos);
                for k in 0..4 {
                    if it.w[k] != 0.0 {
                        *c.entry(it.idx[k]).or_insert(0.0) += coef * it.w[k];
                    }
                }
            }
            let s: f64 = c.values().map(|v| v.abs()).sum();
            for (idx, v) in &c {
                mass[*idx] += a.mass * s * v.abs();
                dash[*idx] += a.dashpot * s * v.abs();
            }
        }

        self.c0 = mass.iter().map(|m| 2.0 * board.sigma0 * m).collect();
        self.k = tk.compress(n);
        self.k_imp = ti.compress(n);
        self.d = td.compress(n);

        // ---- run time kernel ----
        self.spans.clear();
        for i in 0..nx {
            let mut j = 0;
            while j < ny {
                if self.id(i as isize, j as isize).is_none() {
                    j += 1;
                    continue;
                }
                let j0 = j;
                while j < ny && self.id(i as isize, j as isize).is_some() {
                    j += 1;
                }
                self.spans.push(Span { start: (i + 2) * stride + j0 + 2, len: j - j0 });
            }
        }
        self.c0dt = vec![0.0; self.grid_size];
        for a in 0..n {
            self.c0dt[self.gid[a]] = dt * self.c0[a];
        }

        // Uniform interior stencil of the bare plate, and its damping twin.
        let dm = 2.0 * d1 + 4.0 * dk;
        let q = dt * dt * area;
        let x4 = 1.0 / (hx * hx * hx * hx);
        let y4 = 1.0 / (hy * hy * hy * hy);
        let xy = 1.0 / (hx * hx * hy * hy);
        self.a0 = q * (6.0 * dx * x4 + 6.0 * dy * y4 + 4.0 * dm * xy + shell);
        self.a1x = q * (-4.0 * dx * x4 - 2.0 * dm * xy);
        self.a1y = q * (-4.0 * dy * y4 - 2.0 * dm * xy);
        self.a2x = q * dx * x4;
        self.a2y = q * dy * y4;
        self.a11 = q * dm * xy;
        self.bx = -dt * 2.0 * s1x * rho_h * area / (hx * hx);
        self.by = -dt * 2.0 * s1y * rho_h * area / (hy * hy);
        self.b0 = -2.0 * (self.bx + self.by);

        // The energy based operator differs from the uniform stencil only next to the rim. Keep
        // the difference as a small sparse correction so that the kernel reproduces K exactly.
        {
            let kp = tp.compress(n);
            let di: [isize; 13] = [0, -1, 1, 0, 0, -2, 2, 0, 0, -1, -1, 1, 1];
            let dj: [isize; 13] = [0, 0, 0, -1, 1, 0, 0, -2, 2, -1, 1, -1, 1];
            let cf = [self.a0, self.a1x, self.a1x, self.a1y, self.a1y, self.a2x, self.a2x, self.a2y, self.a2y, self.a11, self.a11, self.a11, self.a11];
            self.corr_row.clear();
            self.corr_ptr = vec![0];
            self.corr_col.clear();
            self.corr_val.clear();
            let mut acc: BTreeMap<usize, f64> = BTreeMap::new();
            // In storage order, so that a range of spans owns a contiguous range of rows.
            let mut by_storage: Vec<usize> = (0..n).collect();
            by_storage.sort_by_key(|&a| self.gid[a]);
            for a in by_storage {
                acc.clear();
                for p in kp.row_ptr[a]..kp.row_ptr[a + 1] {
                    *acc.entry(kp.col[p]).or_insert(0.0) += dt * dt * kp.val[p];
                }
                for t in 0..13 {
                    if let Some(b) = self.id(self.gi[a] as isize + di[t], self.gj[a] as isize + dj[t]) {
                        *acc.entry(b).or_insert(0.0) -= cf[t];
                    }
                }
                let mut any = false;
                for (b, v) in &acc {
                    if v.abs() > 1e-9 * self.a0 {
                        self.corr_col.push(self.gid[*b]);
                        self.corr_val.push(*v);
                        any = true;
                    }
                }
                if any {
                    self.corr_row.push(self.gid[a]);
                    self.corr_ptr.push(self.corr_col.len());
                }
            }
        }

        // Ribs as contiguous runs along their columns.
        self.rib_runs.clear();
        self.rib_coef.clear();
        self.longest_rib = 0;
        for &i in &self.rib_columns.clone() {
            let mut j = 0;
            while j < ny {
                if self.id(i as isize, j as isize).is_none() {
                    j += 1;
                    continue;
                }
                let mut run = RibRun { start: (i + 2) * stride + j + 2, len: 0, coef: self.rib_coef.len() };
                self.rib_coef.push(0.0); // one zero pad on each side of the run
                while j < ny {
                    let Some(a) = self.id(i as isize, j as isize) else { break };
                    self.rib_coef.push(dt * dt * rib_ei[a] / (hy * hy * hy));
                    run.len += 1;
                    j += 1;
                }
                self.rib_coef.push(0.0);
                self.longest_rib = self.longest_rib.max(run.len);
                self.rib_runs.push(run);
            }
        }

        // Gershgorin bound on M^-1 (dt^2 (K - Kimp) + 2 dt (D + dashpots)).
        let mut bound = 0.0f64;
        {
            let mut acc: BTreeMap<usize, f64> = BTreeMap::new();
            for a in 0..n {
                acc.clear();
                for p in self.k.row_ptr[a]..self.k.row_ptr[a + 1] {
                    *acc.entry(self.k.col[p]).or_insert(0.0) += dt * dt * self.k.val[p];
                }
                for p in self.k_imp.row_ptr[a]..self.k_imp.row_ptr[a + 1] {
                    *acc.entry(self.k_imp.col[p]).or_insert(0.0) -= dt * dt * self.k_imp.val[p];
                }
                for p in self.d.row_ptr[a]..self.d.row_ptr[a + 1] {
                    *acc.entry(self.d.col[p]).or_insert(0.0) += 2.0 * dt * self.d.val[p];
                }
                *acc.entry(a).or_insert(0.0) += 2.0 * dt * dash[a];
                let s: f64 = acc.values().map(|v| v.abs()).sum();
                bound = bound.max(s / mass[a]);
            }
        }
        self.stability_bound = bound;

        // Power iteration on the same operator, for information.
        let mut x: Vec<f64> = (0..n)
            .map(|a| ((a as u32).wrapping_mul(2654435761) % 1000) as f64 / 1000.0 - 0.5 + if (self.gi[a] + self.gj[a]) & 1 == 1 { 1.0 } else { -1.0 })
            .collect();
        let (mut y, mut z) = (vec![0.0; n], vec![0.0; n]);
        let mut lam = 0.0;
        for _ in 0..400 {
            self.k.mul(&x, &mut y);
            self.k_imp.mul(&x, &mut z);
            for a in 0..n {
                y[a] = dt * dt * (y[a] - z[a]);
            }
            self.d.mul(&x, &mut z);
            let (mut num, mut dn) = (0.0, 0.0);
            for a in 0..n {
                y[a] += 2.0 * dt * (z[a] + dash[a] * x[a]);
                num += x[a] * y[a];
                dn += x[a] * x[a] * mass[a];
                y[a] /= mass[a];
            }
            lam = num / dn;
            let nrm: f64 = (0..n).map(|a| y[a] * y[a] * mass[a]).sum::<f64>().sqrt();
            for a in 0..n {
                x[a] = y[a] / nrm;
            }
        }
        self.stability_estimate = lam;
        self.mass = mass;
    }

    /// r = -(dt^2 K w + dt D v + dt C0 v), the plate's internal force times dt^2.
    pub fn apply(&self, w: &[f64], v: &[f64], r: &mut [f64]) {
        assert!(w.len() == self.grid_size && v.len() == self.grid_size && r.len() == self.grid_size);
        unsafe {
            self.apply_stencil(w.as_ptr(), v.as_ptr(), r.as_mut_ptr(), 0, self.spans.len());
            self.apply_local(w.as_ptr(), r.as_mut_ptr(), 0, self.spans.len());
        }
        self.apply_bridges(w, r);
    }

    /// The uniform stencil on a range of spans. Ranges are independent of each other, so
    /// several threads may run different ranges at once.
    ///
    /// # Safety
    /// `w`, `v`, `r` must point to `grid_size` elements; no other thread may write the spans
    /// in the range or read `r` there while this runs.
    pub unsafe fn apply_stencil(&self, w: *const f64, v: *const f64, r: *mut f64, span_from: usize, span_to: usize) {
        let coef = [self.a0, self.a1x, self.a1y, self.a2x, self.a2y, self.a11, self.b0, self.bx, self.by];
        let n = self.grid_size;
        for sp in &self.spans[span_from..span_to] {
            // The output span is handed over as its own exclusive slice. Spans never overlap, and
            // the inputs are only read, so the compiler may vectorise without overlap checks.
            let rs = std::slice::from_raw_parts_mut(r.add(sp.start), sp.len);
            span_kernel(std::slice::from_raw_parts(w, n), std::slice::from_raw_parts(v, n), &self.c0dt, rs, sp.start, self.stride, &coef);
        }
    }

    /// The rim correction and the ribs, for the nodes of a range of spans. Like the stencil,
    /// ranges are independent of each other. Call after `apply_stencil` for the same range.
    ///
    /// # Safety
    /// As for `apply_stencil`.
    pub unsafe fn apply_local(&self, w: *const f64, r: *mut f64, span_from: usize, span_to: usize) {
        if span_from >= span_to {
            return;
        }
        let w = std::slice::from_raw_parts(w, self.grid_size);
        let lo = self.spans[span_from].start;
        let hi = self.spans[span_to - 1].start + self.spans[span_to - 1].len;
        let rows = self.corr_row.partition_point(|&g| g < lo)..self.corr_row.partition_point(|&g| g < hi);
        for i in rows {
            let mut s = 0.0;
            for p in self.corr_ptr[i]..self.corr_ptr[i + 1] {
                s += self.corr_val[p] * w[self.corr_col[p]];
            }
            *r.add(self.corr_row[i]) -= s;
        }
        // Ribs: bending moment along the run, then its second difference.
        let mut m = [0.0f64; RIB_SCRATCH];
        let runs = self.rib_runs.partition_point(|run| run.start < lo)..self.rib_runs.partition_point(|run| run.start < hi);
        for run in &self.rib_runs[runs] {
            let ws = &w[run.start - 1..run.start + run.len + 1];
            let cf = &self.rib_coef[run.coef + 1..run.coef + 1 + run.len];
            m[0] = 0.0;
            for t in 0..run.len {
                m[t + 1] = cf[t] * (ws[t] - 2.0 * ws[t + 1] + ws[t + 2]);
            }
            m[run.len + 1] = 0.0;
            let rs = std::slice::from_raw_parts_mut(r.add(run.start), run.len);
            for t in 0..run.len {
                rs[t] -= m[t] - 2.0 * m[t + 1] + m[t + 2];
            }
        }
    }

    /// The bridges, which tie nodes of different spans together and so run on one thread.
    pub fn apply_bridges(&self, w: &[f64], r: &mut [f64]) {
        for b in &self.beams {
            let mut s = 0.0;
            for q in 0..12 {
                s += b.c[q] * w[b.idx[q]];
            }
            s *= b.coef;
            for q in 0..12 {
                r[b.idx[q]] -= b.c[q] * s;
            }
        }
    }

    /// 1/A_ii off the stiffened block, 1 on it, 0 outside the board.
    pub fn diag_scale(&self) -> &[f64] {
        &self.inv_diag
    }

    fn factor(&mut self, dt: f64) -> Result<(), String> {
        let n = self.n;
        let (nx, ny) = (self.nx, self.ny);
        let stiff: Vec<bool> = (0..n).map(|a| self.k_imp.row_ptr[a + 1] > self.k_imp.row_ptr[a]).collect();
        let mut core = vec![false; n];
        self.stiff_nodes = stiff.iter().filter(|&&s| s).count();

        // Outside the stiffened block A is diagonal; inside it the factor does the work.
        self.inv_diag = vec![0.0; self.grid_size];
        for a in 0..n {
            self.inv_diag[self.gid[a]] = if stiff[a] { 1.0 } else { 1.0 / (self.mass[a] + 0.5 * dt * self.c0[a]) };
        }

        // Core: every node a bridge touches, widened by one node along the ribs so that a rib
        // chain always ends on two consecutive core nodes.
        for &b in &self.bridge_nodes {
            if stiff[b] {
                core[b] = true;
                for dj in [-1isize, 1] {
                    if let Some(c) = self.id(self.gi[b] as isize, self.gj[b] as isize + dj) {
                        if stiff[c] {
                            core[c] = true;
                        }
                    }
                }
            }
        }
        let mut is_rib = vec![false; nx];
        for &i in &self.rib_columns {
            is_rib[i] = true;
        }
        for a in 0..n {
            if stiff[a] && !is_rib[self.gi[a]] {
                core[a] = true;
            }
        }

        struct Chain {
            nodes: Vec<usize>,
            c_a: Option<usize>,
            c_b: Option<usize>,
        }
        let mut chains: Vec<Chain> = Vec::new();
        for &i in &self.rib_columns {
            let ii = i as isize;
            let mut j = 0isize;
            while j < ny as isize {
                let a = self.id(ii, j);
                if a.is_none() || core[a.unwrap()] {
                    j += 1;
                    continue;
                }
                let j0 = j;
                while j < ny as isize && matches!(self.id(ii, j), Some(a) if !core[a]) {
                    j += 1;
                }
                let j1 = j - 1; // segment [j0, j1] of plain rib nodes
                let (lo, hi) = (self.id(ii, j0 - 1), self.id(ii, j1 + 1));
                let core_lo = matches!(lo, Some(a) if core[a]);
                let core_hi = matches!(hi, Some(a) if core[a]);
                if core_lo && core_hi {
                    // held at both ends: leave it to the general factor
                    for q in j0..=j1 {
                        core[self.id(ii, q).unwrap()] = true;
                    }
                    continue;
                }
                let mut c = Chain { nodes: Vec::new(), c_a: None, c_b: None };
                if core_lo {
                    for q in (j0..=j1).rev() {
                        c.nodes.push(self.id(ii, q).unwrap());
                    }
                    c.c_a = lo;
                    c.c_b = self.id(ii, j0 - 2);
                } else {
                    for q in j0..=j1 {
                        c.nodes.push(self.id(ii, q).unwrap());
                    }
                    if core_hi {
                        c.c_a = hi;
                        c.c_b = self.id(ii, j1 + 2);
                    }
                }
                if let Some(b) = c.c_b {
                    if !core[b] {
                        return Err("rib chain ends on a single core node".into());
                    }
                }
                chains.push(c);
            }
        }

        // Elimination order: chains first, then the core, farthest from the bridges first.
        let mut order: Vec<usize> = chains.iter().flat_map(|c| c.nodes.iter().copied()).collect();
        let n_chain = order.len();
        {
            let mut dist: Vec<Option<usize>> = vec![None; n];
            let mut queue = VecDeque::new();
            for &b in &self.bridge_nodes {
                if core[b] {
                    dist[b] = Some(0);
                    queue.push_back(b);
                }
            }
            while let Some(a) = queue.pop_front() {
                for p in self.k_imp.row_ptr[a]..self.k_imp.row_ptr[a + 1] {
                    let c = self.k_imp.col[p];
                    if core[c] && dist[c].is_none() {
                        dist[c] = Some(dist[a].unwrap() + 1);
                        queue.push_back(c);
                    }
                }
            }
            let mut cn: Vec<usize> = (0..n).filter(|&a| core[a]).collect();
            let far = |a: usize| dist[a].unwrap_or(1 << 28);
            cn.sort_by(|&a, &b| far(b).cmp(&far(a)).then(self.gi[a].cmp(&self.gi[b])).then(self.gj[a].cmp(&self.gj[b])));
            order.extend(cn);
        }
        let m = order.len();
        self.core_n = m - n_chain;
        let mut pos: Vec<Option<usize>> = vec![None; n];
        for (i, &a) in order.iter().enumerate() {
            pos[a] = Some(i);
        }

        // Right-looking Cholesky on column maps. Fine for a one-off setup of a few thousand rows.
        let mut colm: Vec<BTreeMap<usize, f64>> = vec![BTreeMap::new(); m];
        let mut diag = vec![0.0; m];
        for i in 0..m {
            let a = order[i];
            diag[i] = self.mass[a] + 0.5 * dt * self.c0[a];
            for p in self.k_imp.row_ptr[a]..self.k_imp.row_ptr[a + 1] {
                let j = pos[self.k_imp.col[p]].unwrap();
                let v = 0.25 * dt * dt * self.k_imp.val[p];
                if j == i {
                    diag[i] += v;
                } else if j > i {
                    *colm[i].entry(j).or_insert(0.0) += v;
                }
            }
        }
        let mut linv = vec![0.0; m];
        self.factor_nnz = 0;
        for j in 0..m {
            if !(diag[j] > 0.0) {
                return Err("stiffener block is not positive definite".into());
            }
            let l = diag[j].sqrt();
            linv[j] = 1.0 / l;
            for v in colm[j].values_mut() {
                *v /= l;
            }
            let col: Vec<(usize, f64)> = colm[j].iter().map(|(&r, &v)| (r, v)).collect();
            for (x, &(ra, va)) in col.iter().enumerate() {
                diag[ra] -= va * va;
                for &(rb, vb) in &col[x + 1..] {
                    *colm[ra].entry(rb).or_insert(0.0) -= va * vb;
                }
            }
            self.factor_nnz += col.len();
        }

        // Lay the chains out as lanes x levels, two zero levels in front and two behind.
        self.lanes = (chains.len() + 7) / 8 * 8; // whole SIMD groups; spare lanes stay empty
        let lanes = self.lanes;
        let longest = chains.iter().map(|c| c.nodes.len()).max().unwrap_or(0);
        self.levels = longest + 4;
        self.slots = lanes * self.levels;
        let slots = self.slots;
        self.f1 = vec![0.0; slots];
        self.f2 = vec![0.0; slots];
        self.fd = vec![0.0; slots];
        self.g1 = vec![0.0; slots];
        self.g2 = vec![0.0; slots];
        self.gd = vec![1.0; slots];
        self.chain_slot.clear();
        self.chain_gid.clear();
        self.virt_slot.clear();
        self.virt_core.clear();
        // Elimination index -> index in the work vector.
        let mut slot_of = vec![0usize; m];
        {
            let mut e = 0;
            for (c, ch) in chains.iter().enumerate() {
                for t in 0..ch.nodes.len() {
                    slot_of[e] = (t + 2) * lanes + c;
                    e += 1;
                }
            }
            for i in n_chain..m {
                slot_of[i] = slots + (i - n_chain);
            }
        }
        {
            let mut e = 0;
            for (c, ch) in chains.iter().enumerate() {
                let t_len = ch.nodes.len();
                let (va, vb) = ((t_len + 2) * lanes + c, (t_len + 3) * lanes + c);
                let (pa, pb) = (ch.c_a.map(|a| pos[a].unwrap()), ch.c_b.map(|b| pos[b].unwrap()));
                if let Some(pa) = pa {
                    self.virt_slot.push(va);
                    self.virt_core.push(slots + pa - n_chain);
                }
                if let Some(pb) = pb {
                    self.virt_slot.push(vb);
                    self.virt_core.push(slots + pb - n_chain);
                }
                for t in 0..t_len {
                    let s = slot_of[e];
                    self.chain_slot.push(s);
                    self.chain_gid.push(self.gid[ch.nodes[t]]);
                    self.fd[s] = linv[e];
                    self.gd[s] = linv[e];
                    for (&row, &v) in &colm[e] {
                        let target = if row < n_chain {
                            slot_of[row]
                        } else if Some(row) == pa {
                            va
                        } else if Some(row) == pb {
                            vb
                        } else {
                            return Err("unexpected fill in a rib chain".into());
                        };
                        if target == s + lanes {
                            self.g1[s] = v;
                        } else if target == s + 2 * lanes {
                            self.g2[s] = v;
                        } else {
                            return Err("rib chain is not pentadiagonal".into());
                        }
                        if row < n_chain {
                            // the same entry seen from its row, for the forward sweep
                            if target == s + lanes {
                                self.f1[target] = v;
                            } else {
                                self.f2[target] = v;
                            }
                        }
                    }
                    e += 1;
                }
            }
        }
        // Core rows (forward) and core columns (backward).
        self.core_gid = (0..self.core_n).map(|i| self.gid[order[n_chain + i]]).collect();
        self.c_inv_diag = (0..self.core_n).map(|i| linv[n_chain + i]).collect();
        let mut rows: Vec<Vec<(usize, f64)>> = vec![Vec::new(); self.core_n];
        self.cb_ptr = vec![0; self.core_n + 1];
        self.cb_row.clear();
        self.cb_val.clear();
        for j in 0..m {
            for (&row, &v) in &colm[j] {
                if row >= n_chain {
                    rows[row - n_chain].push((slot_of[j], v));
                }
            }
            if j >= n_chain {
                // Farthest row first: the unknown computed just before this one then enters the
                // sum last, which keeps the dependency chain of the substitution short.
                for (&row, &v) in colm[j].iter().rev() {
                    self.cb_row.push(slots + row - n_chain);
                    self.cb_val.push(v);
                }
                self.cb_ptr[j - n_chain + 1] = self.cb_row.len();
            }
        }
        self.cf_ptr = vec![0; self.core_n + 1];
        self.cf_col.clear();
        self.cf_val.clear();
        for i in 0..self.core_n {
            for &(c, v) in &rows[i] {
                self.cf_col.push(c);
                self.cf_val.push(v);
            }
            self.cf_ptr[i + 1] = self.cf_col.len();
        }
        Ok(())
    }

    /// A^-1 x with A = M + dt^2/4 Kimp + dt/2 C0, on grid storage, in two parts so that the
    /// caller can fold the diagonal part into its own loop: this overwrites the entries of the
    /// stiffened block with the solution, and every entry must then be multiplied by
    /// `diag_scale()` (which is 1 on the stiffened block). `work` needs `solve_work_len()`.
    pub fn solve_stiff(&self, x: &mut [f64], work: &mut [f64]) {
        let w = self.lanes;
        let slots = self.slots;
        let (nc, cn) = (self.chain_slot.len(), self.core_n);
        let y = &mut work[..slots + cn];
        for i in 0..nc {
            y[self.chain_slot[i]] = x[self.chain_gid[i]];
        }
        for i in 0..cn {
            y[slots + i] = x[self.core_gid[i]];
        }
        // Forward: all rib chains in step, then the core.
        for lv in 2..self.levels {
            let (before, cur) = y.split_at_mut(lv * w);
            let p1 = &before[(lv - 1) * w..lv * w];
            let p2 = &before[(lv - 2) * w..(lv - 1) * w];
            let cur = &mut cur[..w];
            let (a1, a2, dd) = (&self.f1[lv * w..(lv + 1) * w], &self.f2[lv * w..(lv + 1) * w], &self.fd[lv * w..(lv + 1) * w]);
            for c in 0..w {
                cur[c] = (cur[c] - a1[c] * p1[c] - a2[c] * p2[c]) * dd[c];
            }
        }
        for i in 0..cn {
            let mut s = y[slots + i];
            for p in self.cf_ptr[i]..self.cf_ptr[i + 1] {
                s -= self.cf_val[p] * y[self.cf_col[p]];
            }
            y[slots + i] = s * self.c_inv_diag[i];
        }
        // Backward: core, then the chains outward from the bridges.
        for j in (0..cn).rev() {
            let mut s = y[slots + j];
            for p in self.cb_ptr[j]..self.cb_ptr[j + 1] {
                s -= self.cb_val[p] * y[self.cb_row[p]];
            }
            y[slots + j] = s * self.c_inv_diag[j];
        }
        for i in 0..self.virt_slot.len() {
            y[self.virt_slot[i]] = y[self.virt_core[i]];
        }
        if self.levels >= 5 {
            for lv in (2..=self.levels - 3).rev() {
                let (head, tail) = y.split_at_mut((lv + 1) * w);
                let cur = &mut head[lv * w..];
                let n1 = &tail[..w];
                let n2 = &tail[w..2 * w];
                let (a1, a2, dd) = (&self.g1[lv * w..(lv + 1) * w], &self.g2[lv * w..(lv + 1) * w], &self.gd[lv * w..(lv + 1) * w]);
                for c in 0..w {
                    cur[c] = (cur[c] - a1[c] * n1[c] - a2[c] * n2[c]) * dd[c];
                }
            }
        }
        for i in 0..nc {
            x[self.chain_gid[i]] = y[self.chain_slot[i]];
        }
        for i in 0..cn {
            x[self.core_gid[i]] = y[slots + i];
        }
    }
}
