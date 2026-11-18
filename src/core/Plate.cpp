#include "Plate.h"
#include <algorithm>
#include <cmath>
#include <cstdio>
#include <map>
#include <queue>
#include <stdexcept>

namespace pp {

void Csr::mul(const double* __restrict x, double* __restrict y) const {
    const int* __restrict rp = rowPtr.data();
    const int* __restrict c = col.data();
    const double* __restrict v = val.data();
    for (int i = 0; i < n; i++) {
        double s = 0;
        for (int p = rp[i]; p < rp[i + 1]; p++) s += v[p] * x[c[p]];
        y[i] = s;
    }
}

double Csr::quad(const double* x) const {
    double s = 0;
    for (int i = 0; i < n; i++) {
        double r = 0;
        for (int p = rowPtr[i]; p < rowPtr[i + 1]; p++) r += val[p] * x[col[p]];
        s += r * x[i];
    }
    return s;
}

namespace {

// A linear functional of the nodal values: sum c_k w[idx_k].
struct Form {
    int n = 0;
    int idx[16];
    double c[16];
    void add(int id, double v) {
        if (id < 0 || v == 0) return;
        for (int k = 0; k < n; k++)
            if (idx[k] == id) { c[k] += v; return; }
        idx[n] = id; c[n] = v; n++;
    }
    void add(const Interp& it, double v) {
        for (int k = 0; k < 4; k++)
            if (it.w[k] != 0) add(it.idx[k], v * it.w[k]);
    }
};

struct Triplets {
    struct T { int r, c; double v; };
    std::vector<T> t;
    // Adds coef * (f g' ) to the matrix.
    void outer(const Form& f, const Form& g, double coef) {
        for (int a = 0; a < f.n; a++)
            for (int b = 0; b < g.n; b++) t.push_back({f.idx[a], g.idx[b], coef * f.c[a] * g.c[b]});
    }
    void quad(const Form& f, double coef) { outer(f, f, coef); }
    Csr compress(int n) {
        std::sort(t.begin(), t.end(), [](const T& a, const T& b) { return a.r != b.r ? a.r < b.r : a.c < b.c; });
        Csr m;
        m.n = n;
        m.rowPtr.assign(n + 1, 0);
        for (size_t i = 0; i < t.size();) {
            size_t j = i;
            double s = 0;
            while (j < t.size() && t[j].r == t[i].r && t[j].c == t[i].c) s += t[j++].v;
            m.col.push_back(t[i].c);
            m.val.push_back(s);
            m.rowPtr[t[i].r + 1]++;
            i = j;
        }
        for (int i = 0; i < n; i++) m.rowPtr[i + 1] += m.rowPtr[i];
        return m;
    }
};

std::vector<Vec2> resample(const std::vector<Vec2>& line, double ds) {
    std::vector<double> cum(line.size(), 0.0);
    for (size_t i = 1; i < line.size(); i++)
        cum[i] = cum[i - 1] + std::hypot(line[i].x - line[i - 1].x, line[i].y - line[i - 1].y);
    double total = cum.back();
    int m = std::max(3, (int)std::floor(total / ds) + 1);
    std::vector<Vec2> out;
    size_t seg = 0;
    for (int i = 0; i < m; i++) {
        double s = total * i / (m - 1);
        while (seg + 2 < line.size() && cum[seg + 1] < s) seg++;
        double len = cum[seg + 1] - cum[seg];
        double t = len > 0 ? (s - cum[seg]) / len : 0;
        out.push_back({line[seg].x + t * (line[seg + 1].x - line[seg].x),
                       line[seg].y + t * (line[seg + 1].y - line[seg].y)});
    }
    return out;
}

}  // namespace

Vec2 Plate::toGrid(Vec2 p) const { return {p.x * ex_[0] + p.y * ex_[1], p.x * ey_[0] + p.y * ey_[1]}; }

Vec2 Plate::nodePosition(int node) const {
    double gx = gx0_ + gi[node] * hx, gy = gy0_ + gj[node] * hy;
    return {gx * ex_[0] + gy * ey_[0], gx * ex_[1] + gy * ey_[1]};
}

bool Plate::inside(const BoardDesign& board, Vec2 p) const {
    bool in = false;
    const auto& o = board.outline;
    for (size_t i = 0, j = o.size() - 1; i < o.size(); j = i++) {
        if ((o[i].y > p.y) != (o[j].y > p.y) &&
            p.x < (o[j].x - o[i].x) * (p.y - o[i].y) / (o[j].y - o[i].y) + o[i].x)
            in = !in;
    }
    return in;
}

Interp Plate::interp(Vec2 piano) const {
    Interp it = interpActive(piano);
    for (int k = 0; k < 4; k++) it.idx[k] = it.w[k] != 0 ? gid[it.idx[k]] : gid[0];
    return it;
}

Coupling Plate::couple(const std::vector<std::pair<Vec2, double>>& points) const {
    std::map<int, double> c;
    for (const auto& pt : points) {
        Interp it = interpActive(pt.first);
        for (int k = 0; k < 4; k++)
            if (it.w[k] != 0) c[gid[it.idx[k]]] += pt.second * it.w[k];
    }
    Coupling out;
    for (auto& kv : c)
        if (out.n < 8 && kv.second != 0) { out.idx[out.n] = kv.first; out.w[out.n] = kv.second; out.n++; }
    return out;
}

Interp Plate::interpActive(Vec2 piano) const {
    Vec2 g = toGrid(piano);
    double fx = (g.x - gx0_) / hx, fy = (g.y - gy0_) / hy;
    int i = (int)std::floor(fx), j = (int)std::floor(fy);
    double tx = fx - i, ty = fy - j;
    Interp it;
    const int di[4] = {0, 1, 0, 1}, dj[4] = {0, 0, 1, 1};
    const double w[4] = {(1 - tx) * (1 - ty), tx * (1 - ty), (1 - tx) * ty, tx * ty};
    for (int k = 0; k < 4; k++) {
        int ii = i + di[k], jj = j + dj[k];
        int id = (ii >= 0 && ii < Nx && jj >= 0 && jj < Ny) ? nodeIndex[ii + Nx * jj] : -1;
        if (id >= 0) { it.idx[k] = id; it.w[k] = w[k]; }
    }
    return it;
}

Plate::Plate(const BoardDesign& board, const std::vector<Attachment>& att, double dt, double gridScale) {
    dt_ = dt;
    ex_[0] = std::cos(board.grainAngle); ex_[1] = std::sin(board.grainAngle);
    ey_[0] = -ex_[1]; ey_[1] = ex_[0];

    // First guess from the constant coefficient stability condition, then correct the
    // spacing until the rigorous bound of the assembled operator sits just under 4.
    const double H = board.thickness, rhoH = board.rho * H;
    const double nuyx = board.nuxy * board.Ey / board.Ex;
    const double den = 12.0 * (1.0 - board.nuxy * nuyx);
    const double Dx = board.Ex * H * H * H / den, Dy = board.Ey * H * H * H / den;
    const double D1 = nuyx * Dx, Dk = board.Gxy * H * H * H / 12.0;
    const double ratio = std::pow(Dy / Dx, 0.25);
    hx = std::pow(4 * dt * dt / rhoH * (2 * Dx + 2 * (D1 + 2 * Dk) / (ratio * ratio)), 0.25) * 1.02;
    hy = ratio * hx;

    const double target = 3.96;
    for (int it = 0; it < 12; it++) {
        build(board, att, dt);
        if (stabilityBound <= target && stabilityBound > target - 0.08) break;
        double s = std::pow(stabilityBound / (target - 0.03), 0.25);
        hx *= s; hy *= s;
    }
    if (gridScale > 1.0) {  // coarser than the limit is always stable
        hx *= gridScale; hy *= gridScale;
        build(board, att, dt);
    }
    if (stabilityBound >= 4.0) throw std::runtime_error("plate grid failed the stability bound");
    factor(dt);
}

void Plate::build(const BoardDesign& board, const std::vector<Attachment>& att, double dt) {
    // Bounding box of the outline in grid coordinates.
    double minx = 1e30, maxx = -1e30, miny = 1e30, maxy = -1e30;
    for (Vec2 p : board.outline) {
        Vec2 g = toGrid(p);
        minx = std::min(minx, g.x); maxx = std::max(maxx, g.x);
        miny = std::min(miny, g.y); maxy = std::max(maxy, g.y);
    }
    gx0_ = minx - hx; gy0_ = miny - hy;
    Nx = (int)std::ceil((maxx - minx) / hx) + 3;
    Ny = (int)std::ceil((maxy - miny) / hy) + 3;

    nodeIndex.assign((size_t)Nx * Ny, -1);
    gi.clear(); gj.clear();
    for (int j = 0; j < Ny; j++)
        for (int i = 0; i < Nx; i++) {
            double gx = gx0_ + i * hx, gy = gy0_ + j * hy;
            Vec2 p{gx * ex_[0] + gy * ey_[0], gx * ex_[1] + gy * ey_[1]};
            if (inside(board, p)) {
                nodeIndex[i + Nx * j] = (int)gi.size();
                gi.push_back(i); gj.push_back(j);
            }
        }
    n = (int)gi.size();
    stride = Ny + 4;
    gridSize = (Nx + 4) * stride;
    gid.resize(n);
    for (int a = 0; a < n; a++) gid[a] = (gi[a] + 2) * stride + gj[a] + 2;
    auto id = [&](int i, int j) { return (i < 0 || i >= Nx || j < 0 || j >= Ny) ? -1 : nodeIndex[i + Nx * j]; };

    const double H = board.thickness, rhoH = board.rho * H, area = hx * hy;
    const double nuyx = board.nuxy * board.Ey / board.Ex;
    const double den = 12.0 * (1.0 - board.nuxy * nuyx);
    const double Dx = board.Ex * H * H * H / den, Dy = board.Ey * H * H * H / den;
    const double D1 = nuyx * Dx, Dk = board.Gxy * H * H * H / 12.0;

    const double shell = board.crownRadius > 0 ? std::sqrt(board.Ex * board.Ey) * H / (board.crownRadius * board.crownRadius) : 0.0;
    mass.assign(n, rhoH * area);
    Triplets tk, ti, td, tp;  // full stiffness, implicit part, damping, bare plate
    std::vector<double> ribEI(n, 0.0);
    beams_.clear();

    // Ribs: columns of the grid (they run across the grain, which is the grid y direction).
    ribColumns.clear();
    std::vector<char> isRib(Nx, 0);
    if (board.ribs) {
        for (double x = gx0_ + hx + 0.5 * board.ribSpacing; x < gx0_ + (Nx - 2) * hx; x += board.ribSpacing) {
            int col = (int)std::lround((x - gx0_) / hx);
            if (col > 0 && col < Nx - 1 && !isRib[col]) { isRib[col] = 1; ribColumns.push_back(col); }
        }
    }
    // Distance of each node to the end of its column inside the outline, for the rib taper.
    std::vector<double> endDist(n, 0.0);
    for (int i = 0; i < Nx; i++) {
        if (!isRib[i]) continue;
        int run = 0;
        for (int j = 0; j < Ny; j++) {
            int a = id(i, j);
            run = a >= 0 ? run + 1 : 0;
            if (a >= 0) endDist[a] = run * hy;
        }
        run = 0;
        for (int j = Ny - 1; j >= 0; j--) {
            int a = id(i, j);
            run = a >= 0 ? run + 1 : 0;
            if (a >= 0) endDist[a] = std::min(endDist[a], run * hy);
        }
    }

    for (int a = 0; a < n; a++) {
        int i = gi[a], j = gj[a];
        Form fx, fy;
        fx.add(id(i - 1, j), 1 / (hx * hx)); fx.add(a, -2 / (hx * hx)); fx.add(id(i + 1, j), 1 / (hx * hx));
        fy.add(id(i, j - 1), 1 / (hy * hy)); fy.add(a, -2 / (hy * hy)); fy.add(id(i, j + 1), 1 / (hy * hy));
        for (Triplets* t : {&tk, &tp}) {
            if (shell > 0) { Form self; self.add(a, 1.0); t->quad(self, shell * area); }
            t->quad(fx, Dx * area);
            t->quad(fy, Dy * area);
            t->outer(fx, fy, D1 * area);
            t->outer(fy, fx, D1 * area);
        }
        if (isRib[i]) {
            // Ribs are feathered toward the rim. The rib sits under the board, so part of the
            // parallel-axis stiffness is recovered through the (soft, cross-grain) board acting
            // as a flange; phi is the usual composite-section factor.
            double taper = std::clamp(endDist[a] / 0.15, 0.35, 1.0);
            double h = board.ribHeight * taper, bw = board.ribWidth;
            double eaRib = board.ribE * bw * h, eaFlange = board.Ey * H * board.ribSpacing;
            double phi = eaFlange / (eaFlange + eaRib);
            double e = 0.5 * (h + H);
            double EI = board.ribE * (bw * h * h * h / 12.0 + bw * h * e * e * phi);
            tk.quad(fy, EI * hy);
            ti.quad(fy, EI * hy);
            ribEI[a] = EI;
            mass[a] += board.ribRho * bw * h * hy;
        }
    }
    // Clamped rim: the slope is zero at the edge, so the board bends right at the rim. With
    // the mirror condition the curvature at a rim point is twice the value a zero outside
    // would give, over half a cell.
    if (board.rimClamp > 0)
        for (int j = -1; j <= Ny; j++)
            for (int i = -1; i <= Nx; i++) {
                if (id(i, j) >= 0) continue;
                Form fx, fy;
                fx.add(id(i - 1, j), 1 / (hx * hx)); fx.add(id(i + 1, j), 1 / (hx * hx));
                fy.add(id(i, j - 1), 1 / (hy * hy)); fy.add(id(i, j + 1), 1 / (hy * hy));
                for (Triplets* t : {&tk, &tp}) {
                    if (fx.n == 1) t->quad(fx, 2 * Dx * area * board.rimClamp);
                    if (fy.n == 1) t->quad(fy, 2 * Dy * area * board.rimClamp);
                }
            }
    // Twist energy lives on cells.
    for (int j = -1; j < Ny; j++)
        for (int i = -1; i < Nx; i++) {
            Form f;
            double s = 1 / (hx * hy);
            f.add(id(i, j), s); f.add(id(i + 1, j), -s); f.add(id(i, j + 1), -s); f.add(id(i + 1, j + 1), s);
            if (f.n) { tk.quad(f, 4 * Dk * area); tp.quad(f, 4 * Dk * area); }
        }
    // Damping proportional to slope rate: gives a decay rate proportional to frequency in a
    // plate, i.e. a constant loss factor.
    const double s1x = 0.5 * board.lossFactor * std::sqrt(Dx / rhoH);
    const double s1y = 0.5 * board.lossFactor * std::sqrt(Dy / rhoH);
    for (int j = -1; j < Ny; j++)
        for (int i = -1; i < Nx; i++) {
            Form f, g;
            f.add(id(i, j), -1 / hx); f.add(id(i + 1, j), 1 / hx);
            g.add(id(i, j), -1 / hy); g.add(id(i, j + 1), 1 / hy);
            if (f.n) td.quad(f, 2 * s1x * rhoH * area);
            if (g.n) td.quad(g, 2 * s1y * rhoH * area);
        }

    // Bridges: Euler-Bernoulli beams sampled along their polylines, tied to the board by
    // interpolation.
    bridgeNodes_.clear();
    const double bA = board.bridgeWidth * board.bridgeHeight;
    const double bEI = board.bridgeE * board.bridgeWidth * std::pow(board.bridgeHeight, 3) / 12.0;
    for (const auto& line : board.bridges) {
        if (line.size() < 2) continue;
        auto pts = resample(line, std::max(hx, hy));
        double ds = std::hypot(pts[1].x - pts[0].x, pts[1].y - pts[0].y);
        std::vector<Interp> its;
        for (Vec2 p : pts) its.push_back(interpActive(p));
        for (size_t m = 0; m < pts.size(); m++) {
            double w = (m == 0 || m + 1 == pts.size()) ? 0.5 : 1.0;
            for (int k = 0; k < 4; k++)
                if (its[m].w[k] != 0) {
                    mass[its[m].idx[k]] += its[m].w[k] * board.bridgeRho * bA * ds * w;
                    bridgeNodes_.push_back(its[m].idx[k]);
                }
            if (m > 0 && m + 1 < pts.size()) {
                Form f;
                f.add(its[m - 1], 1 / (ds * ds)); f.add(its[m], -2 / (ds * ds)); f.add(its[m + 1], 1 / (ds * ds));
                tk.quad(f, bEI * ds);
                ti.quad(f, bEI * ds);
                BeamForm bf{};
                for (int q = 0; q < f.n; q++) { bf.idx[q] = gid[f.idx[q]]; bf.c[q] = (RealP)f.c[q]; }
                for (int q = f.n; q < 12; q++) { bf.idx[q] = gid[f.idx[0]]; bf.c[q] = 0; }
                bf.coef = (RealP)(dt * dt * bEI * ds);
                beams_.push_back(bf);
            }
        }
    }
    std::sort(bridgeNodes_.begin(), bridgeNodes_.end());
    bridgeNodes_.erase(std::unique(bridgeNodes_.begin(), bridgeNodes_.end()), bridgeNodes_.end());

    std::vector<double> dash(n, 0.0);
    for (const Attachment& a : att) {
        // A combination with mixed signs (a slope) cannot be lumped weight by weight. By
        // Cauchy-Schwarz, mass * S * |c_j| on node j with S = sum |c| is never less than the
        // true kinetic energy, which is what the stability argument needs.
        std::map<int, double> c;
        for (const auto& pt : a.points) {
            Interp it = interpActive(pt.first);
            for (int k = 0; k < 4; k++)
                if (it.w[k] != 0) c[it.idx[k]] += pt.second * it.w[k];
        }
        double S = 0;
        for (auto& kv : c) S += std::fabs(kv.second);
        for (auto& kv : c) {
            mass[kv.first] += a.mass * S * std::fabs(kv.second);
            dash[kv.first] += a.dashpot * S * std::fabs(kv.second);
        }
    }

    c0.resize(n);
    for (int a = 0; a < n; a++) c0[a] = 2 * board.sigma0 * mass[a];
    K = tk.compress(n);
    Kimp = ti.compress(n);
    D = td.compress(n);

    // ---- run time kernel ----
    spans.clear();
    for (int i = 0; i < Nx; i++)
        for (int j = 0; j < Ny; j++) {
            if (id(i, j) < 0) continue;
            int j0 = j;
            while (j < Ny && id(i, j) >= 0) j++;
            spans.push_back({(i + 2) * stride + j0 + 2, j - j0});
        }
    c0dt_.assign(gridSize, 0);
    for (int a = 0; a < n; a++) c0dt_[gid[a]] = (RealP)(dt * c0[a]);

    // Uniform interior stencil of the bare plate, and its damping twin.
    const double Dm = 2 * D1 + 4 * Dk, q = dt * dt * area;
    const double x4 = 1 / (hx * hx * hx * hx), y4 = 1 / (hy * hy * hy * hy), xy = 1 / (hx * hx * hy * hy);
    a0_ = q * (6 * Dx * x4 + 6 * Dy * y4 + 4 * Dm * xy + shell);
    a1x_ = q * (-4 * Dx * x4 - 2 * Dm * xy);
    a1y_ = q * (-4 * Dy * y4 - 2 * Dm * xy);
    a2x_ = q * Dx * x4;
    a2y_ = q * Dy * y4;
    a11_ = q * Dm * xy;
    bx_ = -dt * 2 * s1x * rhoH * area / (hx * hx);
    by_ = -dt * 2 * s1y * rhoH * area / (hy * hy);
    b0_ = -2 * (bx_ + by_);

    // The energy based operator differs from the uniform stencil only next to the rim. Keep
    // the difference as a small sparse correction so that the kernel reproduces K exactly.
    {
        Csr Kp = tp.compress(n);
        const int di[13] = {0, -1, 1, 0, 0, -2, 2, 0, 0, -1, -1, 1, 1};
        const int dj[13] = {0, 0, 0, -1, 1, 0, 0, -2, 2, -1, 1, -1, 1};
        const double cf[13] = {a0_, a1x_, a1x_, a1y_, a1y_, a2x_, a2x_, a2y_, a2y_, a11_, a11_, a11_, a11_};
        corrRow_.clear(); corrPtr_.assign(1, 0); corrCol_.clear(); corrVal_.clear();
        std::map<int, double> acc;
        for (int a = 0; a < n; a++) {
            acc.clear();
            for (int p = Kp.rowPtr[a]; p < Kp.rowPtr[a + 1]; p++) acc[Kp.col[p]] += dt * dt * Kp.val[p];
            for (int t = 0; t < 13; t++) {
                int b = id(gi[a] + di[t], gj[a] + dj[t]);
                if (b >= 0) acc[b] -= cf[t];
            }
            bool any = false;
            for (auto& kv : acc)
                if (std::fabs(kv.second) > 1e-9 * a0_) {
                    corrCol_.push_back(gid[kv.first]);
                    corrVal_.push_back((RealP)kv.second);
                    any = true;
                }
            if (any) { corrRow_.push_back(gid[a]); corrPtr_.push_back((int)corrCol_.size()); }
        }
    }

    // Ribs as contiguous runs along their columns.
    ribRuns_.clear(); ribCoef_.clear();
    int longest = 0;
    for (int i : ribColumns)
        for (int j = 0; j < Ny; j++) {
            if (id(i, j) < 0) continue;
            RibRun run{(i + 2) * stride + j + 2, 0, (int)ribCoef_.size()};
            ribCoef_.push_back(0);  // one zero pad on each side of the run
            while (j < Ny && id(i, j) >= 0) {
                ribCoef_.push_back((RealP)(dt * dt * ribEI[id(i, j)] / (hy * hy * hy)));
                run.len++; j++;
            }
            ribCoef_.push_back(0);
            ribRuns_.push_back(run);
            longest = std::max(longest, run.len);
        }
    ribScr_.assign(longest + 2, 0);

    // Gershgorin bound on M^-1 (dt^2 (K - Kimp) + 2 dt (D + dashpots)).
    std::vector<double> row(n, 0.0);
    {
        std::map<int, double> acc;
        for (int a = 0; a < n; a++) {
            acc.clear();
            for (int p = K.rowPtr[a]; p < K.rowPtr[a + 1]; p++) acc[K.col[p]] += dt * dt * K.val[p];
            for (int p = Kimp.rowPtr[a]; p < Kimp.rowPtr[a + 1]; p++) acc[Kimp.col[p]] -= dt * dt * Kimp.val[p];
            for (int p = D.rowPtr[a]; p < D.rowPtr[a + 1]; p++) acc[D.col[p]] += 2 * dt * D.val[p];
            acc[a] += 2 * dt * dash[a];
            double s = 0;
            for (auto& kv : acc) s += std::fabs(kv.second);
            row[a] = s / mass[a];
        }
    }
    stabilityBound = *std::max_element(row.begin(), row.end());

    // Power iteration on the same operator, for information.
    std::vector<double> x(n), y(n), z(n);
    for (int a = 0; a < n; a++) x[a] = ((a * 2654435761u) % 1000) / 1000.0 - 0.5 + ((gi[a] + gj[a]) & 1 ? 1 : -1);
    double lam = 0;
    for (int it = 0; it < 400; it++) {
        K.mul(x.data(), y.data());
        Kimp.mul(x.data(), z.data());
        for (int a = 0; a < n; a++) y[a] = dt * dt * (y[a] - z[a]);
        D.mul(x.data(), z.data());
        double num = 0, dn = 0;
        for (int a = 0; a < n; a++) {
            y[a] += 2 * dt * (z[a] + dash[a] * x[a]);
            num += x[a] * y[a];
            dn += x[a] * x[a] * mass[a];
            y[a] /= mass[a];
        }
        lam = num / dn;
        double nrm = 0;
        for (int a = 0; a < n; a++) nrm += y[a] * y[a] * mass[a];
        nrm = std::sqrt(nrm);
        for (int a = 0; a < n; a++) x[a] = y[a] / nrm;
    }
    stabilityEstimate = lam;
}

void Plate::apply(const RealP* w, const RealP* v, RealP* r) const {
    applyStencil(w, v, r, 0, (int)spans.size());
    applyExtras(w, r);
}

void Plate::applyStencil(const RealP* __restrict w, const RealP* __restrict v, RealP* __restrict r, int spanFrom, int spanTo) const {
    const int S = stride;
    const RealP a0 = (RealP)a0_, a1x = (RealP)a1x_, a1y = (RealP)a1y_, a2x = (RealP)a2x_, a2y = (RealP)a2y_, a11 = (RealP)a11_;
    const RealP b0 = (RealP)b0_, bx = (RealP)bx_, by = (RealP)by_;
    for (int si = spanFrom; si < spanTo; si++) {
        const Span& sp = spans[si];
        const RealP* __restrict ws = w + sp.start;
        const RealP* __restrict vs = v + sp.start;
        const RealP* __restrict cs = c0dt_.data() + sp.start;
        RealP* __restrict rs = r + sp.start;
        for (int t = 0; t < sp.len; t++) {
            RealP kw = a0 * ws[t] + a1x * (ws[t - S] + ws[t + S]) + a1y * (ws[t - 1] + ws[t + 1]) +
                        a2x * (ws[t - 2 * S] + ws[t + 2 * S]) + a2y * (ws[t - 2] + ws[t + 2]) +
                        a11 * (ws[t - S - 1] + ws[t - S + 1] + ws[t + S - 1] + ws[t + S + 1]);
            RealP dv = (b0 + cs[t]) * vs[t] + bx * (vs[t - S] + vs[t + S]) + by * (vs[t - 1] + vs[t + 1]);
            rs[t] = -(kw + dv);
        }
    }
}

void Plate::applyExtras(const RealP* __restrict w, RealP* __restrict r) const {
    const int rows = (int)corrRow_.size();
    for (int i = 0; i < rows; i++) {
        RealP s = 0;
        for (int p = corrPtr_[i]; p < corrPtr_[i + 1]; p++) s += corrVal_[p] * w[corrCol_[p]];
        r[corrRow_[i]] -= s;
    }
    // Ribs: bending moment along the run, then its second difference.
    RealP* __restrict m = ribScr_.data();
    for (const RibRun& run : ribRuns_) {
        const RealP* __restrict ws = w + run.start;
        const RealP* __restrict cf = ribCoef_.data() + run.coef + 1;
        RealP* __restrict rs = r + run.start;
        m[0] = 0;
        for (int t = 0; t < run.len; t++) m[t + 1] = cf[t] * (ws[t - 1] - 2 * ws[t] + ws[t + 1]);
        m[run.len + 1] = 0;
        for (int t = 0; t < run.len; t++) rs[t] -= m[t] - 2 * m[t + 1] + m[t + 2];
    }
    for (const BeamForm& b : beams_) {
        RealP s = 0;
        for (int q = 0; q < 12; q++) s += b.c[q] * w[b.idx[q]];
        s *= b.coef;
        for (int q = 0; q < 12; q++) r[b.idx[q]] -= b.c[q] * s;
    }
}

void Plate::factor(double dt) {
    auto id = [&](int i, int j) { return (i < 0 || i >= Nx || j < 0 || j >= Ny) ? -1 : nodeIndex[i + Nx * j]; };
    std::vector<char> stiff(n, 0), core(n, 0);
    for (int a = 0; a < n; a++) stiff[a] = Kimp.rowPtr[a + 1] > Kimp.rowPtr[a];
    stiffNodes = 0;
    for (int a = 0; a < n; a++) stiffNodes += stiff[a];

    // Outside the stiffened block A is diagonal; inside it the factor does the work.
    invDiag_.assign(gridSize, 0);
    for (int a = 0; a < n; a++) invDiag_[gid[a]] = (RealP)(stiff[a] ? 1.0 : 1.0 / (mass[a] + 0.5 * dt * c0[a]));

    // Core: every node a bridge touches, widened by one node along the ribs so that a rib
    // chain always ends on two consecutive core nodes.
    for (int b : bridgeNodes_)
        if (stiff[b]) {
            core[b] = 1;
            for (int dj : {-1, 1}) {
                int c = id(gi[b], gj[b] + dj);
                if (c >= 0 && stiff[c]) core[c] = 1;
            }
        }
    std::vector<char> isRib(Nx, 0);
    for (int i : ribColumns) isRib[i] = 1;
    for (int a = 0; a < n; a++)
        if (stiff[a] && !isRib[gi[a]]) core[a] = 1;

    struct Chain { std::vector<int> nodes; int cA = -1, cB = -1; };
    std::vector<Chain> chains;
    for (int i : ribColumns)
        for (int j = 0; j < Ny; j++) {
            int a = id(i, j);
            if (a < 0 || core[a]) continue;
            int j0 = j;
            while (j < Ny && id(i, j) >= 0 && !core[id(i, j)]) j++;
            int j1 = j - 1;  // segment [j0, j1] of plain rib nodes
            int lo = id(i, j0 - 1), hi = id(i, j1 + 1);
            bool coreLo = lo >= 0 && core[lo], coreHi = hi >= 0 && core[hi];
            if (coreLo && coreHi) {  // held at both ends: leave it to the general factor
                for (int q = j0; q <= j1; q++) core[id(i, q)] = 1;
                continue;
            }
            Chain c;
            if (coreLo) {
                for (int q = j1; q >= j0; q--) c.nodes.push_back(id(i, q));
                c.cA = lo; c.cB = id(i, j0 - 2);
            } else {
                for (int q = j0; q <= j1; q++) c.nodes.push_back(id(i, q));
                if (coreHi) { c.cA = hi; c.cB = id(i, j1 + 2); }
            }
            if (c.cB >= 0 && !core[c.cB]) throw std::runtime_error("rib chain ends on a single core node");
            chains.push_back(c);
        }

    // Elimination order: chains first, then the core, farthest from the bridges first.
    std::vector<int> order, pos(n, -1);
    for (const Chain& c : chains)
        for (int a : c.nodes) order.push_back(a);
    const int nChain = (int)order.size();
    {
        std::vector<int> dist(n, -1), cn;
        std::queue<int> q;
        for (int b : bridgeNodes_)
            if (core[b]) { dist[b] = 0; q.push(b); }
        while (!q.empty()) {
            int a = q.front(); q.pop();
            for (int p = Kimp.rowPtr[a]; p < Kimp.rowPtr[a + 1]; p++) {
                int c = Kimp.col[p];
                if (core[c] && dist[c] < 0) { dist[c] = dist[a] + 1; q.push(c); }
            }
        }
        for (int a = 0; a < n; a++)
            if (core[a]) cn.push_back(a);
        std::sort(cn.begin(), cn.end(), [&](int a, int b) {
            int da = dist[a] < 0 ? 1 << 28 : dist[a], db = dist[b] < 0 ? 1 << 28 : dist[b];
            if (da != db) return da > db;
            if (gi[a] != gi[b]) return gi[a] < gi[b];
            return gj[a] < gj[b];
        });
        for (int a : cn) order.push_back(a);
    }
    const int m = (int)order.size();
    coreN_ = m - nChain;
    for (int i = 0; i < m; i++) pos[order[i]] = i;

    // Right-looking Cholesky on column maps. Fine for a one-off setup of a few thousand rows.
    std::vector<std::map<int, double>> colm(m);
    std::vector<double> diag(m);
    for (int i = 0; i < m; i++) {
        int a = order[i];
        diag[i] = mass[a] + 0.5 * dt * c0[a];
        for (int p = Kimp.rowPtr[a]; p < Kimp.rowPtr[a + 1]; p++) {
            int j = pos[Kimp.col[p]];
            double v = 0.25 * dt * dt * Kimp.val[p];
            if (j == i) diag[i] += v;
            else if (j > i) colm[i][j] += v;
        }
    }
    std::vector<double> linv(m);
    factorNnz = 0;
    for (int j = 0; j < m; j++) {
        if (!(diag[j] > 0)) throw std::runtime_error("stiffener block is not positive definite");
        double l = std::sqrt(diag[j]);
        linv[j] = 1.0 / l;
        for (auto& kv : colm[j]) kv.second /= l;
        for (auto a = colm[j].begin(); a != colm[j].end(); ++a) {
            diag[a->first] -= a->second * a->second;
            auto b = a;
            for (++b; b != colm[j].end(); ++b) colm[a->first][b->first] -= a->second * b->second;
        }
        factorNnz += (int)colm[j].size();
    }

    // Lay the chains out as lanes x levels, two zero levels in front and two behind.
    lanes_ = ((int)chains.size() + 7) / 8 * 8;  // whole SIMD groups; spare lanes stay empty
    int longest = 0;
    for (const Chain& c : chains) longest = std::max(longest, (int)c.nodes.size());
    levels_ = longest + 4;
    slots_ = lanes_ * levels_;
    f1_.assign(slots_, 0); f2_.assign(slots_, 0); fd_.assign(slots_, 0);
    g1_.assign(slots_, 0); g2_.assign(slots_, 0); gd_.assign(slots_, 1);
    chainSlot_.clear(); chainGid_.clear(); virtSlot_.clear(); virtCore_.clear();
    std::vector<int> slotOf(m, -1);  // elimination index -> index in the work vector
    {
        int e = 0;
        for (int c = 0; c < (int)chains.size(); c++)
            for (size_t t = 0; t < chains[c].nodes.size(); t++, e++) slotOf[e] = (int)(t + 2) * lanes_ + c;
        for (int i = nChain; i < m; i++) slotOf[i] = slots_ + (i - nChain);
    }
    {
        int e = 0;
        for (int c = 0; c < (int)chains.size(); c++) {
            const Chain& ch = chains[c];
            const int T = (int)ch.nodes.size();
            const int vA = (T + 2) * lanes_ + c, vB = (T + 3) * lanes_ + c;
            if (ch.cA >= 0) { virtSlot_.push_back(vA); virtCore_.push_back(slots_ + pos[ch.cA] - nChain); }
            if (ch.cB >= 0) { virtSlot_.push_back(vB); virtCore_.push_back(slots_ + pos[ch.cB] - nChain); }
            for (int t = 0; t < T; t++, e++) {
                int s = slotOf[e];
                chainSlot_.push_back(s);
                chainGid_.push_back(gid[ch.nodes[t]]);
                fd_[s] = gd_[s] = (RealP)linv[e];
                for (auto& kv : colm[e]) {
                    int row = kv.first, target;
                    if (row < nChain) target = slotOf[row];
                    else if (ch.cA >= 0 && row == pos[ch.cA]) target = vA;
                    else if (ch.cB >= 0 && row == pos[ch.cB]) target = vB;
                    else throw std::runtime_error("unexpected fill in a rib chain");
                    if (target == s + lanes_) g1_[s] = (RealP)kv.second;
                    else if (target == s + 2 * lanes_) g2_[s] = (RealP)kv.second;
                    else throw std::runtime_error("rib chain is not pentadiagonal");
                    if (row < nChain) {  // the same entry seen from its row, for the forward sweep
                        if (target == s + lanes_) f1_[target] = (RealP)kv.second;
                        else f2_[target] = (RealP)kv.second;
                    }
                }
            }
        }
    }
    // Core rows (forward) and core columns (backward).
    coreGid_.resize(coreN_);
    cInvDiag_.resize(coreN_);
    for (int i = 0; i < coreN_; i++) { coreGid_[i] = gid[order[nChain + i]]; cInvDiag_[i] = (RealP)linv[nChain + i]; }
    std::vector<std::vector<std::pair<int, double>>> rows(coreN_);
    cbPtr_.assign(coreN_ + 1, 0); cbRow_.clear(); cbVal_.clear();
    for (int j = 0; j < m; j++) {
        for (auto& kv : colm[j]) {
            if (kv.first >= nChain) rows[kv.first - nChain].push_back({slotOf[j], kv.second});
        }
        if (j >= nChain) {
            // Farthest row first: the unknown computed just before this one then enters the
            // sum last, which keeps the dependency chain of the substitution short.
            for (auto it = colm[j].rbegin(); it != colm[j].rend(); ++it) {
                cbRow_.push_back(slots_ + it->first - nChain);
                cbVal_.push_back((RealP)it->second);
            }
            cbPtr_[j - nChain + 1] = (int)cbRow_.size();
        }
    }
    cfPtr_.assign(coreN_ + 1, 0); cfCol_.clear(); cfVal_.clear();
    for (int i = 0; i < coreN_; i++) {
        for (auto& e : rows[i]) { cfCol_.push_back(e.first); cfVal_.push_back((RealP)e.second); }
        cfPtr_[i + 1] = (int)cfCol_.size();
    }
    work_.assign(slots_ + coreN_, 0);
}

void Plate::solveStiff(RealP* __restrict x) const {
    RealP* __restrict y = work_.data();
    const int W = lanes_, nc = (int)chainSlot_.size(), cn = coreN_;
    RealP* __restrict yc = y + slots_;
    for (int i = 0; i < nc; i++) y[chainSlot_[i]] = x[chainGid_[i]];
    for (int i = 0; i < cn; i++) yc[i] = x[coreGid_[i]];
    // Forward: all rib chains in step, then the core.
    for (int lv = 2; lv < levels_; lv++) {
        RealP* __restrict cur = y + lv * W;
        const RealP* __restrict p1 = y + (lv - 1) * W;
        const RealP* __restrict p2 = y + (lv - 2) * W;
        const RealP* __restrict a1 = f1_.data() + lv * W;
        const RealP* __restrict a2 = f2_.data() + lv * W;
        const RealP* __restrict dd = fd_.data() + lv * W;
        for (int c = 0; c < W; c++) cur[c] = (cur[c] - a1[c] * p1[c] - a2[c] * p2[c]) * dd[c];
    }
    for (int i = 0; i < cn; i++) {
        RealP s = yc[i];
        for (int p = cfPtr_[i]; p < cfPtr_[i + 1]; p++) s -= cfVal_[p] * y[cfCol_[p]];
        yc[i] = s * cInvDiag_[i];
    }
    // Backward: core, then the chains outward from the bridges.
    for (int j = cn - 1; j >= 0; j--) {
        RealP s = yc[j];
        for (int p = cbPtr_[j]; p < cbPtr_[j + 1]; p++) s -= cbVal_[p] * y[cbRow_[p]];
        yc[j] = s * cInvDiag_[j];
    }
    const int nv = (int)virtSlot_.size();
    for (int i = 0; i < nv; i++) y[virtSlot_[i]] = y[virtCore_[i]];
    for (int lv = levels_ - 3; lv >= 2; lv--) {
        RealP* __restrict cur = y + lv * W;
        const RealP* __restrict n1 = y + (lv + 1) * W;
        const RealP* __restrict n2 = y + (lv + 2) * W;
        const RealP* __restrict a1 = g1_.data() + lv * W;
        const RealP* __restrict a2 = g2_.data() + lv * W;
        const RealP* __restrict dd = gd_.data() + lv * W;
        for (int c = 0; c < W; c++) cur[c] = (cur[c] - a1[c] * n1[c] - a2[c] * n2[c]) * dd[c];
    }
    for (int i = 0; i < nc; i++) x[chainGid_[i]] = y[chainSlot_[i]];
    for (int i = 0; i < cn; i++) x[coreGid_[i]] = yc[i];
}

}  // namespace pp
