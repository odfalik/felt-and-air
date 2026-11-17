// Small self-contained helpers: WAV output and a radix-2 FFT for analysis tools/tests.
#pragma once
#include <cmath>
#include <complex>
#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

namespace pp {

inline bool writeWavFloat(const std::string& path, const std::vector<float>& interleaved,
                          int channels, int sampleRate) {
    FILE* f = std::fopen(path.c_str(), "wb");
    if (!f) return false;
    uint32_t dataBytes = (uint32_t)(interleaved.size() * sizeof(float));
    auto w32 = [&](uint32_t v) { std::fwrite(&v, 4, 1, f); };
    auto w16 = [&](uint16_t v) { std::fwrite(&v, 2, 1, f); };
    std::fwrite("RIFF", 1, 4, f); w32(36 + dataBytes);
    std::fwrite("WAVE", 1, 4, f);
    std::fwrite("fmt ", 1, 4, f); w32(16); w16(3); w16((uint16_t)channels);
    w32((uint32_t)sampleRate); w32((uint32_t)(sampleRate * channels * 4));
    w16((uint16_t)(channels * 4)); w16(32);
    std::fwrite("data", 1, 4, f); w32(dataBytes);
    std::fwrite(interleaved.data(), 1, dataBytes, f);
    std::fclose(f);
    return true;
}

// In-place complex FFT, n must be a power of two.
inline void fft(std::vector<std::complex<double>>& a) {
    const size_t n = a.size();
    for (size_t i = 1, j = 0; i < n; i++) {
        size_t bit = n >> 1;
        for (; j & bit; bit >>= 1) j ^= bit;
        j ^= bit;
        if (i < j) std::swap(a[i], a[j]);
    }
    for (size_t len = 2; len <= n; len <<= 1) {
        double ang = -2.0 * M_PI / (double)len;
        std::complex<double> wl(std::cos(ang), std::sin(ang));
        for (size_t i = 0; i < n; i += len) {
            std::complex<double> w(1.0);
            for (size_t j = 0; j < len / 2; j++) {
                auto u = a[i + j], v = a[i + j + len / 2] * w;
                a[i + j] = u + v;
                a[i + j + len / 2] = u - v;
                w *= wl;
            }
        }
    }
}

// Hann-windowed magnitude spectrum of a real signal (zero-padded to nfft).
inline std::vector<double> magSpectrum(const double* x, size_t n, size_t nfft) {
    std::vector<std::complex<double>> a(nfft);
    for (size_t i = 0; i < n && i < nfft; i++) {
        double w = 0.5 - 0.5 * std::cos(2.0 * M_PI * (double)i / (double)(n - 1));
        a[i] = x[i] * w;
    }
    fft(a);
    std::vector<double> m(nfft / 2);
    for (size_t i = 0; i < nfft / 2; i++) m[i] = std::abs(a[i]);
    return m;
}

// Refine a spectral peak near bin b by parabolic interpolation on log magnitude.
inline double peakBin(const std::vector<double>& m, size_t b) {
    if (b == 0 || b + 1 >= m.size()) return (double)b;
    double a = std::log(m[b - 1] + 1e-300), c = std::log(m[b] + 1e-300), d = std::log(m[b + 1] + 1e-300);
    double den = a - 2 * c + d;
    return (double)b + (den != 0 ? 0.5 * (a - d) / den : 0.0);
}

}  // namespace pp
