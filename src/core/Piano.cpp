#include "Piano.h"
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <stdexcept>

namespace pp {

namespace {
inline double pos(double x) { return x > 0 ? x : 0; }

// Decaying vibration ends in subnormal numbers, which are very slow to compute with.
// Flush them to zero while rendering, and restore the caller's floating point mode after.
struct FlushDenormals {
#if defined(__aarch64__)
    uint64_t saved;
    FlushDenormals() { __asm__ volatile("mrs %0, fpcr" : "=r"(saved)); uint64_t v = saved | (1ull << 24); __asm__ volatile("msr fpcr, %0" : : "r"(v)); }
    ~FlushDenormals() { __asm__ volatile("msr fpcr, %0" : : "r"(saved)); }
#endif
};
}

Piano::Piano(const PianoDesign& d, const PianoOptions& o) {
    os_ = std::max(1, o.oversample);
    fs_ = o.sampleRate * os_;
    k_ = 1.0 / fs_;
    outGain_ = o.outputGain;
    const double k = k_;
    std::fill(noteOfMidi_, noteOfMidi_ + 128, -1);

    std::vector<Attachment> att;
    std::vector<double> endScale;
    int offset = 0, maxN = 0;
    for (const NoteDesign& nd : d.notes) {
        NoteRT n;
        n.midi = nd.midi;
        n.first = (int)strings_.size();
        n.count = (int)nd.strings.size();
        if (n.count > 3) throw std::runtime_error("at most three strings per note");
        n.hMass = nd.hammerMass; n.hK = nd.hammerK; n.hP = nd.hammerP; n.hMu = nd.hammerMu;
        n.hasDamper = nd.hasDamper;
        for (const StringDesign& sd : nd.strings) {
            StringRT s;
            s.note = (int)notes_.size();
            s.attached = sd.attached;
            s.hasDamper = nd.hasDamper;
            // Finest grid the stability condition allows. The two coefficients of the discrete
            // string (wave speed and stiffness) are then fitted so that its fundamental is exactly
            // f0 and a mid-band partial sits where the continuous stiff string puts it. This is
            // what a tuner does for the fundamental, extended to cancel the leading part of the
            // grid dispersion. The physical stiffness enters through the inharmonicity B.
            const double B = M_PI * M_PI * sd.EI / (sd.T * sd.L * sd.L);
            auto target = [&](int q) { return q * sd.f0 * std::sqrt((1 + B * q * q) / (1 + B)); };
            double c2 = sd.T / sd.rhoA, kap2 = sd.EI / sd.rhoA, a = c2 * k * k + 4 * sd.sigma1 * k;
            double hmin = std::sqrt(0.5 * (a + std::sqrt(a * a + 16 * kap2 * k * k)));
            int N = (int)std::floor(sd.L / hmin) + 2, fitQ = 1;
            double h = 0;
            for (; N >= 3; N--) {
                h = sd.L / N;
                auto sn = [&](int q) { return std::pow(std::sin(q * M_PI / (2 * N)), 2); };
                auto lhs = [&](int q) { return 4 / (k * k) * std::pow(std::sin(M_PI * target(q) * k), 2); };
                auto stable = [&](double cc, double kk) {
                    return cc > 0 && kk >= 0 &&
                           h * h * h * h >= ((cc * k * k + 4 * sd.sigma1 * k) * h * h + 4 * kk * k * k) * (1 + 1e-9);
                };
                // Fit: lhs(q) = c2 * (4/h^2) s_q + kap2 * (16/h^4) s_q^2 for q = 1 and q = fit.
                // Among the stable fits, keep the one with the smallest worst-case error over
                // the partials below 8 kHz.
                const int qMax = std::min(N - 1, 400);
                double best = 1e30;
                bool found = false;
                for (int q = 2; q <= std::max(2, qMax); q++) {
                    if (target(q) > 0.45 * fs_ && q > 2) break;
                    double a1 = 4 / (h * h) * sn(1), b1 = 16 / (h * h * h * h) * sn(1) * sn(1);
                    double a2 = 4 / (h * h) * sn(q), b2 = 16 / (h * h * h * h) * sn(q) * sn(q);
                    double det = a1 * b2 - a2 * b1;
                    double cc = (lhs(1) * b2 - lhs(q) * b1) / det, kk = (a1 * lhs(q) - a2 * lhs(1)) / det;
                    if (!stable(cc, kk)) continue;
                    double worst = 0;
                    for (int m = 2; m <= qMax && target(m) < 8000.0; m++) {
                        double x = cc * k * k / (h * h) * sn(m) + 4 * kk * k * k / (h * h * h * h) * sn(m) * sn(m);
                        double f = x >= 1 ? 1e-3 : std::asin(std::sqrt(x)) / (M_PI * k);
                        worst = std::max(worst, std::fabs(std::log2(f / target(m))));
                    }
                    if (worst < best) { best = worst; c2 = cc; kap2 = kk; fitQ = q; found = true; }
                }
                if (found) break;
            }
            if (N < 3) throw std::runtime_error("string cannot be resolved at this sample rate");
            s.fitPartial = fitQ; s.f0 = sd.f0; s.inharm = B;
            s.N = N; s.h = h;
            s.aT = c2 * k * k / (h * h);
            s.aB = kap2 * k * k / (h * h * h * h);
            s.aD = 2 * sd.sigma1 * k / (h * h);
            s.s0k = sd.sigma0 * k;
            s.mNode = sd.rhoA * h;
            s.tension = c2 * sd.rhoA;
            s.z0 = std::sqrt(s.tension * sd.rhoA);
            s.T_h = s.tension / h;
            s.EI_h3 = kap2 * sd.rhoA / (h * h * h);
            s.damp_h = 2 * sd.sigma1 * sd.rhoA / h;

            double hp = nd.strikeRatio * sd.L / h;
            s.hamL = std::min((int)std::floor(hp), N - 2);
            double fr = hp - s.hamL;
            s.hamG[0] = s.hamL == 0 ? 0.0 : 1 - fr;  // node 0 is the fixed agraffe
            s.hamG[1] = fr;
            s.hamA = k * k * (s.hamG[0] * s.hamG[0] + s.hamG[1] * s.hamG[1]) / (s.mNode * (1 + s.s0k));

            double dl = std::clamp(0.12 * sd.L, 0.03, 0.14);
            s.dmp0 = std::max(s.hamL + 3, (int)std::ceil(0.16 * sd.L / h));
            s.dmp1 = std::min(N, s.dmp0 + std::max(2, (int)std::lround(dl / h)));
            if (s.dmp0 >= s.dmp1) s.hasDamper = false;
            s.dampRate = s.dampTarget = s.hasDamper ? dampMax_ : 0.0;

            s.bridge = sd.bridge;
            s.off = offset + 1;
            offset += N + 3;
            maxN = std::max(maxN, N);
            strings_.push_back(s);
        }
        // Second polarisation: the same string moving parallel to the soundboard. The bridge
        // yields along one tilted axis, so the upright motion sees cos(tilt) of it and the
        // sideways motion sin(tilt). The interpolation weights carry that factor (set below).
        n.twin = nd.twoPolarisations;
        n.total = n.count * (n.twin ? 2 : 1);
        for (int i = 0; i < n.count; i++) strings_[n.first + i].hamW = n.twin ? std::cos(nd.hammerTilt) : 1.0;
        if (n.twin)
            for (int i = 0; i < n.count; i++) {
                StringRT t = strings_[n.first + i];
                t.hamW = std::sin(nd.hammerTilt);
                t.off = offset + 1;
                offset += t.N + 3;
                strings_.push_back(t);
            }
        for (int i = 0; i < n.total; i++) {
            const StringDesign& sd = nd.strings[i % n.count];
            const StringRT& s = strings_[n.first + i];
            double es = !n.twin ? 1.0 : i < n.count ? std::cos(sd.bridgeTilt) : std::sin(sd.bridgeTilt);
            if (s.attached) att.push_back({sd.bridge, 0.5 * s.mNode * es * es, s.z0 * es * es});
            endScale.push_back(es);
        }
        noteOfMidi_[nd.midi & 127] = (int)notes_.size();
        notes_.push_back(n);
    }
    u_.assign(offset, 0);
    v_[0].assign(offset, 0);
    v_[1].assign(offset, 0);
    (void)maxN;

    plate_ = std::make_unique<Plate>(d.board, att, k, std::max(1.0, o.boardGridScale));
    const Plate& P = *plate_;
    {
        size_t ai = 0;
        for (size_t i = 0; i < strings_.size(); i++) {
            StringRT& s = strings_[i];
            if (s.attached) {
                s.at = P.interp(att[ai++].pos);
                for (double& wq : s.at.w) wq *= endScale[i];
            }
            // Strings without a damper (top treble, detached test strings) never sleep.
            s.awake = !s.hasDamper || !s.attached;
        }
    }
    w_.assign(P.gridSize, 0); vw_.assign(P.gridSize, 0); r_.assign(P.gridSize, 0);

    // Rayleigh integral over a baffled board: p = rho/(2 pi) * sum dS * accel(t - r/c) / r.
    // Delays are rounded to the nearest step (a path error under 4 mm at 48 kHz, well below
    // the grid spacing).
    int ringLen = 1;
    for (int e = 0; e < 2; e++) {
        std::vector<int> delay(P.n);
        std::vector<double> gain(P.n);
        int lo = 1 << 30, hi = 0;
        for (int a = 0; a < P.n; a++) {
            Vec2 p = P.nodePosition(a);
            double r = std::sqrt(std::pow(p.x - d.ears[e].x, 2) + std::pow(p.y - d.ears[e].y, 2) +
                                 d.ears[e].z * d.ears[e].z);
            delay[a] = (int)std::lround(r / d.airC * fs_);
            gain[a] = d.airRho * P.hx * P.hy / (2 * M_PI * r) / (k * k);  // rhs is dt^2 * accel
            lo = std::min(lo, delay[a]); hi = std::max(hi, delay[a]);
        }
        binFirst_[e] = lo;
        binPtr_[e].assign(hi - lo + 2, 0);
        for (int a = 0; a < P.n; a++) binPtr_[e][delay[a] - lo + 1]++;
        for (int b = 0; b < hi - lo + 1; b++) binPtr_[e][b + 1] += binPtr_[e][b];
        std::vector<int> fill(binPtr_[e].begin(), binPtr_[e].end() - 1);
        tapIdx_[e].resize(P.n); tapGain_[e].resize(P.n);
        for (int a = 0; a < P.n; a++) {
            int q = fill[delay[a] - lo]++;
            tapIdx_[e][q] = P.gid[a];
            tapGain_[e][q] = (RealP)gain[a];
        }
        while (ringLen < hi + 4) ringLen <<= 1;
    }
    ringMask_ = ringLen - 1;
    ring_[0].assign(ringLen, 0.0);
    ring_[1].assign(ringLen, 0.0);

    if (os_ > 1) {  // Kaiser windowed sinc, cutoff at 0.45 of the output rate
        int half = 24 * os_, len = 2 * half + 1;
        fir_.resize(len);
        double fc = 0.45 / os_, beta = 9.0, sum = 0;
        auto bessel = [](double x) {
            double s = 1, t = 1;
            for (int i = 1; i < 40; i++) { t *= (x / (2 * i)) * (x / (2 * i)); s += t; }
            return s;
        };
        for (int i = 0; i < len; i++) {
            double m = i - half;
            double sinc = m == 0 ? 2 * fc : std::sin(2 * M_PI * fc * m) / (M_PI * m);
            double wn = bessel(beta * std::sqrt(1 - (m / half) * (m / half))) / bessel(beta);
            fir_[i] = sinc * wn;
            sum += fir_[i];
        }
        for (double& x : fir_) x /= sum;
        hist_[0].assign(len, 0.0);
        hist_[1].assign(len, 0.0);
    }
}

void Piano::wake(StringRT& s) {
    if (s.awake) return;
    // Start from the static shape for the current bridge position, so waking is silent.
    double uN = uEnd(s, w_.data()), vN = uEnd(s, vw_.data());
    RealS* u = &u_[s.off];
    RealS* v = &v_[vb_][s.off];
    for (int l = 0; l <= s.N; l++) {
        u[l] = (RealS)(uN * l / s.N);
        v[l] = (RealS)(vN * l / s.N);
    }
    s.awake = true;
}

void Piano::sleep(StringRT& s) {
    for (auto* a : {&u_, &v_[0], &v_[1]}) std::fill(a->begin() + s.off - 1, a->begin() + s.off + s.N + 2, (RealS)0);
    s.awake = false;
}

void Piano::noteOn(int midi, double speed) {
    int ni = noteOfMidi_[midi & 127];
    if (ni < 0) return;
    NoteRT& n = notes_[ni];
    n.keyDown = true;
    for (int i = 0; i < n.total; i++) {
        StringRT& s = strings_[n.first + i];
        wake(s);
        s.dampTarget = s.dampRate = 0;  // the key lifts the damper well before the hammer arrives
    }
    if (!(speed > 0)) return;
    // Position and speed of each string along the strike direction, at the hammer.
    auto along = [&](int i, const std::vector<RealS>& f) {
        double g = 0;
        for (int pol = 0; pol < (n.twin ? 2 : 1); pol++) {
            const StringRT& s = strings_[n.first + i + pol * n.count];
            const RealS* x = &f[s.off];
            g += s.hamW * (s.hamG[0] * x[s.hamL] + s.hamG[1] * x[s.hamL + 1]);
        }
        return g;
    };
    double top = -1e30;
    for (int i = 0; i < n.count; i++) top = std::max(top, along(i, u_));
    n.eta = top - 1e-5;
    n.veta = speed * k_;
    n.hammerActive = true;
    for (int i = 0; i < n.count; i++) {
        StringRT& s = strings_[n.first + i];
        s.zeta = n.eta - along(i, u_);
        s.zetaPrev = s.zeta - (n.veta - along(i, v_[vb_]));
        s.force = 0;
    }
}

void Piano::noteOff(int midi) {
    int ni = noteOfMidi_[midi & 127];
    if (ni < 0) return;
    NoteRT& n = notes_[ni];
    n.keyDown = false;
    if (!sustain_)
        for (int i = 0; i < n.total; i++) {
            StringRT& s = strings_[n.first + i];
            if (s.hasDamper) s.dampTarget = dampMax_;
        }
}

void Piano::setSustain(bool down) {
    sustain_ = down;
    for (NoteRT& n : notes_)
        for (int i = 0; i < n.total; i++) {
            StringRT& s = strings_[n.first + i];
            if (!s.hasDamper) continue;
            if (down) { wake(s); s.dampTarget = 0; }
            else if (!n.keyDown) s.dampTarget = dampMax_;
        }
}

// Explicit update of one string: reads displacement u and the current increments, writes
// the next increments into the other buffer, and adds the bridge force to the plate right
// hand side. Displacements are advanced later, after the hammer has had its say.
void Piano::stepString(StringRT& s) {
    const int N = s.N;
    RealS* __restrict u = &u_[s.off];
    RealS* __restrict v = &v_[vb_][s.off];
    RealS* __restrict vn = &v_[vb_ ^ 1][s.off];
    const double uN = uEnd(s, w_.data()), vN = uEnd(s, vw_.data());

    if (!s.awake) {
        // A damped string absorbs what the bridge sends into it: characteristic impedance.
        double f = -k_ * s.z0 * vN;
        for (int q = 0; q < 4; q++) r_[s.at.idx[q]] += (RealP)(s.at.w[q] * f);
        return;
    }
    // Both ends are simply supported: zero bending moment, expressed through ghost nodes.
    u[N] = (RealS)uN; v[N] = (RealS)vN;
    u[-1] = -u[1];
    u[N + 1] = (RealS)(2 * uN - (double)u[N - 1]);

    if (s.attached) {
        double uA = u[N - 1], uB = u[N - 2], vA = v[N - 1];
        double f = s.mNode * (-(s.aT * (uN - uA) + s.aD * (vN - vA)) - s.aB * (uB - 2 * uA + uN));
        for (int q = 0; q < 4; q++) r_[s.at.idx[q]] += (RealP)(s.at.w[q] * f);
    }
    const double inv = 1 / (1 + s.s0k);
    const RealS cu0 = (RealS)((-2 * s.aT - 6 * s.aB) * inv), cu1 = (RealS)((s.aT + 4 * s.aB) * inv), cu2 = (RealS)(-s.aB * inv);
    const RealS cv0 = (RealS)((1 - s.s0k - 2 * s.aD) * inv), cv1 = (RealS)(s.aD * inv);
    for (int l = 1; l < N; l++)
        vn[l] = cv0 * v[l] + cv1 * (v[l - 1] + v[l + 1]) + cu0 * u[l] + cu1 * (u[l - 1] + u[l + 1]) + cu2 * (u[l - 2] + u[l + 2]);
    vn[0] = 0;

    // Damper felt: viscous, backward Euler so that it is stable at any strength.
    if (s.dampRate != s.dampTarget) {
        double rate = s.dampTarget > s.dampRate ? 0.004 : 0.02;  // engage ~5 ms, lift ~1 ms at 48 kHz
        s.dampRate += (s.dampTarget - s.dampRate) * rate * (48000.0 * k_);
        if (std::fabs(s.dampRate - s.dampTarget) < 1e-3) s.dampRate = s.dampTarget;
    }
    if (s.dampRate > 0) {
        RealS scale = (RealS)((1 + s.s0k) / (1 + s.s0k + 2 * k_ * s.dampRate));
        for (int l = s.dmp0; l < s.dmp1; l++) vn[l] *= scale;
    }
}

// Felt contact between one hammer and the strings of its note. The force is the discrete
// gradient of the felt potential, which makes the contact exactly energy consistent; the
// resulting nonlinear system is monotone and solved by a damped Newton iteration.
void Piano::stepHammer(NoteRT& n) {
    const int ns = n.count;
    const double K = n.hK, p = n.hP, b = k_ * k_ / n.hMass;
    double zm[3], zn[3], c[3], a[3], psi[3], r[3], F[3] = {0, 0, 0}, dF[3] = {0, 0, 0};
    bool contact = false;
    for (int i = 0; i < ns; i++) {
        StringRT& s = strings_[n.first + i];
        // Along the strike direction; v holds the increments the string would make without the hammer.
        double gu = 0, gv = 0;
        for (int pol = 0; pol < (n.twin ? 2 : 1); pol++) {
            const StringRT& sp = strings_[n.first + i + pol * n.count];
            const RealS* u = &u_[sp.off];
            const RealS* v = &v_[vb_ ^ 1][sp.off];
            gu += sp.hamW * (sp.hamG[0] * u[sp.hamL] + sp.hamG[1] * u[sp.hamL + 1]);
            gv += sp.hamW * (sp.hamG[0] * v[sp.hamL] + sp.hamG[1] * v[sp.hamL + 1]);
        }
        zn[i] = n.eta - gu;
        zm[i] = s.zetaPrev;
        c[i] = zn[i] + n.veta - gv - zm[i];
        a[i] = s.hamA;
        psi[i] = n.hMu * K * std::pow(pos(zn[i]), p) / (2 * k_);
        r[i] = c[i] - a[i] * s.force;
        if (zm[i] > 0 || zn[i] > 0 || zm[i] + c[i] > 0) contact = true;
    }
    double S = 0;
    if (contact) {
        auto phi = [&](double z) { return z > 0 ? K / (p + 1) * std::pow(z, p + 1) : 0.0; };
        auto dphi = [&](double z) { return z > 0 ? K * std::pow(z, p) : 0.0; };
        auto ddphi = [&](double z) { return z > 0 ? K * p * std::pow(z, p - 1) : 0.0; };
        auto eval = [&](const double* rr, double* G) {
            S = 0;
            for (int i = 0; i < ns; i++) {
                double zp = zm[i] + rr[i], sec, dsec;
                if (std::fabs(rr[i]) < 1e-13) {
                    sec = dphi(zm[i] + 0.5 * rr[i]);
                    dsec = 0.5 * ddphi(zm[i] + 0.5 * rr[i]);
                } else {
                    sec = (phi(zp) - phi(zm[i])) / rr[i];
                    dsec = (dphi(zp) - sec) / rr[i];
                }
                double f = sec + psi[i] * rr[i];
                if (f > 0) { F[i] = f; dF[i] = dsec + psi[i]; }
                else { F[i] = 0; dF[i] = 0; }  // felt cannot pull on the string
                S += F[i];
            }
            double worst = 0;
            for (int i = 0; i < ns; i++) {
                G[i] = rr[i] + a[i] * F[i] + b * S - c[i];
                worst = std::max(worst, std::fabs(G[i]));
            }
            return worst;
        };
        double prev = 0;
        for (int i = 0; i < ns; i++) { prev += strings_[n.first + i].force; }
        for (int i = 0; i < ns; i++) r[i] -= b * prev;
        double G[3], rt[3], Gt[3];
        double err = eval(r, G);
        int it = 0;
        for (; it < 60; it++) {
            double scale = 0;
            for (int i = 0; i < ns; i++) scale = std::max(scale, std::fabs(c[i]) + std::fabs(r[i]));
            // Rounding in the secant of the felt potential limits what can be reached.
            if (err <= 1e-11 * scale + 1e-22) break;
            double t = 0, den = 1, d[3];
            for (int i = 0; i < ns; i++) {
                d[i] = 1 + a[i] * dF[i];
                t += dF[i] * G[i] / d[i];
                den += b * dF[i] / d[i];
            }
            double step = 1, e2 = 0;
            bool improved = false;
            for (int ls = 0; ls < 12; ls++) {
                for (int i = 0; i < ns; i++) rt[i] = r[i] - step * (G[i] - b * t / den) / d[i];
                e2 = eval(rt, Gt);
                if (e2 < err) { improved = true; break; }
                step *= 0.5;
            }
            if (!improved) break;  // at the rounding floor
            for (int i = 0; i < ns; i++) { r[i] = rt[i]; G[i] = Gt[i]; }
            err = e2;
        }
        hammerItersMax_ = std::max(hammerItersMax_, it);
        eval(r, G);
        for (int i = 0; i < ns; i++) r[i] = c[i] - a[i] * F[i] - b * S;  // exact for the forces used
    } else {
        for (int i = 0; i < ns; i++) r[i] = c[i];
    }

    bool away = true;
    for (int i = 0; i < ns; i++) {
        StringRT& s = strings_[n.first + i];
        for (int pol = 0; pol < (n.twin ? 2 : 1); pol++) {
            const StringRT& sp = strings_[n.first + i + pol * n.count];
            RealS* v = &v_[vb_ ^ 1][sp.off];
            double q = sp.hamW * k_ * k_ * F[i] / (sp.mNode * (1 + sp.s0k));
            v[sp.hamL] += (RealS)(sp.hamG[0] * q);
            v[sp.hamL + 1] += (RealS)(sp.hamG[1] * q);
        }
        s.force = F[i];
        s.zetaPrev = zn[i];
        s.zeta = zm[i] + r[i];
        if (s.zeta > -2e-3) away = false;
    }
    n.veta -= b * S;
    n.eta += n.veta;
    if (away && n.veta < 0) {  // the hammer has fallen back; the action catches it
        n.hammerActive = false;
        for (int i = 0; i < ns; i++) strings_[n.first + i].force = 0;
    }
}

void Piano::housekeeping() {
    // A string that has sat under its damper for a while and has stopped moving on its own
    // is put to sleep. From then on the bridge sees it as a plain dashpot.
    const int settle = (int)(0.15 * fs_ / 64);
    for (StringRT& s : strings_) {
        if (!s.awake || !s.hasDamper || !s.attached) continue;
        if (s.dampRate < dampMax_ || notes_[s.note].hammerActive) { s.dampedSteps = 0; continue; }
        if (++s.dampedSteps < settle) continue;
        const RealS* u = &u_[s.off];
        double uN = u[s.N], m = 0;
        for (int l = 1; l < s.N; l++) m = std::max(m, std::fabs(u[l] - uN * l / s.N));
        if (m < 1e-8) sleep(s);
    }
}

void Piano::step(double& pL, double& pR) {
    const Plate& P = *plate_;
    RealP* __restrict r = r_.data();
    RealP* __restrict w = w_.data();
    RealP* __restrict vw = vw_.data();

    P.apply(w, vw, r);  // plate internal forces, scaled by dt^2
    if (extF_ != 0) {
        for (int q = 0; q < 4; q++) r[extAt_.idx[q]] += (RealP)(extAt_.w[q] * extF_ * k_ * k_);
        extF_ = 0;
    }

    for (StringRT& s : strings_) stepString(s);
    for (NoteRT& n : notes_)
        if (n.hammerActive) stepHammer(n);
    vb_ ^= 1;
    for (StringRT& s : strings_) {
        if (!s.awake) continue;
        RealS* __restrict u = &u_[s.off];
        const RealS* __restrict v = &v_[vb_][s.off];
        for (int l = 1; l < s.N; l++) u[l] += v[l];
    }

    // Implicit stiffener solve, then the plate update. Afterwards r holds the change of the
    // plate increment, i.e. dt^2 * acceleration, which is what radiates.
    P.solveStiff(r);
    const RealP* __restrict scale = P.diagScale();
    for (const Plate::Span& sp : P.spans) {
        RealP* __restrict ws = w + sp.start;
        RealP* __restrict vs = vw + sp.start;
        RealP* __restrict rs = r + sp.start;
        const RealP* __restrict ds = scale + sp.start;
        for (int t = 0; t < sp.len; t++) {
            rs[t] *= ds[t];
            vs[t] += rs[t];
            ws[t] += vs[t];
        }
    }

    for (int e = 0; e < 2; e++) {
        double* __restrict ring = ring_[e].data();
        const int* __restrict idx = tapIdx_[e].data();
        const RealP* __restrict g = tapGain_[e].data();
        const int* __restrict bp = binPtr_[e].data();
        const int bins = (int)binPtr_[e].size() - 1, base = ringPos_ + binFirst_[e];
        for (int b = 0; b < bins; b++) {
            // Four independent partial sums, so the additions do not wait on each other.
            RealP s0 = 0, s1 = 0, s2 = 0, s3 = 0;
            int q = bp[b];
            const int end = bp[b + 1];
            for (; q + 3 < end; q += 4) {
                s0 += g[q] * r[idx[q]];
                s1 += g[q + 1] * r[idx[q + 1]];
                s2 += g[q + 2] * r[idx[q + 2]];
                s3 += g[q + 3] * r[idx[q + 3]];
            }
            for (; q < end; q++) s0 += g[q] * r[idx[q]];
            ring[(base + b) & ringMask_] += (double)((s0 + s1) + (s2 + s3));
        }
    }
    pL = ring_[0][ringPos_]; ring_[0][ringPos_] = 0;
    pR = ring_[1][ringPos_]; ring_[1][ringPos_] = 0;
    ringPos_ = (ringPos_ + 1) & ringMask_;

    if ((++stepCount_ & 63) == 0) housekeeping();
}

void Piano::process(float* left, float* right, int n) {
    FlushDenormals flush;
    for (int f = 0; f < n; f++) {
        double l = 0, r = 0;
        if (os_ == 1) {
            step(l, r);
        } else {
            const int len = (int)fir_.size();
            for (int s = 0; s < os_; s++) {
                double a, b;
                step(a, b);
                hist_[0][histPos_] = a;
                hist_[1][histPos_] = b;
                histPos_ = (histPos_ + 1) % len;
            }
            for (int i = 0; i < len; i++) {
                int j = histPos_ + i; if (j >= len) j -= len;
                l += fir_[i] * hist_[0][j];
                r += fir_[i] * hist_[1][j];
            }
        }
        left[f] = (float)(l * outGain_);
        right[f] = (float)(r * outGain_);
    }
}

// ---------------------------------------------------------------- diagnostics

double Piano::energy() const {
    const Plate& P = *plate_;
    const double k = k_;
    // Evaluated directly from energy densities, independently of the stepping stencils.
    // With x = 2 q - v (q, v at the newest step): H = KE + (V2(x) - V2(v)) / 8.
    std::vector<double> xg(P.gridSize), vg(P.gridSize), xp(P.n), vp(P.n);
    for (int g = 0; g < P.gridSize; g++) { vg[g] = vw_[g]; xg[g] = 2 * (double)w_[g] - vg[g]; }
    P.gather(xg.data(), xp.data());
    P.gather(vg.data(), vp.data());
    double kin = 0, v2x = P.K.quad(xp.data()), v2v = P.K.quad(vp.data());
    for (int a = 0; a < P.n; a++) kin += P.mass[a] * vp[a] * vp[a];
    kin += 0.25 * k * k * P.Kimp.quad(vp.data()) - 0.5 * k * P.D.quad(vp.data());

    for (const StringRT& s : strings_) {
        if (!s.awake) continue;
        const int N = s.N;
        const RealS* u = &u_[s.off];
        const RealS* v = &v_[vb_][s.off];
        std::vector<double> x(N + 1, 0.0), vv(N + 1, 0.0);
        for (int l = 1; l < N; l++) { vv[l] = v[l]; x[l] = 2 * (double)u[l] - vv[l]; }
        vv[N] = uEnd(s, vg.data());
        x[N] = uEnd(s, xg.data());
        auto v2 = [&](const std::vector<double>& y) {
            double e = 0;
            for (int l = 0; l < N; l++) e += s.T_h * (y[l + 1] - y[l]) * (y[l + 1] - y[l]);
            for (int l = 1; l < N; l++) {
                double c = y[l - 1] - 2 * y[l] + y[l + 1];
                e += s.EI_h3 * c * c;
            }
            return e;
        };
        v2x += v2(x);
        v2v += v2(vv);
        for (int l = 1; l < N; l++) kin += s.mNode * vv[l] * vv[l];
        double dis = 0;
        for (int l = 0; l < N; l++) dis += s.damp_h * (vv[l + 1] - vv[l]) * (vv[l + 1] - vv[l]);
        kin -= 0.5 * k * dis;
    }
    double H = 0.5 * kin / (k * k) + (v2x - v2v) / 8.0;
    for (const NoteRT& n : notes_) {
        if (!n.hammerActive) continue;
        H += 0.5 * n.hMass * (n.veta / k) * (n.veta / k);
        for (int i = 0; i < n.count; i++) {
            const StringRT& s = strings_[n.first + i];
            auto phi = [&](double z) { return z > 0 ? n.hK / (n.hP + 1) * std::pow(z, n.hP + 1) : 0.0; };
            H += 0.5 * (phi(s.zeta) + phi(s.zetaPrev));
        }
    }
    return H;
}

void Piano::prepareSnapshot(Snapshot& s) const {
    s.board.assign(w_.size(), 0.0f);
    s.stringLevel.assign(strings_.size(), 0.0f);
    int maxN = 0;
    for (const StringRT& st : strings_) maxN = std::max(maxN, st.N + 1);
    for (auto& sh : s.shape) sh.assign(maxN, 0.0f);
}

void Piano::fillSnapshot(Snapshot& s, int focusMidi) const {
    for (size_t g = 0; g < w_.size(); g++) s.board[g] = (float)w_[g];
    s.awake = 0;
    for (size_t i = 0; i < strings_.size(); i++) {
        const StringRT& st = strings_[i];
        float m = 0;
        if (st.awake) {
            s.awake++;
            const RealS* u = &u_[st.off];
            for (int l = 1; l < st.N; l++) m = std::max(m, (float)std::fabs(u[l]));
        }
        s.stringLevel[i] = m;
    }
    s.focusMidi = focusMidi;
    for (int i = 0; i < 3; i++) { s.shapeNodes[i] = 0; s.hammerForce[i] = 0; }
    int ni = focusMidi >= 0 ? noteOfMidi_[focusMidi & 127] : -1;
    if (ni < 0) return;
    for (int i = 0; i < notes_[ni].count; i++) {
        const StringRT& st = strings_[notes_[ni].first + i];
        const RealS* u = &u_[st.off];
        s.shapeNodes[i] = st.N + 1;
        for (int l = 0; l <= st.N; l++) s.shape[i][l] = st.awake ? (float)u[l] : 0.0f;
        s.hammerForce[i] = (float)st.force;
    }
}

int Piano::awakeStrings() const {
    int c = 0;
    for (const StringRT& s : strings_) c += s.awake;
    return c;
}

int Piano::totalStringNodes() const {
    int c = 0;
    for (const StringRT& s : strings_) c += s.N - 1;
    return c;
}

Piano::StringView Piano::stringView(int i) const {
    const StringRT& s = strings_[i];
    return {notes_[s.note].midi, s.N + 1, s.h, &u_[s.off], s.force, s.awake, s.tension};
}

std::vector<int> Piano::stringsOfNote(int midi) const {
    std::vector<int> out;
    int ni = noteOfMidi_[midi & 127];
    if (ni >= 0)
        for (int i = 0; i < notes_[ni].count; i++) out.push_back(notes_[ni].first + i);
    return out;
}

double Piano::discretePartial(int index, int p) const {
    const StringRT& s = strings_[index];
    double sp = std::pow(std::sin(p * M_PI / (2 * s.N)), 2);
    double x = s.aT * sp + 4 * s.aB * sp * sp;  // sin^2(omega dt / 2)
    return x >= 1 ? 0.0 : std::asin(std::sqrt(x)) / (M_PI * k_);
}

double Piano::continuumPartial(int index, int p) const {
    const StringRT& s = strings_[index];
    return p * s.f0 * std::sqrt((1 + s.inharm * p * p) / (1 + s.inharm));
}

}  // namespace pp
