// Loads the CLAP plug-in in this process and drives it the way a host does, without
// installing it: read its descriptor, create it, send a note, pull audio. The CLAP structures
// are declared here as the CLAP 1.x ABI defines them, so no SDK is needed.
#include <algorithm>
#include <atomic>
#include <chrono>
#include <thread>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <vector>

struct clap_version { uint32_t major, minor, revision; };
struct clap_plugin_descriptor {
    clap_version version;
    const char *id, *name, *vendor, *url, *manual_url, *support_url, *plugin_version, *description;
    const char* const* features;
};
struct clap_host {
    clap_version version;
    void* host_data;
    const char *name, *vendor, *url, *host_version;
    const void* (*get_extension)(const clap_host*, const char*);
    void (*request_restart)(const clap_host*);
    void (*request_process)(const clap_host*);
    void (*request_callback)(const clap_host*);
};
struct clap_event_header { uint32_t size, time; uint16_t space_id, type; uint32_t flags; };
struct clap_event_note { clap_event_header header; int32_t note_id; int16_t port_index, channel, key; double velocity; };
struct clap_input_events {
    void* ctx;
    uint32_t (*size)(const clap_input_events*);
    const clap_event_header* (*get)(const clap_input_events*, uint32_t);
};
struct clap_output_events { void* ctx; bool (*try_push)(const clap_output_events*, const clap_event_header*); };
struct clap_audio_buffer { float** data32; double** data64; uint32_t channel_count, latency; uint64_t constant_mask; };
struct clap_process {
    int64_t steady_time;
    uint32_t frames_count;
    const void* transport;
    const clap_audio_buffer* audio_inputs;
    clap_audio_buffer* audio_outputs;
    uint32_t audio_inputs_count, audio_outputs_count;
    const clap_input_events* in_events;
    const clap_output_events* out_events;
};
struct clap_plugin {
    const clap_plugin_descriptor* desc;
    void* plugin_data;
    bool (*init)(const clap_plugin*);
    void (*destroy)(const clap_plugin*);
    bool (*activate)(const clap_plugin*, double, uint32_t, uint32_t);
    void (*deactivate)(const clap_plugin*);
    bool (*start_processing)(const clap_plugin*);
    void (*stop_processing)(const clap_plugin*);
    void (*reset)(const clap_plugin*);
    int32_t (*process)(const clap_plugin*, const clap_process*);
    const void* (*get_extension)(const clap_plugin*, const char*);
    void (*on_main_thread)(const clap_plugin*);
};
struct clap_plugin_factory {
    uint32_t (*get_plugin_count)(const clap_plugin_factory*);
    const clap_plugin_descriptor* (*get_plugin_descriptor)(const clap_plugin_factory*, uint32_t);
    const clap_plugin* (*create_plugin)(const clap_plugin_factory*, const clap_host*, const char*);
};
struct clap_plugin_entry {
    clap_version version;
    bool (*init)(const char*);
    void (*deinit)(void);
    const void* (*get_factory)(const char*);
};
struct clap_param_info { uint32_t id, flags; void* cookie; char name[256], module[1024]; double min_value, max_value, default_value; };
struct clap_plugin_params {
    uint32_t (*count)(const clap_plugin*);
    bool (*get_info)(const clap_plugin*, uint32_t, clap_param_info*);
};
struct clap_event_midi { clap_event_header header; uint16_t port_index; uint8_t data[3]; };
struct clap_event_param_value { clap_event_header header; uint32_t param_id; void* cookie; int32_t note_id; int16_t port_index, channel, key; double value; };
// Prefix of clap_plugin_gui from the CLAP 1.x ABI. Querying these does not create a window.
struct clap_plugin_gui {
    bool (*is_api_supported)(const clap_plugin*, const char*, bool);
    bool (*get_preferred_api)(const clap_plugin*, const char**, bool*);
    bool (*create)(const clap_plugin*, const char*, bool);
    void (*destroy)(const clap_plugin*);
    bool (*set_scale)(const clap_plugin*, double);
    bool (*get_size)(const clap_plugin*, uint32_t*, uint32_t*);
};

static std::vector<clap_event_note> gEvents;
static std::vector<clap_event_midi> gMidi;
static std::vector<clap_event_param_value> gParams;
static std::vector<const clap_event_header*> gInput;
static std::atomic<bool> gCallback{false};

int main(int argc, char** argv) {
    const char* path = argc > 1 ? argv[1] : "build/FeltAndAir.clap/Contents/MacOS/FeltAndAir";
    void* lib = dlopen(path, RTLD_NOW);
    if (!lib) { std::printf("cannot load %s: %s\n", path, dlerror()); return 1; }
    auto* entry = (const clap_plugin_entry*)dlsym(lib, "clap_entry");
    int failures = 0;
    auto check = [&](bool ok, const char* what) { std::printf("  [%s] %s\n", ok ? "ok" : "FAIL", what); failures += !ok; };
    if (!entry || !entry->init(path)) { std::printf("no usable clap_entry\n"); return 1; }
    auto* factory = (const clap_plugin_factory*)entry->get_factory("clap.plugin-factory");
    check(factory && factory->get_plugin_count(factory) == 1, "offers one plug-in");
    const clap_plugin_descriptor* desc = factory->get_plugin_descriptor(factory, 0);
    check(std::strcmp(desc->name, "Felt & Air") == 0 && std::strcmp(desc->id, "com.odfalik.felt-and-air") == 0,
          "reports the new Felt & Air name and CLAP identity");
    std::printf("  %s (%s) by %s, CLAP %u.%u.%u\n", desc->name, desc->id, desc->vendor, desc->version.major, desc->version.minor, desc->version.revision);

    clap_host host = {};
    host.version = desc->version;
    host.name = "claptest"; host.vendor = "felt-and-air"; host.url = ""; host.host_version = "1";
    host.get_extension = [](const clap_host*, const char*) -> const void* { return nullptr; };
    host.request_restart = [](const clap_host*) {};
    host.request_process = [](const clap_host*) {};
    host.request_callback = [](const clap_host*) { gCallback = true; };
    const clap_plugin* plugin = factory->create_plugin(factory, &host, desc->id);
    check(plugin && plugin->init(plugin), "creates and initialises");
    auto* params = (const clap_plugin_params*)plugin->get_extension(plugin, "clap.params");
    uint32_t count = params ? params->count(plugin) : 0;
    std::printf("  %u parameters\n", count);
    check(count > 30, "every setting of the instrument is a host parameter");
    auto* gui = (const clap_plugin_gui*)plugin->get_extension(plugin, "clap.gui");
    check(gui && gui->is_api_supported(plugin, "cocoa", false), "offers an embedded Cocoa editor");
    uint32_t width = 0, height = 0;
    check(gui && gui->get_size(plugin, &width, &height) && width >= 1180 && height >= 800,
          "integrated editor reports its usable window size without opening a window");
    check(plugin->activate(plugin, 48000.0, 1, 512) && plugin->start_processing(plugin), "activates at 48 kHz");

    std::vector<float> l(512), r(512);
    float* chans[2] = {l.data(), r.data()};
    clap_audio_buffer out = {chans, nullptr, 2, 0, 0};
    clap_input_events in = {nullptr, [](const clap_input_events*) { return (uint32_t)gInput.size(); },
                            [](const clap_input_events*, uint32_t i) -> const clap_event_header* { return gInput[i]; }};
    clap_output_events sink = {nullptr, [](const clap_output_events*, const clap_event_header*) { return true; }};
    clap_process proc = {};
    proc.frames_count = 512; proc.audio_outputs = &out; proc.audio_outputs_count = 1; proc.in_events = &in; proc.out_events = &sink;
    auto note = [](uint16_t type, int key, double velocity, uint32_t time) {
        clap_event_note e = {};
        e.header = {sizeof(clap_event_note), time, 0, type, 0};
        e.note_id = -1; e.key = (int16_t)key; e.velocity = velocity;
        return e;
    };
    auto process = [&] {
        gInput.clear();
        for (const auto& e : gEvents) gInput.push_back(&e.header);
        for (const auto& e : gMidi) gInput.push_back(&e.header);
        for (const auto& e : gParams) gInput.push_back(&e.header);
        std::stable_sort(gInput.begin(), gInput.end(), [](auto* a, auto* b) { return a->time < b->time; });
        plugin->process(plugin, &proc);
        if (gCallback.exchange(false)) plugin->on_main_thread(plugin);
        proc.steady_time += 512;
    };
    auto clear = [&] { gEvents.clear(); gMidi.clear(); gParams.clear(); };
    auto cc = [&](uint8_t controller, uint8_t value) {
        clap_event_midi e = {}; e.header = {sizeof(e), 0, 0, 10, 0};
        e.data[0] = 0xb0; e.data[1] = controller; e.data[2] = value; gMidi.push_back(e);
    };
    double peak = 0, early = 0, late = 0;
    bool finite = true;
    for (int block = 0; block < 280; block++) {
        gEvents.clear();
        if (block == 2) { gEvents.push_back(note(0, 60, 0.7, 100)); gEvents.push_back(note(0, 64, 0.6, 300)); }
        if (block == 150) { gEvents.push_back(note(1, 60, 0.5, 0)); gEvents.push_back(note(1, 64, 0.5, 0)); }
        process();
        for (int i = 0; i < 512; i++) {
            finite = finite && std::isfinite(l[i]);
            peak = std::fmax(peak, std::fabs(l[i]));
            if (block >= 4 && block < 40) early = std::fmax(early, std::fabs(l[i]));
            if (block >= 250) late = std::fmax(late, std::fabs(l[i]));
        }
    }
    check(finite && peak > 0.01 && peak <= 1.0, "notes produce sound within range");
    check(late < 0.05 * early, "note-off brings it back to quiet");
    std::printf("  peak %.3f, after release %.4f\n", peak, late);
    // Real host regression: C held -> CC66 down -> D held -> parameter change ->
    // release D and C. Only C belonged to the pedal's capture set and must keep ringing.
    // CLAP has no existing applied-generation readout: this does NOT assert completion
    // of the asynchronous rebuild. The real-engine Rust regression covers acceptance.
    uint32_t hardness_id = UINT32_MAX;
    for (uint32_t i = 0; i < count; ++i) {
        clap_param_info info = {};
        if (params->get_info(plugin, i, &info) && std::strcmp(info.name, "Felt hardness") == 0) hardness_id = info.id;
    }
    check(hardness_id != UINT32_MAX, "host can identify a physical rebuild parameter");
    plugin->reset(plugin);
    std::vector<float> retained;
    for (int block = 0; block < 180; ++block) {
        clear();
        if (block == 0) gEvents.push_back(note(0, 60, 0.7, 0));
        if (block == 8) cc(66, 127);
        if (block == 12) gEvents.push_back(note(0, 62, 0.7, 0));
        if (block == 20 && hardness_id != UINT32_MAX) {
            clap_event_param_value e = {}; e.header = {sizeof(e), 0, 0, 5, 0};
            e.param_id = hardness_id; e.note_id = -1; e.port_index = e.channel = e.key = -1; e.value = 1.2; gParams.push_back(e);
        }
        // Deliberate waits on the host control thread (never the callback) to exercise the
        // wall-clock debounce and allow the real framework background executor to finish.
        if (block == 21) std::this_thread::sleep_for(std::chrono::milliseconds(220));
        if (block == 24) std::this_thread::sleep_for(std::chrono::milliseconds(400));
        if (block == 35) gEvents.push_back(note(1, 60, 0.5, 0)); // C is now released but captured
        if (block == 40) cc(64, 100);
        if (block == 42) cc(67, 127);
        if (block == 44) cc(64, 0);
        if (block == 46) cc(67, 0);
        if (block == 65) gEvents.push_back(note(1, 62, 0.5, 0));
        process();
        if (block >= 110 && block < 174) retained.insert(retained.end(), l.begin(), l.end());
    }
    auto tone = [](const std::vector<float>& audio, double hz, double fs) {
        double re = 0, im = 0;
        for (size_t i = 0; i < audio.size(); ++i) {
            double w = 0.5 - 0.5 * std::cos(2 * M_PI * i / (audio.size()-1));
            double a = 2 * M_PI * hz * i / fs;
            re += audio[i] * w * std::cos(a); im += audio[i] * w * std::sin(a);
        }
        return std::hypot(re,im) / audio.size();
    };
    double c4 = tone(retained, 261.626, 48000), d4 = tone(retained, 293.665, 48000);
    check(std::isfinite(c4) && c4 > 1e-6 && d4 < c4 * 0.2,
          "CC66 capture survives host parameter change + other pedals; later D damps while released captured C rings");
    std::printf("  retained C4 %.3e, later D4 %.3e (spectral amplitude)\n", c4, d4);
    plugin->stop_processing(plugin);
    plugin->deactivate(plugin);
    check(plugin->activate(plugin, 96000.0, 1, 512) && plugin->start_processing(plugin), "reactivates the same instance at 96 kHz");
    double peak96 = 0; bool finite96 = true;
    for (int block = 0; block < 60; ++block) {
        clear(); if (block == 1) gEvents.push_back(note(0, 60, 0.7, 0));
        process();
        for (float v : l) { finite96 &= std::isfinite(v); peak96 = std::fmax(peak96,std::fabs(v)); }
    }
    check(finite96 && peak96 > 0.01 && peak96 <= 1.0, "reactivated instrument renders finite bounded audio at the new rate");
    plugin->stop_processing(plugin);
    plugin->deactivate(plugin);
    plugin->destroy(plugin);
    entry->deinit();
    std::printf("%s\n", failures ? "FAILED" : "ALL PASSED");
    return failures ? 1 : 0;
}
