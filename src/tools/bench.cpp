// Realtime budget measurement. Runs the engine block by block, the way an audio callback
// would, and reports how much of each block's deadline the physics uses.
//   bench [oversample] [seconds per scenario] [block size]
// Numbers are per block, as a share of the block's real duration. "cpu" is processor time
// actually spent in the engine; "wall" also counts time the thread was kept waiting by other
// work on the machine, which is what an audio callback would experience.
#include "Piano.h"
#include <algorithm>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <libproc.h>
#include <pthread.h>
#include <unistd.h>
#include <time.h>
#include <vector>

using namespace pp;

struct Stats { double minv, median, p99, maxv, mean; };

static Stats summarize(std::vector<double> v) {
    std::sort(v.begin(), v.end());
    double sum = 0;
    for (double x : v) sum += x;
    return {v.front(), v[v.size() / 2], v[(size_t)(v.size() * 0.99)], v.back(), sum / v.size()};
}

int main(int argc, char** argv) {
    int os = argc > 1 ? std::atoi(argv[1]) : 1;
    double seconds = argc > 2 ? std::atof(argv[2]) : 4.0;
    int block = argc > 3 ? std::atoi(argv[3]) : 128;
    double gridScale = argc > 4 ? std::atof(argv[4]) : 1.0;
    int helpers = argc > 5 ? std::atoi(argv[5]) : 0;
    const double fs = 48000;
    pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);  // ask for a performance core

    PianoOptions opt;
    opt.sampleRate = fs; opt.oversample = os; opt.boardGridScale = gridScale;
    Piano piano(makeGrand(), opt);
    piano.setHelperThreads(helpers);
    std::printf("helper threads: %d (cycle counts below include them)\n", helpers);
    std::printf("engine: %d strings / %d string nodes, board %d nodes (%.1f x %.1f mm), internal rate %.0f Hz, block %d (%.2f ms)\n",
                piano.stringCount(), piano.totalStringNodes(), piano.plate().n, piano.plate().hx * 1e3, piano.plate().hy * 1e3, fs * os, block,
                1e3 * block / fs);
    double la[3] = {0, 0, 0};
    getloadavg(la, 3);
    std::printf("machine load average right now: %.1f (above ~4 means this run is competing for cores)\n", la[0]);
    std::printf("%-34s %8s %8s %8s | %8s %8s %8s | %9s %9s  %s\n", "scenario", "cpu min", "cpu med", "cpu p99", "wall med", "wall p99",
                "wall max", "instr/stp", "cycle/stp", "awake");

    std::vector<float> l(block), r(block);
    const double deadline = block / fs;
    int blocks = (int)(seconds * fs / block);
    auto run = [&](const char* name, auto&& events) {
        std::vector<double> wall, cpu;
        int awake = 0;
        // Retired instructions and cycles do not depend on how busy the machine is.
        rusage_info_v4 ru0{}, ru1{};
        proc_pid_rusage(getpid(), RUSAGE_INFO_V4, (rusage_info_t*)&ru0);
        auto cpuNow = [] { timespec ts; clock_gettime(CLOCK_THREAD_CPUTIME_ID, &ts); return ts.tv_sec + 1e-9 * ts.tv_nsec; };
        for (int b = 0; b < blocks; b++) {
            events(b);
            auto t0 = std::chrono::steady_clock::now();
            double c0 = cpuNow();
            piano.process(l.data(), r.data(), block);
            double c1 = cpuNow();
            double t = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
            wall.push_back(t / deadline);
            cpu.push_back((c1 - c0) / deadline);
            awake = std::max(awake, piano.awakeStrings());
        }
        proc_pid_rusage(getpid(), RUSAGE_INFO_V4, (rusage_info_t*)&ru1);
        double steps = (double)blocks * block * os;
        Stats w = summarize(wall), c = summarize(cpu);
        std::printf("%-34s %7.1f%% %7.1f%% %7.1f%% | %7.1f%% %7.1f%% %7.1f%% | %9.0f %9.0f  %d\n", name, 100 * c.minv, 100 * c.median,
                    100 * c.p99, 100 * w.median, 100 * w.p99, 100 * w.maxv, (ru1.ri_instructions - ru0.ri_instructions) / steps,
                    (ru1.ri_cycles - ru0.ri_cycles) / steps, awake);
    };
    auto settle = [&] {
        piano.setSustain(false);
        for (int m = 21; m <= 108; m++) piano.noteOff(m);
        for (int b = 0; b < (int)(1.5 * fs / block); b++) piano.process(l.data(), r.data(), block);
    };

    run("idle (board + undamped treble)", [&](int) {});
    run("one note held (C4)", [&](int b) { if (b == 0) piano.noteOn(60, 2.5); });
    settle();
    run("six note chord held", [&](int b) {
        if (b == 0) for (int m : {36, 48, 55, 60, 64, 72}) piano.noteOn(m, 2.5);
    });
    settle();
    run("fast playing, no pedal", [&](int b) {
        int period = std::max(1, (int)(0.08 * fs / block));  // a note every 80 ms
        if (b % period == 0) {
            int i = b / period;
            piano.noteOff(36 + (i * 7 + 60 - 7) % 60);
            piano.noteOn(36 + (i * 7) % 60, 3.0);
        }
    });
    settle();
    run("pedal down, all strings ringing", [&](int b) {
        if (b == 0) piano.setSustain(true);
        int period = std::max(1, (int)(0.1 * fs / block));
        if (b % period == 0) piano.noteOn(30 + (b / period * 11) % 70, 4.0);
    });
    std::printf("budget: one step may take %.0f cycles of a 4.05 GHz performance core at 100%% load\n", 4.05e9 / (fs * os));
    return 0;
}
