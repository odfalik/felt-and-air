#include "Design.h"
#include <algorithm>
#include <cmath>

namespace pp {

namespace {
double lerp(double a, double b, double t) { return a + (b - a) * t; }

// Piecewise linear through three anchor keys (C2, C4, C7), linear extrapolation outside.
double anchors(int key, double c2, double c4, double c7) {
    if (key <= 40) return lerp(c2, c4, (key - 16) / 24.0);
    return lerp(c4, c7, (key - 40) / 36.0);
}
}  // namespace

double midiVelocityToHammerSpeed(int velocity) {
    double v = std::clamp(velocity, 1, 127) / 127.0;
    return 0.25 + 5.5 * std::pow(v, 1.7);
}

PianoDesign makeGrand(const GrandOptions& o) {
    PianoDesign d;
    BoardDesign& b = d.board;
    b.outline = {{0, 0},       {1.40, 0},    {1.40, 0.42}, {1.33, 0.62}, {1.20, 0.82},
                 {1.02, 1.02}, {0.82, 1.22}, {0.62, 1.38}, {0.45, 1.50}, {0, 1.50}};
    b.grainAngle = 130.0 * M_PI / 180.0;  // roughly parallel to the long bridge

    const double steelE = 2.0e11, steelRho = 7850;
    const double agraffeY = 0.03;

    auto keyX = [](int key) { return 0.09 + (key - 1) / 87.0 * 1.22; };
    auto plainLength = [](int key) {
        double l = 0.052 * std::pow(1.858, (88 - key) / 12.0);
        const double cap = 1.20;  // case length limits the low tenor
        return std::pow(std::pow(l, -6.0) + std::pow(cap, -6.0), -1.0 / 6.0);
    };
    auto bridgePoint = [&](int key) -> Vec2 {
        if (key <= 26) {  // overstrung bass, separate bridge near the tail
            double t = (key - 1) / 25.0;
            return {lerp(0.22, 0.50, t), lerp(1.40, 1.30, t)};
        }
        return {keyX(key), std::max(0.075, agraffeY + plainLength(key))};
    };

    // Bridges follow the string terminations of the full compass, whatever subset is built.
    std::vector<Vec2> bass, treble;
    for (int key = 1; key <= 26; key++) bass.push_back(bridgePoint(key));
    for (int key = 27; key <= 88; key++) treble.push_back(bridgePoint(key));
    b.bridges = {bass, treble};

    // The wire of one key, before tuning: length, mass and stiffness follow from the scale
    // design at the nominal pitch.
    auto equal = [](int key) { return 440.0 * std::pow(2.0, (key - 49) / 12.0); };
    auto wire = [&](int key) {
        StringDesign s;
        double f0 = equal(key);
        if (key <= 26) {  // copper wound on a steel core; only the core carries bending
            double t = (26 - key) / 25.0;
            s.L = lerp(1.12, 1.42, t);
            s.T = lerp(750, 1000, t);
            s.rhoA = s.T / std::pow(2 * s.L * f0, 2);
            double core = lerp(1.0e-3, 1.5e-3, t);
            s.EI = steelE * M_PI * std::pow(core, 4) / 64.0;
        } else {
            s.L = plainLength(key);
            s.T = 720;
            s.rhoA = s.T / std::pow(2 * s.L * f0, 2);
            double dia = std::sqrt(4 * s.rhoA / (M_PI * steelRho));
            s.EI = steelE * M_PI * std::pow(dia, 4) / 64.0;
        }
        return s;
    };

    // Tuning. Stiff strings have sharp partials, so octaves that sound in tune are wider than
    // 2:1. Tune as a tuner does: an equal tempered octave around A4, then octaves outward by
    // matching partials (4:2 in the middle, 2:1 in the high treble, 6:3 in the bass).
    std::vector<double> tuned(89);
    {
        std::vector<double> B(89);
        for (int key = 1; key <= 88; key++) {
            StringDesign s = wire(key);
            B[key] = M_PI * M_PI * s.EI / (s.T * s.L * s.L);
            tuned[key] = equal(key);
        }
        if (o.stretchTuning) {
            // Partial k of a string whose first partial is f1.
            auto partial = [&](int key, int k, double f1) { return k * f1 * std::sqrt((1 + B[key] * k * k) / (1 + B[key])); };
            // First partial of `key` such that its partial a equals partial b of `ref`.
            auto match = [&](int key, int a, int ref, int b) { return partial(ref, b, tuned[ref]) / partial(key, a, 1.0); };
            tuned[37] = match(37, 4, 49, 2);
            for (int key = 38; key < 49; key++) tuned[key] = tuned[37] * std::pow(tuned[49] / tuned[37], (key - 37) / 12.0);
            for (int key = 50; key <= 88; key++) tuned[key] = key <= 64 ? match(key, 2, key - 12, 4) : match(key, 1, key - 12, 2);
            for (int key = 36; key >= 1; key--) tuned[key] = key >= 28 ? match(key, 4, key + 12, 2) : match(key, 6, key + 12, 3);
        }
    }

    for (int key = std::max(1, o.firstKey); key <= std::min(88, o.lastKey); key++) {
        NoteDesign n;
        n.midi = keyToMidi(key);
        double f0 = tuned[key];
        int count = key <= 10 ? 1 : key <= 26 ? 2 : 3;

        StringDesign s = wire(key);
        s.T *= std::pow(f0 / equal(key), 2);  // the tension that gives the tuned pitch
        // Loss law of Chaigne & Askenfelt (1994): rate = b1 + b3 * omega^2.
        double b3 = key <= 40 ? 6.25e-9 : 6.25e-9 * std::pow(1.0e-9 / 6.25e-9, (key - 40) / 48.0);
        // Their constant term (0.5 /s) was fitted with a rigid bridge, so it contains the energy
        // the soundboard takes. Here the board takes that energy itself, and only the string's
        // own loss (air and internal friction) belongs in the constant. 0.12 /s is an estimate.
        s.sigma0 = 0.12;
        s.sigma1 = b3 * s.T / s.rhoA;

        Vec2 bp = bridgePoint(key);
        for (int i = 0; i < count; i++) {
            StringDesign si = s;
            double offset = count == 1 ? 0.0 : (i - (count - 1) / 2.0);
            // Uneven spread, so that no two strings of a unison are mirror images.
            double cents = o.unisonDetuneCents * (offset + 0.15 * offset * offset);
            si.f0 = f0 * std::pow(2.0, cents / 1200.0);
            si.bridge = {bp.x + 0.006 * offset, bp.y};
            // Alternate the sign from string to string so unisons do not all lean one way.
            si.bridgeTilt = o.bridgeTiltDeg * M_PI / 180.0 * (((key + i) & 1) ? 1.0 : -0.7);
            n.strings.push_back(si);
        }

        n.strikeRatio = key <= 50 ? 0.12 : lerp(0.12, 0.075, (key - 50) / 38.0);
        // Chaigne & Askenfelt give per-string equivalent hammers at C2, C4, C7.
        double perStringMass = std::clamp(anchors(key, 4.9e-3, 2.97e-3, 2.2e-3), 2.0e-3, 5.5e-3);
        n.hammerMass = perStringMass * count;
        n.hammerK = std::exp(anchors(key, std::log(4.0e8), std::log(4.5e9), std::log(1.0e11)));
        n.hammerP = std::clamp(anchors(key, 2.3, 2.5, 3.0), 2.2, 3.1);
        n.hammerMu = 0.1;
        n.hasDamper = key <= 69;
        n.hammerTilt = o.hammerTiltDeg * M_PI / 180.0;
        n.twoPolarisations = o.twoPolarisations;
        d.notes.push_back(n);
    }

    d.ears[0] = {0.35, 0.55, 0.9};
    d.ears[1] = {1.05, 0.45, 0.9};
    return d;
}

}  // namespace pp
