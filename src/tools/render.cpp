// Offline renderer: plays a small score through the model and writes a WAV file.
//   render <out.wav> [score] [oversample] [seconds]
// Scores: "c4" (one mezzo forte note), "phrase" (a short two-hand phrase with pedal),
// or the path of a MIDI file.
#include "Midi.h"
#include "Piano.h"
#include "Util.h"
#include <algorithm>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

using namespace pp;

struct Event { double t; int type; int midi; int vel; };  // type 0 off, 1 on, 2 pedal (vel = down)

static std::vector<Event> phrase() {
    std::vector<Event> e;
    auto note = [&](double t, double len, int midi, int vel) {
        e.push_back({t, 1, midi, vel});
        e.push_back({t + len, 0, midi, 0});
    };
    double q = 0.42;
    // Slow broken chords, C major to A minor to F to G, then a held cadence.
    int bass[] = {36, 33, 29, 31};
    int chords[4][4] = {{48, 55, 64, 72}, {45, 52, 60, 69}, {41, 53, 60, 65}, {43, 50, 59, 67}};
    int melody[4][2] = {{76, 79}, {77, 76}, {72, 74}, {71, 74}};
    for (int bar = 0; bar < 4; bar++) {
        double t = bar * 4 * q;
        e.push_back({t + 0.02, 2, 0, 1});
        note(t, 3.8 * q, bass[bar], 70);
        for (int i = 0; i < 4; i++) note(t + i * q, 1.6 * q, chords[bar][i], 54 + 6 * i);
        note(t + 2 * q, 1.0 * q, melody[bar][0], 82);
        note(t + 3 * q, 0.9 * q, melody[bar][1], 74);
        e.push_back({t + 3.9 * q, 2, 0, 0});
    }
    double t = 16 * q;
    e.push_back({t + 0.02, 2, 0, 1});
    for (int m : {24, 36, 48, 52, 55, 60, 64, 72}) note(t, 5.0, m, m < 40 ? 88 : 76);
    e.push_back({t + 5.2, 2, 0, 0});
    return e;
}

int main(int argc, char** argv) {
    std::string out = argc > 1 ? argv[1] : "out/render.wav";
    std::string score = argc > 2 ? argv[2] : "phrase";
    int os = argc > 3 ? std::atoi(argv[3]) : 1;
    std::vector<Event> ev;
    double seconds;
    GrandOptions g;
    if (score == "c4") {
        g.firstKey = g.lastKey = 40;
        ev = {{0.05, 1, 60, 80}, {4.0, 0, 60, 0}};
        seconds = 5.0;
    } else if (score.size() > 4 && score.substr(score.size() - 4) == ".mid") {
        for (const MidiEvent& m : readMidiFile(score)) ev.push_back({m.seconds, m.type, m.type == 2 ? 0 : m.a, m.type == 2 ? m.a : m.b});
        if (ev.empty()) { std::fprintf(stderr, "no notes in %s\n", score.c_str()); return 1; }
        seconds = ev.back().t + 4.0;
    } else {
        ev = phrase();
        seconds = 13.5;
    }
    if (argc > 4) seconds = std::atof(argv[4]);

    const int fs = 48000, block = 64;
    Piano piano(makeGrand(g), {(double)fs, os});
    piano.setHelperThreads(3);
    std::printf("%d strings (%d nodes), board %d nodes at %.1f x %.1f mm, internal rate %d Hz\n", piano.stringCount(),
                piano.totalStringNodes(), piano.plate().n, piano.plate().hx * 1e3, piano.plate().hy * 1e3, fs * os);

    std::sort(ev.begin(), ev.end(), [](const Event& a, const Event& b) { return a.t < b.t; });
    std::vector<float> l(block), r(block), wav;
    size_t next = 0;
    double peak = 0;
    int maxAwake = 0;
    auto t0 = std::chrono::steady_clock::now();
    for (int frame = 0; frame < (int)(seconds * fs); frame += block) {
        while (next < ev.size() && ev[next].t * fs <= frame) {
            const Event& e = ev[next++];
            if (e.type == 1) piano.noteOn(e.midi, midiVelocityToHammerSpeed(e.vel));
            else if (e.type == 0) piano.noteOff(e.midi);
            else piano.setSustain(e.vel != 0);
        }
        piano.process(l.data(), r.data(), block);
        maxAwake = std::max(maxAwake, piano.awakeStrings());
        for (int i = 0; i < block; i++) {
            peak = std::max(peak, (double)std::max(std::fabs(l[i]), std::fabs(r[i])));
            wav.push_back(l[i]);
            wav.push_back(r[i]);
        }
    }
    double wall = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
    // Normalise only if the render would clip; report the gain so levels stay traceable.
    double norm = peak > 0.98 ? 0.98 / peak : 1.0;
    for (float& x : wav) x = (float)(x * norm);
    system(("mkdir -p \"$(dirname '" + out + "')\"").c_str());
    if (!writeWavFloat(out, wav, 2, fs)) { std::fprintf(stderr, "cannot write %s\n", out.c_str()); return 1; }
    std::printf("wrote %s: %.1f s, peak %.3f (scaled by %.3f), up to %d strings awake, %.2fx realtime\n", out.c_str(),
                seconds, peak, norm, maxAwake, seconds / wall);
    return 0;
}
