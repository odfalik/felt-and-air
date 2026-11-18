#include "Piano.h"
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <mutex>
#include <stdexcept>
#include <thread>
#if defined(__APPLE__)
#include <pthread.h>
#endif

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

// Work sharing for the strings. Within one time step every string depends only on the state
// of the previous step, so strings can be advanced side by side. The audio thread publishes
// a job, takes chunks itself, and helpers take the rest; if a helper is late the audio thread
// simply ends up doing more of the chunks, so a slow helper costs time but never correctness.
struct Piano::Pool {
    std::vector<std::thread> threads;
    // One word holds the whole job: generation, phase and the next unclaimed chunk. A chunk
    // is claimed by compare-and-swap on that word, so a thread can never pair a chunk of a
    // new job with the phase of an old one.
    std::atomic<uint64_t> ticket{3ull << 32};  // phase 3: nothing to do
    std::atomic<int> done{0};
    std::atomic<bool> stop{false}, active{false};
    std::atomic<bool> inBlock{false};  // helpers spin only while a block is being rendered
    std::mutex m;
    std::condition_variable cv;
    int count[3] = {0, 0, 0};  // chunks in each phase
    uint64_t generation = 0;

    // Claims a chunk of the current job. Returns false when none are left.
    bool claim(int& phase, int& chunk) {
        uint64_t t = ticket.load(std::memory_order_acquire);
        for (;;) {
            uint32_t idx = (uint32_t)t;
            int ph = (int)((t >> 32) & 3);
            if (ph > 2 || idx >= (uint32_t)count[ph]) return false;
            if (ticket.compare_exchange_weak(t, t + 1, std::memory_order_acq_rel)) {
                phase = ph;
                chunk = (int)idx;
                return true;
            }
        }
    }
    void post(int phase) {
        done.store(0, std::memory_order_release);
        ticket.store((++generation << 34) | ((uint64_t)phase << 32), std::memory_order_release);
    }
};

Piano::~Piano() { setHelperThreads(0); }

void Piano::setHelperThreads(int count, std::function<void()> onStart, std::function<void()> onExit) {
    if (pool_) {
        pool_->stop = true;
        { std::lock_guard<std::mutex> g(pool_->m); }
        pool_->cv.notify_all();
        for (auto& t : pool_->threads) t.join();
        pool_.reset();
    }
    if (count <= 0) return;
    pool_ = std::make_unique<Pool>();
    Pool* p = pool_.get();
    for (int ph = 0; ph < 3; ph++) p->count[ph] = (int)phaseChunks_[ph].size();
    for (int i = 0; i < count; i++)
        p->threads.emplace_back([this, p, onStart, onExit] {
#if defined(__APPLE__)
            pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
#endif
            if (onStart) onStart();
            FlushDenormals flush;
            while (!p->stop.load(std::memory_order_relaxed)) {
                if (!p->active.load(std::memory_order_acquire) || !p->inBlock.load(std::memory_order_acquire)) {
                    // Between blocks, and while there is little to share, sleep. A missed
                    // wake-up only means sitting out one block.
                    std::unique_lock<std::mutex> lk(p->m);
                    p->cv.wait_for(lk, std::chrono::milliseconds(50),
                                   [p] { return p->stop.load() || (p->active.load() && p->inBlock.load()); });
                    continue;
                }
                int ph, c;
                if (p->claim(ph, c)) {
                    chunkWork(ph, c);
                    p->done.fetch_add(1, std::memory_order_release);
                } else {
#if defined(__aarch64__)
                    __asm__ volatile("yield");
#endif
                }
            }
            if (onExit) onExit();
        });
}

bool Piano::helpersBusy() const { return pool_ && pool_->active.load(); }

// Phase 0 computes forces from the state of the previous step (strings, plate stencil) and
// the sound of the previous step. Phase 1 advances the string displacements. Phase 2 advances
// the plate once its implicit solve is done.
void Piano::chunkWork(int phase, int index) {
    const Chunk& c = chunks_[phaseChunks_[phase][index]];
    const Plate& P = *plate_;
    if (c.kind == 0) {
        if (phase == 0) radiation_.step(radIn_.data(), radL_, radR_);
    } else if (c.kind == 1) {
        if (phase == 0) {
            P.applyStencil(w_.data(), vw_.data(), r_.data(), c.from, c.to);
            // Reaction of the air, as worked out by the radiation task one step ago.
            if (!airNow_.empty()) {
                RealP* __restrict r = r_.data();
                const int* __restrict patch = radiation_.patchOfGrid().data();
                const RealP* __restrict air = airNow_.data();
                for (int si = c.from; si < c.to; si++) {
                    const Plate::Span& sp = P.spans[si];
                    for (int t = sp.start; t < sp.start + sp.len; t++) r[t] += air[patch[t]];
                }
            }
        } else if (phase == 2) {
            RealP* __restrict w = w_.data();
            RealP* __restrict vw = vw_.data();
            RealP* __restrict r = r_.data();
            RealP* __restrict keep = radIn_.data();
            const RealP* __restrict scale = P.diagScale();
            for (int si = c.from; si < c.to; si++) {
                const Plate::Span& sp = P.spans[si];
                for (int t = sp.start; t < sp.start + sp.len; t++) {
                    RealP dv = r[t] * scale[t];  // change of the increment, dt^2 * acceleration
                    vw[t] += dv;
                    w[t] += vw[t];
                    keep[t] = dv;  // what radiates; picked up by the next step's radiation task
                }
            }
        }
    } else if (phase == 0) {
        for (int i = c.from; i < c.to; i++) stepString(strings_[i]);
    } else if (phase == 1) {
        const int vb = vb_;
        for (int i = c.from; i < c.to; i++) {
            const StringRT& s = strings_[i];
            if (!s.awake) continue;
            RealS* __restrict u = &u_[s.off];
            const RealS* __restrict v = &v_[vb][s.off];
            for (int l = 1; l < s.N; l++) u[l] += v[l];
        }
    }
}

// Starts a pass over all chunks. With helpers active the caller may do other work before
// calling finishChunks(); without them the pass runs here and now.
void Piano::runChunks(int phase) {
    Pool* p = pool_.get();
    const int n = (int)phaseChunks_[phase].size();
    if (!p || !p->active.load(std::memory_order_relaxed)) {
        for (int c = 0; c < n; c++) chunkWork(phase, c);
        return;
    }
    p->post(phase);
}

Piano::Piano(const PianoDesign& d, const PianoOptions& o) {
    os_ = std::max(1, o.oversample);
    fs_ = o.sampleRate * os_;
    k_ = 1.0 / fs_;
    outGain_ = o.outputGain;
    const double k = k_;
    std::fill(noteOfMidi_, noteOfMidi_ + 128, -1);
    dampMax_ = 1500.0 * d.damperStrength;
    neverSleep_ = d.dampedStringsRing;
    mechNoise_ = d.mechanismNoise;

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
        n.thump = nd.keyThump;
        auto buildString = [&](const StringDesign& sd, bool duplex) -> bool {
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
            if (N < 3) {
                if (duplex) return false;  // too short for the grid: its pitch is beyond hearing anyway
                throw std::runtime_error("string cannot be resolved at this sample rate");
            }
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
            if (s.hasDamper) s.pressK = k * k * d.damperPress * d.damperStrength / (s.dmp1 - s.dmp0) / s.mNode;

            // Longitudinal grid: the coarsest whole number of transverse segments per cell that
            // keeps the (fast) longitudinal wave stable. Short strings get a single cell, which
            // is the quasi-static limit: the tension follows the mean squared slope.
            if (d.longitudinal && sd.EA > 0) {
                double cL = std::sqrt(sd.EA / sd.rhoA);
                int span = std::max(1, (int)std::ceil(cL * k * 1.03 / h));
                int cells = std::max(1, std::min((int)StringRT::kLong, N / span));
                if (cells == 1) span = N;
                else span = std::max(span, N / cells);  // the last cell takes any remainder, so it is the longest
                s.lCells = cells; s.lSpan = span; s.lEA = sd.EA;
                double H = span * h;
                s.lA = sd.EA * k * k / (sd.rhoA * H * H);
                s.lDamp = 1.0 / (1.0 + 30.0 * k);  // longitudinal modes ring for a few tenths of a second
                s.lBridge = 2 * std::sin(d.downbearingDeg * M_PI / 180.0);
                s.lOff = (int)xi_.size();
                xi_.resize(xi_.size() + StringRT::kLong + 1, 0.0);
                vxi_.resize(xi_.size(), 0.0);
            }
            {
                const double inv = 1 / (1 + s.s0k);
                s.cu0 = (RealS)((-2 * s.aT - 6 * s.aB) * inv); s.cu1 = (RealS)((s.aT + 4 * s.aB) * inv); s.cu2 = (RealS)(-s.aB * inv);
                s.cv0 = (RealS)((1 - s.s0k - 2 * s.aD) * inv); s.cv1 = (RealS)(s.aD * inv);
            }
            s.bridge = sd.bridge;
            s.off = offset + 1;
            offset += N + 3;
            maxN = std::max(maxN, N);
            if (duplex) { s.hasDamper = false; s.dampRate = s.dampTarget = 0; s.lCells = 0; s.hamW = 0; }
            strings_.push_back(s);
            return true;
        };
        for (const StringDesign& sd : nd.strings) buildString(sd, false);
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
                t.lCells = 0;  // stretching is computed once, on the upright motion
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
        for (const StringDesign& sd : nd.duplex)
            if (buildString(sd, true)) {
                const StringRT& s = strings_.back();
                if (s.attached) att.push_back({sd.bridge, 0.5 * s.mNode, s.z0});
                endScale.push_back(1.0);
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
            s.awake = !s.hasDamper || !s.attached || d.dampedStringsRing;
        }
    }
    w_.assign(P.gridSize, 0); vw_.assign(P.gridSize, 0); r_.assign(P.gridSize, 0);
    radIn_.assign(P.gridSize, 0);
    chunks_.push_back({0, 0, 0});
    {
        int start = 0, nodes = 0;
        const int per = std::max(200, P.n / 3);
        for (int i = 0; i < (int)P.spans.size(); i++) {
            nodes += P.spans[i].len;
            if (nodes >= per || i + 1 == (int)P.spans.size()) { chunks_.push_back({1, start, i + 1}); start = i + 1; nodes = 0; }
        }
        start = 0; nodes = 0;
        for (int i = 0; i < (int)strings_.size(); i++) {
            nodes += strings_[i].N;
            if (nodes >= std::max(400, offset / 8) || i + 1 == (int)strings_.size()) { chunks_.push_back({2, start, i + 1}); start = i + 1; nodes = 0; }
        }
        // Each phase lists only the chunks that have work in it: every claim costs a round
        // trip of a shared cache line between cores, so there must be few of them.
        for (int c = 0; c < (int)chunks_.size(); c++) {
            phaseChunks_[0].push_back(c);
            if (chunks_[c].kind == 2) phaseChunks_[1].push_back(c);
            if (chunks_[c].kind == 1) phaseChunks_[2].push_back(c);
        }
    }

    for (size_t i = 0; i < notes_.size(); i++) notes_[i].thumpAt = P.interp(d.notes[i].keyThumpAt);
    pedalAt_ = P.interp({0.70, 0.06});  // the pedal work hangs under the middle of the keybed
    radiation_.build(P, d, fs_);
    airNow_ = radiation_.airForce();

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
    s.lOn = false; s.lPeak = 0; s.tensionChange = 0;
    if (s.lCells > 0) {
        std::fill(&xi_[s.lOff], &xi_[s.lOff] + StringRT::kLong + 1, 0.0);
        std::fill(&vxi_[s.lOff], &vxi_[s.lOff] + StringRT::kLong + 1, 0.0);
    }
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
        if (s.lCells > 0) { s.lOn = true; s.lPeak = 1; }
    }
    if (!(speed > 0)) return;
    knock(n.thumpAt, n.thump * speed, 0.003);
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

// A short knock on the case: a half-sine force pulse carrying the given impulse.
void Piano::knock(const Interp& at, double impulse, double seconds) {
    if (!(impulse > 0) || pulseCount_ >= kPulses) return;
    int len = std::max(2, (int)(seconds * fs_));
    pulses_[pulseCount_++] = {at, impulse * M_PI / (2 * len * k_), 0, len};
}

void Piano::noteOff(int midi, double releaseSpeed) {
    int ni = noteOfMidi_[midi & 127];
    if (ni < 0) return;
    NoteRT& n = notes_[ni];
    if (!n.keyDown) return;
    n.keyDown = false;
    releaseSpeed = std::clamp(releaseSpeed, 0.25, 4.0);
    for (int i = 0; i < n.total; i++) {
        StringRT& s = strings_[n.first + i];
        if (s.hasDamper) { s.dampTarget = dampMax_ * pedalDamping_; s.dampLand = releaseSpeed; }
    }
    // The key landing back on its rest, and the hammer on its own: a softer knock than the
    // one going down. Speed here is the hammer speed that an ordinary release corresponds to.
    knock(n.thumpAt, 0.3 * n.thump * mechNoise_ * releaseSpeed, 0.004);
}

void Piano::setPedal(double position) {
    // Dampers start to leave the strings around 34 % of the pedal travel and are clear by
    // 47 %; in between the felt only grazes the string, modelled as a weaker damper. The
    // range is inferred from recorded performances: players release a bass key with the pedal
    // near 50 % on its way down and expect the note to be caught, and dip to about 40 % to
    // clear the previous harmony.
    double x = std::clamp((position - 0.34) / 0.13, 0.0, 1.0);
    double damping = 1.0 - x * x * (3 - 2 * x);
    // Pedal work: its speed drives a faint friction noise, and it knocks at each end of its travel.
    {
        double seconds = std::max(1e-3, (stepCount_ - pedalStep_) * k_);
        double speed = std::fabs(position - pedalPos_) / seconds;  // travels per second
        if (speed > 0) {
            pedalSpeed_ = std::min(speed, 30.0);
            pedalHiss_ = std::max(pedalHiss_, std::min(1.5, pedalSpeed_ / 6.0));
            bool bottom = position >= 0.97 && pedalPos_ < 0.97, top = position <= 0.03 && pedalPos_ > 0.03;
            if (bottom || top) knock(pedalAt_, 0.0015 * mechNoise_ * pedalSpeed_ * (top ? 0.7 : 1.0), 0.006);
        }
        pedalPos_ = position;
        pedalStep_ = stepCount_;
    }
    if (damping == pedalDamping_) return;
    pedalDamping_ = damping;
    for (NoteRT& n : notes_)
        for (int i = 0; i < n.total; i++) {
            StringRT& s = strings_[n.first + i];
            if (!s.hasDamper) continue;
            if (damping < 1.0) wake(s);
            if (!n.keyDown) { s.dampTarget = dampMax_ * damping; s.dampLand = 1; }
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
        s.endForce = -k_ * s.z0 * vN;
        return;
    }
    // Both ends are simply supported: zero bending moment, expressed through ghost nodes.
    u[N] = (RealS)uN; v[N] = (RealS)vN;
    u[-1] = -u[1];
    u[N + 1] = (RealS)(2 * uN - (double)u[N - 1]);

    if (s.attached) {
        double uA = u[N - 1], uB = u[N - 2], vA = v[N - 1];
        s.endForce = s.mNode * (-(s.aT * (uN - uA) + s.aD * (vN - vA)) - s.aB * (uB - 2 * uA + uN));
    }
    const RealS cu0 = s.cu0, cu1 = s.cu1, cu2 = s.cu2, cv0 = s.cv0, cv1 = s.cv1;
    for (int l = 1; l < N; l++)
        vn[l] = cv0 * v[l] + cv1 * (v[l - 1] + v[l + 1]) + cu0 * u[l] + cu1 * (u[l - 1] + u[l + 1]) + cu2 * (u[l - 2] + u[l + 2]);
    vn[0] = 0;

    // Stretching. The strain of a cell is its end-to-end stretch plus the extra length the
    // transverse wave takes up, half the mean squared slope. Tension follows the strain; where
    // it differs between cells the string is pulled along its length. This is one-way: the
    // tension change is not fed back into the transverse motion.
    s.tensionChange = 0;
    if (s.lOn) {
        const int C = s.lCells, span = s.lSpan;
        double* __restrict xi = &xi_[s.lOff];
        double* __restrict vxi = &vxi_[s.lOff];
        const double invh2 = 1.0 / (s.h * s.h);
        double strain[StringRT::kLong];
        double peak = 0;
        for (int c = 0, l = 0; c < C; c++) {
            int end = c == C - 1 ? N : l + span;
            double sq = 0;
            for (int j = l; j < end; j++) { double dq = (double)u[j + 1] - (double)u[j]; sq += dq * dq; }
            double H = (end - l) * s.h;
            strain[c] = (xi[c + 1] - xi[c]) / H + 0.5 * sq * invh2 / (end - l);
            peak = std::max(peak, std::fabs(strain[c]));
            l = end;
        }
        const double H = span * s.h;
        const double lastScale = 2.0 * span / (span + (N - (C - 1) * span));  // the node next to the longer last cell is heavier
        for (int c = 1; c < C; c++) {
            double acc = s.lA * H * (strain[c] - strain[c - 1]);
            if (c == C - 1) acc *= lastScale;
            vxi[c] = (vxi[c] + acc) * s.lDamp;
            xi[c] += vxi[c];
        }
        s.tensionChange = s.lEA * strain[C - 1];
        s.lPeak = std::max(peak, 0.999 * s.lPeak);
        if (s.lPeak < 2e-9) {  // no longer stretching to any effect: stop until it is struck again
            s.lOn = false;
            std::fill(xi, xi + StringRT::kLong + 1, 0.0);
            std::fill(vxi, vxi + StringRT::kLong + 1, 0.0);
        }
        if (s.attached) s.endForce += k_ * k_ * s.lBridge * s.tensionChange;
    }

    // Damper felt: viscous, backward Euler so that it is stable at any strength.
    if (s.dampRate != s.dampTarget) {
        // Felt takes a few milliseconds to seat or to let go.
        double rate = s.dampTarget > s.dampRate ? 0.004 * s.dampLand : 0.006;
        s.dampRate += (s.dampTarget - s.dampRate) * rate * (48000.0 * k_);
        if (std::fabs(s.dampRate - s.dampTarget) < 1e-3) s.dampRate = s.dampTarget;
    }
    if (s.dampRate > 0) {
        // The damper rests on the string (toward the board) and soaks up its motion.
        RealS scale = (RealS)((1 + s.s0k) / (1 + s.s0k + 2 * k_ * s.dampRate));
        RealS press = (RealS)(s.pressK * s.dampRate / dampMax_);
        for (int l = s.dmp0; l < s.dmp1; l++) vn[l] = (vn[l] - press) * scale;
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
    // is put to sleep. From then on the bridge sees it as a plain dashpot. "On its own" matters:
    // while the board still rings it keeps shaking the damped string by about the bridge's own
    // displacement, and that forced motion is exactly what the dashpot stands for.
    const int settle = (int)(0.15 * fs_ / 64);
    for (StringRT& s : strings_) {
        if (!s.awake || !s.hasDamper || !s.attached) continue;
        if (s.dampRate < dampMax_ || notes_[s.note].hammerActive) { s.dampedSteps = 0; continue; }
        if (++s.dampedSteps < settle) continue;
        const RealS* u = &u_[s.off];
        double uN = u[s.N], m = 0;
        s.endPeak = std::max(std::fabs(uN), 0.97 * s.endPeak);
        for (int l = 1; l < s.N; l++) m = std::max(m, std::fabs((double)u[l] - uN * l / s.N));
        if (m < 1e-8 + 3 * s.endPeak && !neverSleep_) sleep(s);
    }
    // Bring the helpers in only when there is enough string work to share.
    // Helpers are worth calling in whenever anything is sounding: the plate work is shared too.
    if (pool_) {
        bool sounding = pulseCount_ > 0;
        for (const StringRT& s : strings_) sounding = sounding || (s.awake && s.hasDamper);
        if (sounding) quietChecks_ = 0; else quietChecks_++;
        pool_->active.store(quietChecks_ < 1500, std::memory_order_release);  // about 2 s of ring-out
    }
}

void Piano::step(double& pL, double& pR) {
    const Plate& P = *plate_;
    RealP* __restrict r = r_.data();
    RealP* __restrict w = w_.data();
    RealP* __restrict vw = vw_.data();

    Pool* pool = pool_.get();
    const bool shared = pool && pool->active.load(std::memory_order_relaxed);
    // Finish a shared pass: take whatever chunks are left, then wait for the helpers' ones.
    auto finish = [&](int phase) {
        if (!shared) return;
        int ph, c;
        while (pool->claim(ph, c)) {
            chunkWork(ph, c);
            pool->done.fetch_add(1, std::memory_order_release);
        }
        const int nChunks = (int)phaseChunks_[phase].size();
        while (pool->done.load(std::memory_order_acquire) < nChunks) {
#if defined(__aarch64__)
            __asm__ volatile("yield");
#endif
        }
    };

    runChunks(0);  // strings, plate stencil and last step's sound, side by side
    finish(0);
    P.applyExtras(w, r);
    if (!airNow_.empty()) airNow_ = radiation_.airForce();  // same size: a plain copy, no allocation
    for (int i = 0; i < pulseCount_;) {
        Pulse& pu = pulses_[i];
        double f = pu.peak * std::sin(M_PI * (pu.step + 0.5) / pu.length) * k_ * k_;
        for (int q = 0; q < 4; q++) r[pu.at.idx[q]] += (RealP)(pu.at.w[q] * f);
        if (++pu.step >= pu.length) pulses_[i] = pulses_[--pulseCount_]; else i++;
    }
    if (pedalHiss_ > 1e-4) {  // friction in the pedal work: a faint, quickly fading rustle on the case
        rng_ = rng_ * 1664525u + 1013904223u;
        double noise = ((rng_ >> 8) * (1.0 / 8388608.0) - 1.0) * 0.02 * mechNoise_ * pedalHiss_;
        for (int q = 0; q < 4; q++) r[pedalAt_.idx[q]] += (RealP)(pedalAt_.w[q] * noise * k_ * k_);
        pedalHiss_ *= 1.0 - 30.0 * k_;
    }
    if (extF_ != 0) {
        for (int q = 0; q < 4; q++) r[extAt_.idx[q]] += (RealP)(extAt_.w[q] * extF_ * k_ * k_);
        extF_ = 0;
    }
    for (const StringRT& s : strings_)
        if (s.attached && s.endForce != 0)
            for (int q = 0; q < 4; q++) r[s.at.idx[q]] += (RealP)(s.at.w[q] * s.endForce);
    for (NoteRT& n : notes_)
        if (n.hammerActive) stepHammer(n);
    vb_ ^= 1;
    runChunks(1);  // advance the string displacements while this thread solves for the stiffeners
    P.solveStiff(r);
    finish(1);
    runChunks(2);  // advance the plate
    finish(2);
    pL = radL_; pR = radR_;
    (void)vw;
    if ((++stepCount_ & 63) == 0) housekeeping();
}

void Piano::process(float* left, float* right, int n) {
    FlushDenormals flush;
    // Call the helpers in for the duration of this block.
    struct Block {
        Pool* p;
        explicit Block(Pool* pool) : p(pool && pool->active.load(std::memory_order_relaxed) ? pool : nullptr) {
            if (p) { p->inBlock.store(true, std::memory_order_release); p->cv.notify_all(); }
        }
        ~Block() { if (p) p->inBlock.store(false, std::memory_order_release); }
    } block(pool_.get());
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
    return {notes_[s.note].midi, s.N + 1, s.h, &u_[s.off], s.force, s.awake, s.tension, s.tensionChange, s.lOn ? s.lCells : 0};
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
