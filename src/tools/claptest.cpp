// Loads the CLAP plug-in in this process and drives it the way a host does, without
// installing it: read its descriptor, create it, send a note, pull audio. The CLAP structures
// are declared here as the CLAP 1.x ABI defines them, so no SDK is needed.
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
struct clap_plugin_params {
    uint32_t (*count)(const clap_plugin*);
};

static std::vector<clap_event_note> gEvents;
static bool gCallback = false;

int main(int argc, char** argv) {
    const char* path = argc > 1 ? argv[1] : "build/PhysicalPiano.clap/Contents/MacOS/PhysicalPiano";
    void* lib = dlopen(path, RTLD_NOW);
    if (!lib) { std::printf("cannot load %s: %s\n", path, dlerror()); return 1; }
    auto* entry = (const clap_plugin_entry*)dlsym(lib, "clap_entry");
    int failures = 0;
    auto check = [&](bool ok, const char* what) { std::printf("  [%s] %s\n", ok ? "ok" : "FAIL", what); failures += !ok; };
    if (!entry || !entry->init(path)) { std::printf("no usable clap_entry\n"); return 1; }
    auto* factory = (const clap_plugin_factory*)entry->get_factory("clap.plugin-factory");
    check(factory && factory->get_plugin_count(factory) == 1, "offers one plug-in");
    const clap_plugin_descriptor* desc = factory->get_plugin_descriptor(factory, 0);
    std::printf("  %s (%s) by %s, CLAP %u.%u.%u\n", desc->name, desc->id, desc->vendor, desc->version.major, desc->version.minor, desc->version.revision);

    clap_host host = {};
    host.version = desc->version;
    host.name = "claptest"; host.vendor = "physical-piano"; host.url = ""; host.host_version = "1";
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
    check(plugin->activate(plugin, 48000.0, 1, 512) && plugin->start_processing(plugin), "activates at 48 kHz");

    std::vector<float> l(512), r(512);
    float* chans[2] = {l.data(), r.data()};
    clap_audio_buffer out = {chans, nullptr, 2, 0, 0};
    clap_input_events in = {nullptr, [](const clap_input_events*) { return (uint32_t)gEvents.size(); },
                            [](const clap_input_events*, uint32_t i) -> const clap_event_header* { return &gEvents[i].header; }};
    clap_output_events sink = {nullptr, [](const clap_output_events*, const clap_event_header*) { return true; }};
    clap_process proc = {};
    proc.frames_count = 512; proc.audio_outputs = &out; proc.audio_outputs_count = 1; proc.in_events = &in; proc.out_events = &sink;
    auto note = [](uint16_t type, int key, double velocity, uint32_t time) {
        clap_event_note e = {};
        e.header = {sizeof(clap_event_note), time, 0, type, 0};
        e.note_id = -1; e.key = (int16_t)key; e.velocity = velocity;
        return e;
    };
    double peak = 0, early = 0, late = 0;
    bool finite = true;
    for (int block = 0; block < 280; block++) {
        gEvents.clear();
        if (block == 2) { gEvents.push_back(note(0, 60, 0.7, 100)); gEvents.push_back(note(0, 64, 0.6, 300)); }
        if (block == 150) { gEvents.push_back(note(1, 60, 0.5, 0)); gEvents.push_back(note(1, 64, 0.5, 0)); }
        plugin->process(plugin, &proc);
        if (gCallback) { gCallback = false; plugin->on_main_thread(plugin); }
        proc.steady_time += 512;
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
    plugin->stop_processing(plugin);
    plugin->deactivate(plugin);
    plugin->destroy(plugin);
    entry->deinit();
    std::printf("%s\n", failures ? "FAILED" : "ALL PASSED");
    return failures ? 1 : 0;
}
