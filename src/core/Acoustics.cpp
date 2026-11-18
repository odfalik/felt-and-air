#include "Acoustics.h"
#include <algorithm>
#include <cmath>

namespace pp {

void Radiation::Bins::build(const std::vector<int>& delay, const std::vector<int>& source, const std::vector<double>& g) {
    ptr.assign(1, 0); idx.clear(); gain.clear(); first = 0;
    if (delay.empty()) return;
    int lo = *std::min_element(delay.begin(), delay.end()), hi = *std::max_element(delay.begin(), delay.end());
    first = lo;
    ptr.assign(hi - lo + 2, 0);
    for (int d : delay) ptr[d - lo + 1]++;
    for (int b = 0; b < hi - lo + 1; b++) ptr[b + 1] += ptr[b];
    std::vector<int> fill(ptr.begin(), ptr.end() - 1);
    idx.resize(delay.size()); gain.resize(delay.size());
    for (size_t i = 0; i < delay.size(); i++) {
        int q = fill[delay[i] - lo]++;
        idx[q] = source[i];
        gain[q] = (RealP)g[i];
    }
}

void Radiation::build(const Plate& P, const PianoDesign& d, double fs) {
    const RoomDesign& room = d.room;
    const double k = 1.0 / fs, c = d.airC, rho = d.airRho, dS = P.hx * P.hy;
    const double strength = rho * dS / (2 * M_PI) / (k * k);  // half-space monopole, input is dt^2 * accel

    // Patches of about a quarter metre, used for everything but the direct sound. Reflected
    // paths are metres long, so this resolves them up to roughly 700 Hz across a patch; above
    // that the reflections are effectively blurred, as real surfaces blur them.
    std::vector<int> patchOf(P.n);
    std::vector<Vec2> centre;
    {
        const int bi = std::max(1, (int)std::lround(0.25 / P.hx)), bj = std::max(1, (int)std::lround(0.25 / P.hy));
        std::vector<int> id((P.Nx / bi + 1) * (P.Ny / bj + 1), -1), count;
        for (int a = 0; a < P.n; a++) {
            int cell = P.gi[a] / bi + (P.Nx / bi + 1) * (P.gj[a] / bj);
            if (id[cell] < 0) { id[cell] = (int)centre.size(); centre.push_back({0, 0}); count.push_back(0); }
            patchOf[a] = id[cell];
            Vec2 p = P.nodePosition(a);
            centre[id[cell]].x += p.x; centre[id[cell]].y += p.y; count[id[cell]]++;
        }
        for (size_t p = 0; p < centre.size(); p++) { centre[p].x /= count[p]; centre[p].y /= count[p]; }
        patchCount = (int)centre.size();
        patchPtr_.assign(patchCount + 1, 0);
        for (int a = 0; a < P.n; a++) patchPtr_[patchOf[a] + 1]++;
        for (int p = 0; p < patchCount; p++) patchPtr_[p + 1] += patchPtr_[p];
        std::vector<int> fill(patchPtr_.begin(), patchPtr_.end() - 1);
        patchNode_.resize(P.n);
        for (int a = 0; a < P.n; a++) patchNode_[fill[patchOf[a]]++] = P.gid[a];
        patchAcc_.assign(patchCount, 0);
        patchOfGrid_.assign(P.gridSize, 0);
        for (int a = 0; a < P.n; a++) patchOfGrid_[P.gid[a]] = patchOf[a];
    }

    air_ = d.airLoading;
    airForce_.clear();
    if (air_) {
        const int np = patchCount;
        airForce_.assign(np, 0);
        patchVel_.assign(np, 0.0); invCount_.resize(np); selfDelay_.resize(np);
        int far = 1, nearMax = 1;
        mutPtr_.assign(1, 0); mutDelay_.clear(); mutSrc_.clear(); mutGain_.clear();
        for (int p = 0; p < np; p++) {
            int count = patchPtr_[p + 1] - patchPtr_[p];
            invCount_[p] = 1.0 / count;
            double radius = std::sqrt(count * dS / M_PI);
            selfDelay_[p] = std::max(1, (int)std::lround(radius / c * fs));
            nearMax = std::max(nearMax, selfDelay_[p]);
            for (int q = 0; q < np; q++) {
                if (q == p) continue;
                double r = std::hypot(centre[p].x - centre[q].x, centre[p].y - centre[q].y);
                int dl = std::max(1, (int)std::lround(r / c * fs));
                mutDelay_.push_back(dl);
                mutSrc_.push_back(q);
                mutGain_.push_back((float)(-rho * dS * dS / (M_PI * r)));  // both faces; acts on the patch's summed dt^2 * accel
                far = std::max(far, dl);
            }
            mutPtr_.push_back((int)mutDelay_.size());
        }
        selfGain_ = -2 * rho * c * k * dS;  // both faces; acts on the mean increment per step
        int len = 1;
        while (len < far + 2) len <<= 1;
        histMask_ = len - 1; histPos_ = 0;
        int vlen = 1;
        while (vlen < nearMax + 2) vlen <<= 1;
        velMask_ = vlen - 1;
        velHist_.assign((size_t)np * vlen, 0.0f);
        accHist_.assign((size_t)np * len, 0.0f);
    }

    int maxDelay = 0;
    imageCount = 0;
    for (int e = 0; e < 2; e++) {
        // Direct sound, node by node. Delays are rounded to the nearest step (a path error
        // under 4 mm at 48 kHz, well below the grid spacing).
        std::vector<int> delay(P.n), src(P.n);
        std::vector<double> gain(P.n);
        for (int a = 0; a < P.n; a++) {
            Vec2 p = P.nodePosition(a);
            double r = std::sqrt(std::pow(p.x - d.ears[e].x, 2) + std::pow(p.y - d.ears[e].y, 2) + d.ears[e].z * d.ears[e].z);
            delay[a] = (int)std::lround(r / c * fs);
            src[a] = P.gid[a];
            gain[a] = strength / r * (d.ears[e].z >= 0 ? 1.0 : -1.0);
            maxDelay = std::max(maxDelay, delay[a]);
        }
        node_[e].build(delay, src, gain);

        std::vector<int> idelay, isrc;
        std::vector<double> igain;
        auto addImage = [&](double ex, double ey, double ez, double amplitude) {
            // ex, ey, ez: mirrored ear, piano coordinates with z measured from the board.
            // A path that leaves upward comes from the top side, downward from the underside.
            for (int p = 0; p < patchCount; p++) {
                double r = std::sqrt(std::pow(centre[p].x - ex, 2) + std::pow(centre[p].y - ey, 2) + ez * ez);
                idelay.push_back((int)std::lround(r / c * fs));
                isrc.push_back(p);
                igain.push_back(strength / r * amplitude * (ez >= 0 ? 1.0 : -1.0));
                maxDelay = std::max(maxDelay, idelay.back());
            }
            imageCount++;
        };
        if (room.enabled) {
            // Shoebox images of the ear, up to two reflections. Room coordinates put the floor
            // at z = 0; the board sits at boardHeight.
            const double L[3] = {room.Lx, room.Ly, room.Lz};
            const double E[3] = {d.ears[e].x + room.pianoX, d.ears[e].y + room.pianoY, d.ears[e].z + room.boardHeight};
            const double refl = std::sqrt(1.0 - room.absorption);
            for (int nx = -2; nx <= 2; nx++)
                for (int ny = -2; ny <= 2; ny++)
                    for (int nz = -2; nz <= 2; nz++) {
                        int order = std::abs(nx) + std::abs(ny) + std::abs(nz);
                        if (order == 0 || order > 2) continue;
                        const int n[3] = {nx, ny, nz};
                        double im[3];
                        for (int q = 0; q < 3; q++) im[q] = n[q] * L[q] + ((n[q] & 1) ? L[q] - E[q] : E[q]);
                        addImage(im[0] - room.pianoX, im[1] - room.pianoY, im[2] - room.boardHeight, std::pow(refl, order));
                    }
        }
        if (d.lid.open) {
            // The lid is a plane hinged along the bass side, rising toward the treble. Mirror
            // the ear in it; keep the image only for patches whose reflection point is on the lid.
            const double th = d.lid.angleDeg * M_PI / 180.0, nxl = -std::sin(th), nzl = std::cos(th);
            const double ex = d.ears[e].x, ey = d.ears[e].y, ez = d.ears[e].z;
            double dist = nxl * ex + nzl * (ez - d.lid.hingeHeight);  // signed distance of the ear from the lid plane
            if (dist < 0) {  // the ear is on the underside of the lid, where the reflection goes
                double mx = ex - 2 * dist * nxl, mz = ez - 2 * dist * nzl;
                for (int p = 0; p < patchCount; p++) {
                    // Where the straight line from the patch to the mirrored ear crosses the lid.
                    double sd = nxl * centre[p].x + nzl * (0 - d.lid.hingeHeight);
                    double denom = nxl * (mx - centre[p].x) + nzl * (mz - 0);
                    if (std::fabs(denom) < 1e-9) continue;
                    double u = -sd / denom;
                    if (u <= 0 || u >= 1) continue;
                    double hx = centre[p].x + u * (mx - centre[p].x), hy = centre[p].y + u * (ey - centre[p].y);
                    if (hx < 0 || hx > d.lid.width * std::cos(th) || hy < 0 || hy > d.lid.length) continue;
                    double r = std::sqrt(std::pow(centre[p].x - mx, 2) + std::pow(centre[p].y - ey, 2) + mz * mz);
                    idelay.push_back((int)std::lround(r / c * fs));
                    isrc.push_back(p);
                    igain.push_back(strength / r * d.lid.reflection);
                    maxDelay = std::max(maxDelay, idelay.back());
                }
                imageCount++;
            }
        }
        imgDelay_[e] = idelay; imgPatch_[e] = isrc;
        imgGain_[e].assign(igain.begin(), igain.end());
    }
    imageCount /= 2;

    int ringLen = 1;
    while (ringLen < maxDelay + 4) ringLen <<= 1;
    ringMask_ = ringLen - 1;
    ring_[0].assign(ringLen, 0.0);
    ring_[1].assign(ringLen, 0.0);
    ringPos_ = 0;

    // Late field. Sabine: T60 = 0.161 V / A. A steady source of volume acceleration q gives a
    // diffuse field of mean square pressure rho^2 q^2 / (pi A); the network is scaled so that
    // its response to white input has that energy, less the part the mirror images above
    // already carry (everything arriving within about two mean free paths).
    reverb_ = room.enabled;
    reverbTime = 0;
    if (reverb_) {
        const double V = room.Lx * room.Ly * room.Lz, S = 2 * (room.Lx * room.Ly + room.Lx * room.Lz + room.Ly * room.Lz);
        const double A = room.absorption * S;
        reverbTime = 0.161 * V / A;
        const double base[kLines] = {0.0297, 0.0371, 0.0411, 0.0437, 0.0533, 0.0617, 0.0683, 0.0739};
        for (int i = 0; i < kLines; i++) {
            lineLen_[i] = (int)(base[i] * fs) | 1;
            line_[i].assign(lineLen_[i], 0.0);
            linePos_[i] = 0; lineLp_[i] = 0;
            lineGain_[i] = std::pow(10.0, -3.0 * lineLen_[i] / (fs * reverbTime));
        }
        lpCoef_ = 1.0 - std::exp(-2 * M_PI * 6000.0 / fs);  // air and surfaces take the top off the tail
        hpCoef_ = std::exp(-2 * M_PI * 120.0 / fs);          // the two sides of the board cancel at low frequency
        reverbGain_ = 1; feed_ = 1; hpState_ = 0;
        double energy = 0, l, r;
        int steps = (int)(3 * reverbTime * fs);
        for (int n = 0; n < steps; n++) {
            double in = n == 0 ? 1.0 : 0.0;
            // Bypass the high-pass while calibrating: feed the network directly.
            double save = hpCoef_; hpCoef_ = -1;
            reverbTick(in, l, r);
            hpCoef_ = save;
            energy += 0.5 * (l * l + r * r);
        }
        for (int i = 0; i < kLines; i++) { std::fill(line_[i].begin(), line_[i].end(), 0.0); lineLp_[i] = 0; }
        hpState_ = 0;
        const double meanFreePath = 4 * V / S, early = std::exp(-13.8 * (2 * meanFreePath / c) / reverbTime);
        const double target = rho * rho / (M_PI * A) * early;       // per unit volume acceleration squared
        reverbGain_ = std::sqrt(target / energy) * dS / (k * k);    // input is the sum of dt^2 * accel
    }
}

void Radiation::reverbTick(double in, double& l, double& r) {
    double x = in;
    if (hpCoef_ >= 0) {  // one-pole high-pass
        hpState_ = hpCoef_ * hpState_ + (1 - hpCoef_) * in;
        x = in - hpState_;
    }
    double y[kLines], sum = 0;
    for (int i = 0; i < kLines; i++) {
        double out = line_[i][linePos_[i]] * lineGain_[i];
        lineLp_[i] += lpCoef_ * (out - lineLp_[i]);
        y[i] = lineLp_[i];
        sum += y[i];
    }
    sum *= 2.0 / kLines;  // Householder reflection: lossless mixing of the lines
    for (int i = 0; i < kLines; i++) {
        line_[i][linePos_[i]] = y[i] - sum + ((i & 1) ? -x : x);
        if (++linePos_[i] == lineLen_[i]) linePos_[i] = 0;
    }
    l = reverbGain_ * (y[0] - y[1] + y[2] - y[3] + y[4] - y[5] + y[6] - y[7]);
    r = reverbGain_ * (y[0] + y[1] - y[2] - y[3] + y[4] + y[5] - y[6] - y[7]);
}

void Radiation::step(const RealP* __restrict r, double& left, double& right) {
    // Patch sums: total motion of each patch.
    RealP total = 0;
    const int np = patchCount;
    for (int p = 0; p < np; p++) {
        RealP s0 = 0, s1 = 0;
        int q = patchPtr_[p];
        const int end = patchPtr_[p + 1];
        for (; q + 1 < end; q += 2) { s0 += r[patchNode_[q]]; s1 += r[patchNode_[q + 1]]; }
        if (q < end) s0 += r[patchNode_[q]];
        patchAcc_[p] = s0 + s1;
        total += s0 + s1;
    }
    if (air_) {
        const int now = histPos_;
        float* __restrict vh = velHist_.data();
        float* __restrict ah = accHist_.data();
        for (int p = 0; p < np; p++) {
            patchVel_[p] += (double)patchAcc_[p] * invCount_[p];
            vh[(size_t)(now & velMask_) * np + p] = (float)patchVel_[p];
            ah[(size_t)now * np + p] = (float)patchAcc_[p];
        }
        const int* __restrict md = mutDelay_.data();
        const int* __restrict ms = mutSrc_.data();
        const float* __restrict mg = mutGain_.data();
        for (int p = 0; p < np; p++) {
            double f = selfGain_ * (patchVel_[p] - (double)vh[(size_t)((now - selfDelay_[p]) & velMask_) * np + p]);
            float f0 = 0, f1 = 0;
            int q = mutPtr_[p];
            const int end = mutPtr_[p + 1];
            for (; q + 1 < end; q += 2) {
                f0 += mg[q] * ah[(size_t)((now - md[q]) & histMask_) * np + ms[q]];
                f1 += mg[q + 1] * ah[(size_t)((now - md[q + 1]) & histMask_) * np + ms[q + 1]];
            }
            if (q < end) f0 += mg[q] * ah[(size_t)((now - md[q]) & histMask_) * np + ms[q]];
            airForce_[p] = (RealP)(f + (double)(f0 + f1));
        }
        histPos_ = (now + 1) & histMask_;
    }
    const RealP* __restrict pa = patchAcc_.data();
    for (int e = 0; e < 2; e++) {
        double* __restrict ring = ring_[e].data();
        {   // Direct sound: many nodes share each delay, so sum per delay.
            const Bins& b = node_[e];
            const int* __restrict idx = b.idx.data();
            const RealP* __restrict g = b.gain.data();
            const int* __restrict bp = b.ptr.data();
            const int bins = (int)b.ptr.size() - 1, base = ringPos_ + b.first;
            for (int k = 0; k < bins; k++) {
                // Four independent partial sums, so the additions do not wait on each other.
                RealP s0 = 0, s1 = 0, s2 = 0, s3 = 0;
                int q = bp[k];
                const int end = bp[k + 1];
                for (; q + 3 < end; q += 4) {
                    s0 += g[q] * r[idx[q]];
                    s1 += g[q + 1] * r[idx[q + 1]];
                    s2 += g[q + 2] * r[idx[q + 2]];
                    s3 += g[q + 3] * r[idx[q + 3]];
                }
                for (; q < end; q++) s0 += g[q] * r[idx[q]];
                ring[(base + k) & ringMask_] += (double)((s0 + s1) + (s2 + s3));
            }
        }
        {   // Reflections: delays are spread thin, so add each one where it lands.
            const int n = (int)imgDelay_[e].size();
            const int* __restrict dl = imgDelay_[e].data();
            const int* __restrict src = imgPatch_[e].data();
            const RealP* __restrict g = imgGain_[e].data();
            for (int q = 0; q < n; q++) ring[(ringPos_ + dl[q]) & ringMask_] += (double)(g[q] * pa[src[q]]);
        }
    }
    left = ring_[0][ringPos_]; ring_[0][ringPos_] = 0;
    right = ring_[1][ringPos_]; ring_[1][ringPos_] = 0;
    ringPos_ = (ringPos_ + 1) & ringMask_;
    if (reverb_) {
        double l, rr;
        reverbTick((double)total, l, rr);
        left += l; right += rr;
    }
}

}  // namespace pp
