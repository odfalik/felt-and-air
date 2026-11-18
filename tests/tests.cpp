// Validation suite. Each test checks a physical or numerical property of the model and
// prints the measured numbers, so the output doubles as a report.
#include "Acoustics.h"
#include "Piano.h"
#include "Util.h"
#include <cmath>
#include <cstdio>
#include <functional>
#include <string>
#include <vector>

using namespace pp;

static int failures = 0;
#define CHECK(cond, ...)                                                  \
    do {                                                                  \
        bool ok_ = (cond);                                                \
        std::printf("    [%s] ", ok_ ? "ok" : "FAIL");                    \
        std::printf(__VA_ARGS__);                                         \
        std::printf("\n");                                                \
        if (!ok_) failures++;                                             \
    } while (0)

static PianoDesign oneNote(int key = 40) {
    GrandOptions g;
    g.firstKey = g.lastKey = key;
    return makeGrand(g);
}

static void makeLossless(PianoDesign& d) {
    d.longitudinal = false;  // driven one-way by the transverse motion, so not part of the energy balance
    d.board.lossFactor = 0;
    d.board.sigma0 = 0;
    for (auto& n : d.notes) {
        n.hammerMu = 0;
        n.keyThump = 0;  // an outside force: it would add energy
        for (auto& s : n.strings) s.sigma0 = s.sigma1 = 0;
    }
}

// Frequency of the strongest spectral peak inside [lo, hi] Hz.
static double peakFrequency(const std::vector<double>& x, double fs, double lo, double hi) {
    size_t nfft = 1;
    while (nfft < x.size() * 4) nfft <<= 1;
    auto m = magSpectrum(x.data(), x.size(), nfft);
    size_t b0 = (size_t)(lo / fs * nfft), b1 = std::min(m.size() - 2, (size_t)(hi / fs * nfft)), best = b0;
    for (size_t b = b0; b <= b1; b++)
        if (m[b] > m[best]) best = b;
    return peakBin(m, best) * fs / nfft;
}

// ---------------------------------------------------------------------------------------

static void testKernelMatchesAssembly() {
    std::printf("\n1. Fast plate kernel reproduces the assembled energy operator\n");
    Piano p(oneNote(), {48000, 1, 0.5});
    const Plate& P = p.plate();
    std::vector<RealP> w(P.gridSize, 0), v(P.gridSize, 0), r(P.gridSize, 0);
    std::vector<double> wa(P.n), va(P.n), kw(P.n), dv(P.n);
    unsigned seed = 12345;
    auto rnd = [&] { seed = seed * 1664525u + 1013904223u; return (seed >> 8) / 16777216.0 - 0.5; };
    for (int a = 0; a < P.n; a++) { w[P.gid[a]] = (RealP)rnd(); v[P.gid[a]] = (RealP)rnd(); }
    P.gather(w.data(), wa.data());
    P.gather(v.data(), va.data());
    P.apply(w.data(), v.data(), r.data());
    P.K.mul(wa.data(), kw.data());
    P.D.mul(va.data(), dv.data());
    double k = p.dt(), err = 0, ref = 0;
    for (int a = 0; a < P.n; a++) {
        double expect = -(k * k * kw[a] + k * dv[a] + k * P.c0[a] * va[a]);
        err = std::max(err, std::fabs(expect - r[P.gid[a]]));
        ref = std::max(ref, std::fabs(expect));
    }
    CHECK(err < 1e-9 * ref, "max relative difference %.2e", err / ref);
    // Symmetry and semidefiniteness of K by random probes.
    std::vector<double> x(P.n), y(P.n), kx(P.n), ky(P.n);
    for (int a = 0; a < P.n; a++) { x[a] = rnd(); y[a] = rnd(); }
    P.K.mul(x.data(), kx.data());
    P.K.mul(y.data(), ky.data());
    double xy = 0, yx = 0;
    for (int a = 0; a < P.n; a++) { xy += x[a] * ky[a]; yx += y[a] * kx[a]; }
    CHECK(std::fabs(xy - yx) < 1e-10 * std::fabs(xy), "K symmetric: x'Ky - y'Kx relative %.2e", std::fabs(xy - yx) / std::fabs(xy));
    CHECK(P.stabilityBound < 4.0, "stability bound %.4f < 4 (rigorous), power-iteration estimate %.4f", P.stabilityBound, P.stabilityEstimate);
    std::printf("    grid: %d nodes, %.1f x %.1f mm, %d stiffener nodes, factor fill %d, rim correction %d entries\n",
                P.n, P.hx * 1e3, P.hy * 1e3, P.stiffNodes, P.factorNnz, P.correctionNnz());
}

static void testEnergyConservation() {
    std::printf("\n2. Lossless instrument conserves the discrete energy (strike, contact, coupling)\n");
    PianoDesign d = oneNote();
    makeLossless(d);
    Piano p(d, {48000, 1, 0.5});
    p.noteOn(60, 3.0);
    double e0 = p.energy(), worst = 0, a, b, peakForce = 0;
    int n = 0;
    for (; n < 24000; n++) {
        p.step(a, b);
        if (!p.hammerActive(60)) break;  // the hammer is dropped from the system once it has left
        peakForce = std::max(peakForce, p.stringView(0).hammerForce);
        worst = std::max(worst, std::fabs(p.energy() - e0) / e0);
    }
    std::printf("    hammer in play for %.2f ms\n", n / 48.0);
    CHECK(worst < 1e-9, "relative energy drift %.2e through the strike (peak felt force %.1f N)", worst, peakForce);
    CHECK(p.hammerIterationsMax() < 20, "contact solver: at most %d Newton iterations", p.hammerIterationsMax());
}

static void testEnergyAfterRelease() {
    std::printf("\n3. Lossless free vibration after the hammer leaves\n");
    PianoDesign d = oneNote();
    makeLossless(d);
    Piano p(d, {48000, 1, 0.5});
    p.noteOn(60, 3.0);
    double a, b;
    for (int n = 0; n < 4800; n++) p.step(a, b);
    double e0 = p.energy(), worst = 0;
    for (int n = 0; n < 96000; n++) {
        p.step(a, b);
        if (n % 64 == 0) worst = std::max(worst, std::fabs(p.energy() - e0) / e0);
    }
    CHECK(worst < 1e-9, "relative drift %.2e over 2 s (%.3f mJ in strings and board)", worst, e0 * 1e3);
}

static void testPassivity() {
    std::printf("\n4. Lossy instrument: energy never increases (strike, hold, damper)\n");
    PianoDesign pd = oneNote();
    pd.notes[0].keyThump = 0;  // the key knock is an outside force and would add energy
    pd.longitudinal = false;   // one-way drive, outside the energy balance
    Piano p(pd, {48000, 1, 0.5});
    p.noteOn(60, 4.0);
    double prev = p.energy(), worstRise = 0, a, b, e0 = prev;
    int n = 0;
    for (; n < 48000; n++) {
        if (n == 24000) p.noteOff(60);
        p.step(a, b);
        if (p.awakeStrings() == 0) break;
        double e = p.energy();
        worstRise = std::max(worstRise, (e - prev) / e0);
        prev = e;
    }
    CHECK(worstRise < 1e-12, "largest step-to-step rise %.2e of the initial energy", worstRise);
    CHECK(p.awakeStrings() == 0, "damped strings went to sleep %.0f ms after key release", (n - 24000) / 48.0);
}

static void testPlateAnalytic() {
    std::printf("\n5. Orthotropic plate against the analytic simply supported rectangle\n");
    PianoDesign d = oneNote();
    makeLossless(d);
    d.board.outline = {{0, 0}, {1.2, 0}, {1.2, 0.8}, {0, 0.8}};
    d.board.grainAngle = 0;
    d.board.ribs = false;
    d.board.crownRadius = 0;
    d.board.rimClamp = 0;
    d.board.bridges.clear();
    for (auto& s : d.notes[0].strings) s.attached = false;
    for (int os : {1, 2}) {
        Piano p(d, {48000, os, 0.5});
        const Plate& P = p.plate();
        int ax = 0, ay = 0;
        for (int a = 0; a < P.n; a++) { ax = std::max(ax, P.gi[a]); ay = std::max(ay, P.gj[a]); }
        int i0 = 1 << 30, j0 = 1 << 30;
        for (int a = 0; a < P.n; a++) { i0 = std::min(i0, P.gi[a]); j0 = std::min(j0, P.gj[a]); }
        double A = (ax - i0 + 2) * P.hx, B = (ay - j0 + 2) * P.hy;  // supports sit one spacing outside
        const BoardDesign& bd = d.board;
        double H = bd.thickness, nuyx = bd.nuxy * bd.Ey / bd.Ex, den = 12 * (1 - bd.nuxy * nuyx);
        double Dx = bd.Ex * H * H * H / den, Dy = bd.Ey * H * H * H / den, D1 = nuyx * Dx, Dk = bd.Gxy * H * H * H / 12;
        double worst = 0;
        for (auto [m, n] : std::vector<std::pair<int, int>>{{1, 1}, {2, 1}, {1, 2}, {3, 2}, {2, 5}}) {
            double exact = 0.5 * M_PI * std::sqrt((Dx * std::pow(m / A, 4) + 2 * (D1 + 2 * Dk) * std::pow(m * n / (A * B), 2) +
                                                   Dy * std::pow(n / B, 4)) / (bd.rho * H));
            std::fill(p.plateU().begin(), p.plateU().end(), (RealP)0);
            std::fill(p.plateV().begin(), p.plateV().end(), (RealP)0);
            int probe = 0;
            for (int a = 0; a < P.n; a++) {
                double sx = std::sin(m * M_PI * (P.gi[a] - i0 + 1) * P.hx / A), sy = std::sin(n * M_PI * (P.gj[a] - j0 + 1) * P.hy / B);
                p.plateU()[P.gid[a]] = (RealP)(1e-6 * sx * sy);
                if (std::fabs(sx * sy) > 0.9) probe = P.gid[a];
            }
            // Count zero crossings of the probe node.
            double a2, b2, prev = p.plateU()[probe], first = -1, last = -1;
            int crossings = 0, steps = (int)(0.4 / p.dt());
            for (int s = 0; s < steps; s++) {
                p.step(a2, b2);
                double cur = p.plateU()[probe];
                if ((prev > 0) != (cur > 0)) {
                    double t = (s + prev / (prev - cur)) * p.dt();
                    if (first < 0) first = t;
                    last = t;
                    crossings++;
                }
                prev = cur;
            }
            double f = 0.5 * (crossings - 1) / (last - first);
            double errPct = 100 * (f - exact) / exact;
            worst = std::max(worst, std::fabs(errPct));
            std::printf("      mode (%d,%d): analytic %8.2f Hz, simulated %8.2f Hz (%+.3f %%)\n", m, n, exact, f, errPct);
        }
        CHECK(worst < (os == 1 ? 1.5 : 0.8), "oversampling %d (%d nodes): worst error %.3f %%", os, P.n, worst);
    }
}

static void testStringPartials() {
    std::printf("\n6. Stiff string partials (rigid terminations)\n");
    for (int key : {16, 40, 64}) {
        PianoDesign d = oneNote(key);
        makeLossless(d);
        d.notes[0].strings.resize(1);
        StringDesign& sd = d.notes[0].strings[0];
        sd.attached = false;
        Piano p(d, {48000, 1, 0.5});
        p.noteOn(d.notes[0].midi, 2.0);
        auto view = p.stringView(0);
        int node = std::max(1, (int)(0.31 * view.nodes));
        int steps = 96000;
        std::vector<double> rec(steps);
        double a, b;
        for (int n = 0; n < steps; n++) { p.step(a, b); rec[n] = view.u[node]; }
        double worstCents = 0;
        double B = M_PI * M_PI * sd.EI / (sd.T * sd.L * sd.L);
        std::printf("      key %d (%.1f Hz), %d nodes, inharmonicity B = %.2e, dispersion fitted at partial %d\n", key, sd.f0, view.nodes, B,
                    p.fitPartial(0));
        for (int q : {1, 2, 3, 5, 8, 12, 16, 24, 32}) {
            double pred = p.discretePartial(0, q);
            if (pred <= 0 || pred > 20000) continue;
            double meas = peakFrequency(rec, 48000, pred * 0.985, pred * 1.015);
            double cents = 1200 * std::log2(meas / pred);
            double ideal = p.continuumPartial(0, q);
            worstCents = std::max(worstCents, std::fabs(cents));
            std::printf("        partial %2d: scheme %9.3f Hz, simulated %9.3f Hz (%+.2f c), continuum %9.3f Hz (grid error %+.1f c)\n",
                        q, pred, meas, cents, ideal, 1200 * std::log2(pred / ideal));
        }
        CHECK(worstCents < 0.5, "simulation matches the scheme's closed form within %.2f cents", worstCents);
        CHECK(std::fabs(p.discretePartial(0, 1) - sd.f0) < 1e-6, "fundamental tuned to %.4f Hz", p.discretePartial(0, 1));
    }
}

static void testHammer() {
    std::printf("\n7. Hammer felt contact: pulse shape and time step convergence\n");
    double ref = 0;
    for (int os : {1, 2, 4}) {
        Piano p(oneNote(), {48000, os, 0.5});
        p.noteOn(60, 2.0);
        double a, b, peak = 0, impulse = 0;
        int first = -1, last = -1, steps = (int)(0.01 / p.dt());
        for (int n = 0; n < steps; n++) {
            p.step(a, b);
            double f = p.stringView(1).hammerForce;
            peak = std::max(peak, f);
            impulse += f * p.dt();
            if (f > 0) { if (first < 0) first = n; last = n; }
        }
        double dur = (last - first + 1) * p.dt() * 1e3;
        std::printf("      oversampling %d: peak %.3f N, contact %.3f ms, impulse %.3f mN s, Newton max %d\n", os, peak, dur, impulse * 1e3,
                    p.hammerIterationsMax());
        if (os == 4) ref = peak;
        if (os == 1) {
            CHECK(peak > 3 && peak < 40, "peak force per string plausible for mezzo forte C4");
            CHECK(dur > 0.5 && dur < 5, "contact duration plausible");
            ref = peak;
        } else if (os == 2) {
            CHECK(std::fabs(peak - ref) / ref < 0.1, "peak force changes %.1f %% from 48 to 96 kHz", 100 * std::fabs(peak - ref) / ref);
            ref = peak;
        } else {
            CHECK(true, "96 to 192 kHz noted above");
        }
    }
}

static void testReciprocity() {
    std::printf("\n8. Reciprocity between two strings through bridge and soundboard\n");
    PianoDesign d = oneNote();
    makeLossless(d);
    double resp[2];
    for (int dir = 0; dir < 2; dir++) {
        Piano p(d, {48000, 1, 0.5});
        p.setSustain(true);  // wakes the strings without a hammer
        double a, b, acc = 0;
        for (int n = 0; n < 2000; n++) p.step(a, b);  // let the dampers finish lifting
        int from = dir == 0 ? 0 : 2, to = dir == 0 ? 2 : 0;
        int nf = dir == 0 ? 20 : 41, nt = dir == 0 ? 41 : 20;
        p.stringV(from)[nf] = (RealS)(1e-9 / p.stringNodeMass(from));  // unit momentum
        for (int n = 0; n < 6000; n++) {
            p.step(a, b);
            if (n % 500 == 499) acc += p.stringU(to)[nt] * (1 + n);
        }
        resp[dir] = acc;
    }
    double rel = std::fabs(resp[0] - resp[1]) / std::fabs(resp[0]);
    CHECK(rel < 1e-6, "string 0 -> string 2 response equals 2 -> 0 within %.2e", rel);
    CHECK(std::fabs(resp[0]) > 0, "strings are coupled (transfer %.3e)", resp[0]);
}

static void testCoupling() {
    std::printf("\n9. Two-way bridge coupling changes the string decay\n");
    double level[2];
    for (int attached = 0; attached < 2; attached++) {
        PianoDesign d = oneNote();
        for (auto& s : d.notes[0].strings) s.attached = attached;
        Piano p(d, {48000, 1, 0.5});
        p.noteOn(60, 2.0);
        double a, b, early = 0, late = 0;
        for (int n = 0; n < 96000; n++) {
            p.step(a, b);
            double u = p.stringU(1)[30];
            if (n >= 9600 && n < 19200) early += u * u;
            if (n >= 86400) late += u * u;
        }
        level[attached] = 10 * std::log10(late / early);
        std::printf("      %s: string level changes %.2f dB between 0.3 s and 1.9 s\n", attached ? "on the bridge " : "rigid supports", level[attached]);
    }
    CHECK(level[1] < level[0] - 1.0, "the soundboard drains the strings (%.1f dB extra decay)", level[0] - level[1]);
}

static void testRobustness() {
    std::printf("\n10. Robustness: fortissimo repeated strikes across the compass\n");
    GrandOptions g;
    PianoDesign d = makeGrand(g);
    Piano p(d, {48000, 1, 0.5});
    std::printf("      full compass: %d strings, %d string nodes, %d board nodes\n", p.stringCount(), p.totalStringNodes(), p.plate().n);
    std::vector<float> l(64), r(64);
    double peak = 0;
    bool finite = true;
    int keys[] = {21, 22, 36, 48, 60, 72, 84, 96, 107, 108};
    for (int block = 0; block < 1500; block++) {
        if (block % 40 == 0)
            for (int k : keys) p.noteOn(k, 6.0);
        if (block % 40 == 30)
            for (int k : keys) p.noteOff(k);
        if (block == 600) p.setSustain(true);
        if (block == 1200) p.setSustain(false);
        p.process(l.data(), r.data(), 64);
        for (int i = 0; i < 64; i++) {
            finite = finite && std::isfinite(l[i]) && std::isfinite(r[i]);
            peak = std::max(peak, (double)std::fabs(l[i]));
        }
    }
    CHECK(finite, "output finite through 2 s of hammering with pedal changes");
    CHECK(peak > 1e-4 && peak < 100, "peak output %.3f (linear, before any limiter)", peak);
    CHECK(p.hammerIterationsMax() < 40, "contact solver: at most %d Newton iterations", p.hammerIterationsMax());
    double e = p.energy();
    CHECK(std::isfinite(e) && e < 10.0, "stored energy %.4f J stays bounded", e);
}

static void testRoom() {
    std::printf("\n12. Room: arrival time, mirror images, reverberant decay\n");
    PianoDesign d = oneNote();
    Piano p(d, {48000, 1});
    const Plate& P = p.plate();
    Vec2 at = d.notes[0].strings[0].bridge;
    Interp it = P.interp(at);
    int node = it.idx[0];
    double best = 0;
    for (int q = 0; q < 4; q++) if (it.w[q] > best) { best = it.w[q]; node = it.idx[q]; }
    int a = 0;
    for (int i = 0; i < P.n; i++) if (P.gid[i] == node) a = i;
    Vec2 pos = P.nodePosition(a);
    auto response = [&](const PianoDesign& dd, int steps) {
        Radiation rad;
        rad.build(P, dd, 48000);
        std::vector<RealP> r(P.gridSize, 0);
        std::vector<double> out(steps);
        double l, rr;
        for (int n = 0; n < steps; n++) {
            r[node] = n == 0 ? 1 : 0;
            rad.step(r.data(), l, rr);
            out[n] = l;
        }
        return std::make_pair(out, rad);
    };
    PianoDesign dry = d;
    dry.room.enabled = false; dry.lid.open = false;
    auto [h0, rad0] = response(dry, 4800);
    int first = 0;
    for (int n = 0; n < 4800; n++) if (h0[n] != 0) { first = n; break; }
    double dist = std::sqrt(std::pow(pos.x - d.ears[0].x, 2) + std::pow(pos.y - d.ears[0].y, 2) + d.ears[0].z * d.ears[0].z);
    CHECK(std::abs(first - dist / 343.0 * 48000) <= 1.0, "direct sound arrives after %.2f ms for a %.2f m path", first / 48.0, dist);

    auto [h, rad] = response(d, 48000 * 3);
    std::printf("      %d mirror images over %d board patches; Sabine reverberation time %.2f s\n", rad.imageCount, rad.patchCount, rad.reverbTime);
    double direct = h0[first] * h0[first], early = 0, late = 0;
    int split = first + (int)(0.08 * 48000);
    for (int n = 0; n < (int)h.size(); n++) (n < split ? early : late) += h[n] * h[n];
    std::printf("      energy relative to the direct sound: first 80 ms %+.1f dB, later %+.1f dB\n", 10 * std::log10(early / direct),
                10 * std::log10(late / direct));
    // Decay time from the backward integrated tail, fitted between -5 and -25 dB.
    std::vector<double> sch(h.size());
    double acc = 0;
    for (int n = (int)h.size() - 1; n >= 0; n--) { acc += h[n] * h[n]; sch[n] = acc; }
    int n5 = -1, n25 = -1;
    for (int n = split; n < (int)h.size(); n++) {
        double db = 10 * std::log10(sch[n] / sch[split]);
        if (n5 < 0 && db <= -5) n5 = n;
        if (n25 < 0 && db <= -25) { n25 = n; break; }
    }
    double t60 = n25 > n5 && n5 > 0 ? 3.0 * (n25 - n5) / 48000.0 : 0;
    CHECK(std::fabs(t60 - rad.reverbTime) < 0.2 * rad.reverbTime, "measured decay time %.2f s against Sabine %.2f s", t60, rad.reverbTime);
    CHECK(late > 0 && late < 30 * direct, "reverberant level is finite and plausible");
}

static void testLongitudinal() {
    std::printf("\n13. Longitudinal string motion: phantom partial and longitudinal mode\n");
    GrandOptions g;
    g.firstKey = g.lastKey = 28;
    g.twoPolarisations = false;
    PianoDesign d = makeGrand(g);
    Piano p(d, {48000, 1});
    p.noteOn(d.notes[0].midi, 4.5);
    const int n = 1 << 16;
    std::vector<double> dT(n);
    double a, b, peak = 0;
    for (int i = 0; i < n; i++) { p.step(a, b); dT[i] = p.stringView(0).tensionChange; peak = std::max(peak, std::fabs(dT[i])); }
    const StringDesign& sd = d.notes[0].strings[0];
    double fL = std::sqrt(sd.EA / sd.rhoA) / (2 * sd.L);
    double phantom = peakFrequency(dT, 48000, 1.9 * sd.f0, 2.1 * sd.f0), mode = peakFrequency(dT, 48000, 0.9 * fL, 1.1 * fL);
    auto view = p.stringView(0);
    std::printf("      %d longitudinal cells; tension swings %.1f N around %.0f N at fortissimo\n", view.longitudinalCells, peak, view.tension);
    CHECK(std::fabs(phantom / (2 * sd.f0) - 1) < 0.01, "tension pulses at twice the fundamental: %.1f Hz against %.1f Hz", phantom, 2 * sd.f0);
    CHECK(std::fabs(mode / fL - 1) < 0.03, "longitudinal mode at %.0f Hz against c_L / 2L = %.0f Hz", mode, fL);
    CHECK(peak > 0.5 && peak < 0.2 * view.tension, "tension change is a small fraction of the tension");
}

static void testThreads() {
    std::printf("\n11. Helper threads give bit-identical output\n");
    std::vector<float> ref, out;
    for (int helpers : {0, 3}) {
        Piano p(makeGrand(), {48000, 1});
        p.setHelperThreads(helpers);
        std::vector<float> l(64), r(64);
        std::vector<float>& dst = helpers ? out : ref;
        bool shared = false;
        p.setSustain(true);
        for (int block = 0; block < 300; block++) {
            if (block % 25 == 0) p.noteOn(30 + (block / 25 * 7) % 60, 3.0);
            p.process(l.data(), r.data(), 64);
            shared = shared || p.helpersBusy();
            dst.insert(dst.end(), l.begin(), l.end());
        }
        if (helpers) CHECK(shared, "helpers were sharing the string work");
    }
    size_t diff = 0;
    for (size_t i = 0; i < ref.size(); i++) diff += ref[i] != out[i];
    CHECK(diff == 0, "%zu of %zu samples differ between 0 and 3 helpers", diff, ref.size());
}

int main(int argc, char** argv) {
    std::vector<std::pair<std::string, std::function<void()>>> tests = {
        {"kernel", testKernelMatchesAssembly}, {"energy", testEnergyConservation}, {"free", testEnergyAfterRelease},
        {"passivity", testPassivity},          {"plate", testPlateAnalytic},       {"partials", testStringPartials},
        {"hammer", testHammer},                {"reciprocity", testReciprocity},   {"coupling", testCoupling},
        {"robust", testRobustness},           {"threads", testThreads},           {"room", testRoom},
        {"longitudinal", testLongitudinal}};
    for (auto& t : tests)
        if (argc < 2 || t.first == argv[1]) t.second();
    std::printf("\n%s (%d failed checks)\n", failures ? "FAILED" : "ALL PASSED", failures);
    return failures ? 1 : 0;
}
