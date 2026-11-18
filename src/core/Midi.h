// Minimal Standard MIDI File reader: notes and the sustain pedal, with tempo changes,
// flattened to a list of timed events.
#pragma once
#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

namespace pp {

struct MidiEvent { double seconds; int type, a, b; };  // 1 note on (a note, b velocity), 0 note off (b release velocity, 0 unknown), 2 pedal (a = position 0..127)

inline std::vector<MidiEvent> readMidiFile(const std::string& path) {
    std::vector<MidiEvent> out;
    FILE* f = std::fopen(path.c_str(), "rb");
    if (!f) return out;
    std::vector<uint8_t> d;
    uint8_t buf[65536];
    size_t got;
    while ((got = std::fread(buf, 1, sizeof(buf), f)) > 0) d.insert(d.end(), buf, buf + got);
    std::fclose(f);
    size_t p = 0;
    auto u32 = [&] { uint32_t v = 0; for (int i = 0; i < 4 && p < d.size(); i++) v = v << 8 | d[p++]; return v; };
    auto u16 = [&] { uint32_t v = 0; for (int i = 0; i < 2 && p < d.size(); i++) v = v << 8 | d[p++]; return v; };
    if (d.size() < 14 || d[0] != 'M' || d[1] != 'T' || d[2] != 'h' || d[3] != 'd') return out;
    p = 4;
    uint32_t hlen = u32();
    size_t hend = p + hlen;
    u16();
    int tracks = (int)u16(), division = (int)u16();
    if (division <= 0 || (division & 0x8000)) return out;  // SMPTE time is not supported
    p = hend;

    struct Raw { uint64_t tick; int type, a, b; };  // type 3: tempo (a = microseconds per quarter)
    std::vector<Raw> raw;
    for (int t = 0; t < tracks && p + 8 <= d.size(); t++) {
        if (!(d[p] == 'M' && d[p + 1] == 'T' && d[p + 2] == 'r' && d[p + 3] == 'k')) break;
        p += 4;
        uint32_t len = u32();
        size_t end = std::min(d.size(), p + len);
        uint64_t tick = 0;
        int status = 0;
        auto var = [&] { uint32_t v = 0; while (p < end) { uint8_t c = d[p++]; v = v << 7 | (c & 0x7F); if (!(c & 0x80)) break; } return v; };
        while (p < end) {
            tick += var();
            if (p >= end) break;
            if (d[p] & 0x80) status = d[p++];
            if (status == 0xFF) {
                int meta = p < end ? d[p++] : 0;
                uint32_t l = var();
                if (meta == 0x51 && l == 3 && p + 3 <= end) raw.push_back({tick, 3, d[p] << 16 | d[p + 1] << 8 | d[p + 2], 0});
                p += l;
            } else if (status == 0xF0 || status == 0xF7) {
                p += var();
            } else {
                int hi = status & 0xF0, a = p < end ? d[p++] : 0, b = 0;
                if (hi != 0xC0 && hi != 0xD0) b = p < end ? d[p++] : 0;
                if (hi == 0x90 && b > 0) raw.push_back({tick, 1, a, b});
                else if (hi == 0x80 || hi == 0x90) raw.push_back({tick, 0, a, hi == 0x80 ? b : 0});  // b: release velocity
                else if (hi == 0xB0 && a == 64) raw.push_back({tick, 2, b, 0});
            }
        }
        p = end;
    }
    std::stable_sort(raw.begin(), raw.end(), [](const Raw& x, const Raw& y) { return x.tick < y.tick; });
    double seconds = 0, usPerQuarter = 500000;
    uint64_t last = 0;
    int pedal = -1;
    for (const Raw& r : raw) {
        seconds += (double)(r.tick - last) * usPerQuarter * 1e-6 / division;
        last = r.tick;
        if (r.type == 3) { usPerQuarter = r.a; continue; }
        if (r.type == 2) { if (r.a == pedal) continue; pedal = r.a; }
        out.push_back({seconds, r.type, r.a, r.b});
    }
    return out;
}

}  // namespace pp
