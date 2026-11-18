// The engine's parameter table (engine/src/params.rs) in the shape the app and the Audio
// Unit use: sliders, switches, and the four positions on the floor plan.
#pragma once
#include <cmath>
#include <vector>
#include "piano_engine.h"

namespace pp {

enum { kSpace, kHammers, kStrings, kBoard, kTabCount };
inline const char* const kTabNames[] = {"Space", "Hammers", "Strings", "Soundboard"};

struct Knob {
    int index, tab;
    const char* name;
    double lo, hi;
    bool logScale;
    const char* format;
    const char* help;
    double get(const PPOptions& o) const { return pp_param_get(&o, index); }
    void set(PPOptions& o, double v) const { pp_param_set(&o, index, v); }
};
struct Toggle {
    int index, tab;
    const char* name;
    bool get(const PPOptions& o) const { return pp_param_get(&o, index) >= 0.5; }
    void set(PPOptions& o, bool on) const { pp_param_set(&o, index, on ? 1.0 : 0.0); }
};

inline std::vector<Knob> loadKnobs(int kind) {
    std::vector<Knob> out;
    for (int i = 0; i < pp_param_count(); i++) {
        PPParam p;
        pp_param_info(i, &p);
        if (p.kind == kind) out.push_back({i, p.tab, p.name, p.lo, p.hi, p.log, p.format, p.help});
    }
    return out;
}
inline const std::vector<Knob> kKnobs = loadKnobs(0);
inline const int kKnobCount = (int)kKnobs.size();
inline const std::vector<Knob> kPlan = loadKnobs(2);  // piano x, y; listener x, y
inline const std::vector<Toggle> kToggles = [] {
    std::vector<Toggle> out;
    for (const Knob& k : loadKnobs(1)) out.push_back({k.index, k.tab, k.name});
    return out;
}();
inline const int kToggleCount = (int)kToggles.size();

inline double knobToSlider(const Knob& k, double v) { return k.logScale ? std::log(v / k.lo) / std::log(k.hi / k.lo) : (v - k.lo) / (k.hi - k.lo); }
inline double sliderToKnob(const Knob& k, double t) { return k.logScale ? k.lo * std::pow(k.hi / k.lo, t) : k.lo + t * (k.hi - k.lo); }

}  // namespace pp
