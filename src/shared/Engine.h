// The Audio Unit's thin C++ face on the engine's C interface (engine/include/piano_engine.h).
#pragma once
#include <functional>
#include <string>
#include <vector>
#include "piano_engine.h"

namespace pp {

struct Vec2 { double x = 0, y = 0; };

struct GrandOptions : PPOptions {
    GrandOptions() { pp_options_default(this); }
};

struct MidiEvent { double seconds; int type, a, b; };  // type: see pp_midi_get

inline std::vector<MidiEvent> readMidiFile(const std::string& path, double letOffMetres) {
    std::vector<MidiEvent> out;
    PPMidi* m = pp_midi_read(path.c_str(), letOffMetres);
    if (!m) return out;
    out.resize(pp_midi_count(m));
    for (size_t i = 0; i < out.size(); i++) pp_midi_get(m, (int)i, &out[i].seconds, &out[i].type, &out[i].a, &out[i].b);
    pp_midi_free(m);
    return out;
}

inline double midiVelocityToHammerSpeed(int velocity) { return pp_velocity_to_speed(velocity); }

// Outline and bridge lines of a design, in piano coordinates.
inline std::vector<Vec2> outlineOf(const PPOptions& o) {
    double xy[128];
    int n = pp_outline(&o, xy, 64);
    std::vector<Vec2> out(n);
    for (int i = 0; i < n; i++) out[i] = {xy[2 * i], xy[2 * i + 1]};
    return out;
}
inline std::vector<Vec2> bridgeOf(const PPOptions& o, int which) {
    double xy[256];
    int n = pp_bridge(&o, which, xy, 128);
    std::vector<Vec2> out(n);
    for (int i = 0; i < n; i++) out[i] = {xy[2 * i], xy[2 * i + 1]};
    return out;
}

class Piano {
public:
    // Check ok() before use: building fails at rates the strings cannot be resolved at.
    Piano(const PPOptions& options, double sampleRate, double boardGridScale) : p_(pp_new(&options, sampleRate, 1, boardGridScale)) {}
    ~Piano() { pp_free(p_); }
    Piano(const Piano&) = delete;
    Piano& operator=(const Piano&) = delete;
    bool ok() const { return p_ != nullptr; }
    PPPiano* raw() const { return p_; }

    void noteOn(int midi, double hammerSpeed) { pp_note_on(p_, midi, hammerSpeed); }
    void noteOff(int midi, double releaseSpeed = 1.0) { pp_note_off(p_, midi, releaseSpeed); }
    void setPedal(double position) { pp_set_pedal(p_, position); }
    void setSustain(bool down) { pp_set_pedal(p_, down ? 1.0 : 0.0); }
    void setSoftPedal(bool down) { pp_set_soft_pedal(p_, down); }
    void setSostenuto(bool down) { pp_set_sostenuto(p_, down); }
    void process(float* left, float* right, int frames) { pp_process(p_, left, right, frames); }

    // onStart and onExit run once on each helper thread (for example to join and leave an
    // audio workgroup).
    void setHelperThreads(int count, std::function<void()> onStart = {}, std::function<void()> onExit = {}) {
        pp_set_helper_threads(p_, 0, nullptr, nullptr, nullptr);  // old helpers are gone before their hooks are replaced
        onStart_ = std::move(onStart);
        onExit_ = std::move(onExit);
        pp_set_helper_threads(p_, count, onStart_ ? &Piano::start : nullptr, onExit_ ? &Piano::exit : nullptr, this);
    }
    bool helpersBusy() const { return pp_helpers_busy(p_); }

    int stringCount() const { return pp_string_count(p_); }
    int awakeStrings() const { return pp_awake_strings(p_); }
    int boardNodes() const { return pp_board_nodes(p_); }
    Vec2 boardSpacing() const { Vec2 h; pp_board_spacing(p_, &h.x, &h.y); return h; }
    int boardNodeAt(Vec2 p) const { return pp_board_node_at(p_, p.x, p.y); }
    Vec2 stringBridge(int index) const { Vec2 b; pp_string_bridge(p_, index, &b.x, &b.y); return b; }

private:
    static void start(void* self) { ((Piano*)self)->onStart_(); }
    static void exit(void* self) { ((Piano*)self)->onExit_(); }
    PPPiano* p_;
    std::function<void()> onStart_, onExit_;
};

}  // namespace pp
