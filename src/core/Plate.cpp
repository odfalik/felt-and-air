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

Plate::Plate(const BoardDesign& board, const std::vector<Attachment>& att, double dt) {
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
                for (int q = 0; q < f.n; q++) { bf.idx[q] = gid[f.idx[q]]; bf.c[q] = f.c[q]; }
                for (int q = f.n; q < 12; q++) { bf.idx[q] = gid[f.idx[0]]; bf.c[q] = 0; }
                bf.coef = dt * dt * bEI * ds;
                beams_.push_back(bf);
            }
        }
    }
    std::sort(bridgeNodes_.begin(), bridgeNodes_.end());
    bridgeNodes_.erase(std::unique(bridgeNodes_.begin(), bridgeNodes_.end()), bridgeNodes_.end());

    std::vector<double> dash(n, 0.0);
    for (const Attachment& a : att) {
        Interp it = interpActive(a.pos);
        for (int k = 0; k < 4; k++) {
            mass[it.idx[k]] += it.w[k] * a.mass;
            dash[it.idx[k]] += it.w[k] * a.dashpot;
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
    c0dt_.assign(gridSize, 0.0);
    for (int a = 0; a < n; a++) c0dt_[gid[a]] = dt * c0[a];

    // Uniform interior stencil of the bare plate, and its damping twin.
    const double Dm = 2 * D1 + 4 * Dk, q = dt * dt * area;
    const double x4 = 1 / (hx * hx * hx * hx), y4 = 1 / (hy * hy * hy * hy), xy = 1 / (hx * hx * hy * hy);
    a0_ = q * (6 * Dx * x4 + 6 * Dy * y4 + 4 * Dm * xy);
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
                    corrVal_.push_back(kv.second);
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
            ribCoef_.push_back(0.0);  // one zero pad on each side of the run
            while (j < Ny && id(i, j) >= 0) {
                ribCoef_.push_back(dt * dt * ribEI[id(i, j)] / (hy * hy * hy));
                run.len++; j++;
            }
            ribCoef_.push_back(0.0);
            ribRuns_.push_back(run);
            longest = std::max(longest, run.len);
        }
    ribScr_.assign(longest + 2, 0.0);

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

void Plate::apply(const double* __restrict w, const double* __restrict v, double* __restrict r) const {
    const int S = stride;
    const double a0 = a0_, a1x = a1x_, a1y = a1y_, a2x = a2x_, a2y = a2y_, a11 = a11_;
    const double b0 = b0_, bx = bx_, by = by_;
    for (const Span& sp : spans) {
        const double* __restrict ws = w + sp.start;
        const double* __restrict vs = v + sp.start;
        const double* __restrict cs = c0dt_.data() + sp.start;
        double* __restrict rs = r + sp.start;
        for (int t = 0; t < sp.len; t++) {
            double kw = a0 * ws[t] + a1x * (ws[t - S] + ws[t + S]) + a1y * (ws[t - 1] + ws[t + 1]) +
                        a2x * (ws[t - 2 * S] + ws[t + 2 * S]) + a2y * (ws[t - 2] + ws[t + 2]) +
                        a11 * (ws[t - S - 1] + ws[t - S + 1] + ws[t + S - 1] + ws[t + S + 1]);
            double dv = (b0 + cs[t]) * vs[t] + bx * (vs[t - S] + vs[t + S]) + by * (vs[t - 1] + vs[t + 1]);
            rs[t] = -(kw + dv);
        }
    }
    const int rows = (int)corrRow_.size();
    for (int i = 0; i < rows; i++) {
        double s = 0;
        for (int p = corrPtr_[i]; p < corrPtr_[i + 1]; p++) s += corrVal_[p] * w[corrCol_[p]];
        r[corrRow_[i]] -= s;
    }
    // Ribs: bending moment along the run, then its second difference.
    double* __restrict m = ribScr_.data();
    for (const RibRun& run : ribRuns_) {
        const double* __restrict ws = w + run.start;
        const double* __restrict cf = ribCoef_.data() + run.coef + 1;
        double* __restrict rs = r + run.start;
        m[0] = 0;
        for (int t = 0; t < run.len; t++) m[t + 1] = cf[t] * (ws[t - 1] - 2 * ws[t] + ws[t + 1]);
        m[run.len + 1] = 0;
        for (int t = 0; t < run.len; t++) rs[t] -= m[t] - 2 * m[t + 1] + m[t + 2];
    }
    for (const BeamForm& b : beams_) {
        double s = 0;
        for (int q = 0; q < 12; q++) s += b.c[q] * w[b.idx[q]];
        s *= b.coef;
        for (int q = 0; q < 12; q++) r[b.idx[q]] -= b.c[q] * s;
    }
}

void Plate::factor(double dt) {
    // Nodes touched by an implicit stiffener form a sparse SPD block; everything else is diagonal.
    std::vector<int> pos(n, -1);
    std::vector<int> stiff;
    for (int a = 0; a < n; a++)
        if (Kimp.rowPtr[a + 1] > Kimp.rowPtr[a]) stiff.push_back(a);
    stiffNodes = (int)stiff.size();

    // Elimination order: farthest from the bridges first. Rib chains are then eliminated from
    // their free ends inward with no fill, leaving the narrow band along the bridges for last.
    std::vector<int> dist(n, -1);
    std::queue<int> q;
    for (int b : bridgeNodes_)
        if (Kimp.rowPtr[b + 1] > Kimp.rowPtr[b]) { dist[b] = 0; q.push(b); }
    while (!q.empty()) {
        int a = q.front(); q.pop();
        for (int p = Kimp.rowPtr[a]; p < Kimp.rowPtr[a + 1]; p++) {
            int c = Kimp.col[p];
            if (dist[c] < 0) { dist[c] = dist[a] + 1; q.push(c); }
        }
    }
    std::sort(stiff.begin(), stiff.end(), [&](int a, int b) {
        int da = dist[a] < 0 ? 1 << 28 : dist[a], db = dist[b] < 0 ? 1 << 28 : dist[b];
        if (da != db) return da > db;
        if (gi[a] != gi[b]) return gi[a] < gi[b];
        return gj[a] < gj[b];
    });
    perm_.resize(stiffNodes);
    for (int i = 0; i < stiffNodes; i++) { pos[stiff[i]] = i; perm_[i] = gid[stiff[i]]; }

    invDiag_.assign(gridSize, 0.0);
    for (int a = 0; a < n; a++) invDiag_[gid[a]] = 1.0 / (mass[a] + 0.5 * dt * c0[a]);

    // Right-looking Cholesky on column maps. Fine for a one-off setup of a few thousand rows.
    std::vector<std::map<int, double>> colm(stiffNodes);
    std::vector<double> diag(stiffNodes);
    for (int i = 0; i < stiffNodes; i++) {
        int a = stiff[i];
        diag[i] = mass[a] + 0.5 * dt * c0[a];
        for (int p = Kimp.rowPtr[a]; p < Kimp.rowPtr[a + 1]; p++) {
            int j = pos[Kimp.col[p]];
            double v = 0.25 * dt * dt * Kimp.val[p];
            if (j == i) diag[i] += v;
            else if (j > i) colm[i][j] += v;
        }
    }
    lPtr_.assign(stiffNodes + 1, 0);
    lRow_.clear(); lVal_.clear();
    lInvDiag_.assign(stiffNodes, 0.0);
    for (int j = 0; j < stiffNodes; j++) {
        if (!(diag[j] > 0)) throw std::runtime_error("stiffener block is not positive definite");
        double l = std::sqrt(diag[j]);
        lInvDiag_[j] = 1.0 / l;
        for (auto& kv : colm[j]) kv.second /= l;
        for (auto a = colm[j].begin(); a != colm[j].end(); ++a) {
            diag[a->first] -= a->second * a->second;
            auto b = a;
            for (++b; b != colm[j].end(); ++b) colm[a->first][b->first] -= a->second * b->second;
        }
        for (auto& kv : colm[j]) { lRow_.push_back(kv.first); lVal_.push_back(kv.second); }
        lPtr_[j + 1] = (int)lRow_.size();
        colm[j].clear();
    }
    factorNnz = (int)lRow_.size();
    work_.assign(stiffNodes, 0.0);
    // Row copy of L. Both substitutions then only read earlier results, which lets the
    // processor run the independent rib chains side by side.
    rPtr_.assign(stiffNodes + 1, 0);
    for (int r : lRow_) rPtr_[r + 1]++;
    for (int i = 0; i < stiffNodes; i++) rPtr_[i + 1] += rPtr_[i];
    rCol_.resize(factorNnz); rVal_.resize(factorNnz);
    std::vector<int> fill(rPtr_.begin(), rPtr_.end() - 1);
    for (int j = 0; j < stiffNodes; j++)
        for (int p = lPtr_[j]; p < lPtr_[j + 1]; p++) {
            int q = fill[lRow_[p]]++;
            rCol_[q] = j; rVal_[q] = lVal_[p];
        }
}

void Plate::solve(double* __restrict x) const {
    double* __restrict y = work_.data();
    const int m = stiffNodes;
    for (int i = 0; i < m; i++) y[i] = x[perm_[i]];
    const double* __restrict id = invDiag_.data();
    for (const Span& sp : spans) {
        double* __restrict xs = x + sp.start;
        const double* __restrict ds = id + sp.start;
        for (int t = 0; t < sp.len; t++) xs[t] *= ds[t];
    }
    const int* __restrict lp = lPtr_.data();
    const int* __restrict lr = lRow_.data();
    const double* __restrict lv = lVal_.data();
    const int* __restrict rp = rPtr_.data();
    const int* __restrict rc = rCol_.data();
    const double* __restrict rv = rVal_.data();
    for (int i = 0; i < m; i++) {
        double s = y[i];
        for (int p = rp[i]; p < rp[i + 1]; p++) s -= rv[p] * y[rc[p]];
        y[i] = s * lInvDiag_[i];
    }
    for (int j = m - 1; j >= 0; j--) {
        double s = y[j];
        for (int p = lp[j]; p < lp[j + 1]; p++) s -= lv[p] * y[lr[p]];
        y[j] = s * lInvDiag_[j];
    }
    for (int i = 0; i < m; i++) x[perm_[i]] = y[i];
}

}  // namespace pp
