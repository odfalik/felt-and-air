// Measurements that characterise the instrument rather than test it.
//   report            bridge driving-point impedance at several notes
#include "Piano.h"
#include "Util.h"
#include <complex>
#include <cstdio>
#include <vector>

using namespace pp;

int main() {
    const double fs = 48000;
    PianoDesign d = makeGrand();
    std::printf("Bridge driving-point impedance |Z| in kg/s (strings asleep), by octave band\n");
    std::printf("%-6s", "note");
    for (double f = 62.5; f < 8000; f *= 2) std::printf(" %6.0f-%-5.0f", f, 2 * f);
    std::printf("\n");
    for (int key : {4, 16, 28, 40, 52, 64, 76, 86}) {
        Piano p(d, {fs, 1});
        Vec2 at = d.notes[key - 1].strings[0].bridge;
        const size_t n = 1 << 16;
        std::vector<std::complex<double>> v(n);
        double a, b;
        p.pushBoard(at, 1.0);  // one step of 1 N: flat force spectrum of magnitude dt
        for (size_t i = 0; i < n; i++) { p.step(a, b); v[i] = p.boardVelocity(at); }
        fft(v);
        std::printf("%-6d", key + 20);
        for (double f = 62.5; f < 8000; f *= 2) {
            // Mean mobility magnitude in the band, then inverted.
            double sum = 0; int cnt = 0;
            for (size_t k = (size_t)(f / fs * n); k < (size_t)(2 * f / fs * n); k++) { sum += std::abs(v[k]) / fs; cnt++; }
            std::printf(" %12.0f", 1.0 / (sum / cnt * fs));  // |V| / (F dt) with dt = 1/fs
        }
        std::printf("\n");
    }
    return 0;
}
