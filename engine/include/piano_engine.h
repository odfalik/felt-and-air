/* C interface of the piano engine (engine/src/ffi.rs). */
#ifndef PIANO_ENGINE_H
#define PIANO_ENGINE_H
#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Everything adjustable about the reference grand. Field for field the Rust GrandOptions
   (engine/src/design.rs), where each is documented. Fill with pp_options_default first. */
typedef struct PPOptions {
    int first_key, last_key;
    double a4_hz, unison_detune_cents, string_loss, string_stiffness, downbearing_deg, hammer_tilt_deg;
    double hammer_hardness, hammer_mass, felt_damping, strike_position, key_thump, shank_hz, let_off_mm;
    double damper_strength, damper_press_n, mechanism_noise;
    double board_thickness_mm, board_loss_percent, rib_height_mm, rib_spacing_mm, crown_radius_m, rim_clamp, bridge_height_mm;
    double room_lx, room_ly, room_lz, room_absorption, piano_x, piano_y, listener_x, listener_y, ear_height_m, ear_spacing_m, lid_angle_deg;
    bool stretch_tuning, longitudinal, air_loading, duplex, damped_strings_ring, two_polarisations, room_enabled;
} PPOptions;

typedef struct PPPiano PPPiano;
typedef struct PPSnapshot PPSnapshot;
typedef struct PPMidi PPMidi;

void pp_options_default(PPOptions* out);
size_t pp_options_size(void);
double pp_velocity_to_speed(int midi_velocity);

/* NULL if the instrument cannot be built (for example at an unsupported rate). */
PPPiano* pp_new(const PPOptions* options, double sample_rate, int oversample, double board_grid_scale);
void pp_free(PPPiano* piano);

/* Control and rendering: one thread, no allocation, no locks. */
void pp_note_on(PPPiano* piano, int midi, double hammer_speed);
void pp_note_off(PPPiano* piano, int midi, double release_speed);
void pp_set_pedal(PPPiano* piano, double position);
void pp_set_soft_pedal(PPPiano* piano, bool down);
void pp_set_sostenuto(PPPiano* piano, bool down);
void pp_process(PPPiano* piano, float* left, float* right, int frames);

/* on_start and on_exit run once on each helper thread, with context; either may be NULL. */
void pp_set_helper_threads(PPPiano* piano, int count, void (*on_start)(void*), void (*on_exit)(void*), void* context);
bool pp_helpers_busy(const PPPiano* piano);

int pp_string_count(const PPPiano* piano);
int pp_awake_strings(const PPPiano* piano);
int pp_board_nodes(const PPPiano* piano);
void pp_board_spacing(const PPPiano* piano, double* hx, double* hy);
double pp_reverb_time(const PPPiano* piano);
/* Grid storage index of the board node nearest a point (piano coordinates), or -1 off the board. */
int pp_board_node_at(const PPPiano* piano, double x, double y);
void pp_string_bridge(const PPPiano* piano, int index, double* x, double* y);

/* Geometry of a design, without building the instrument. xy holds x0, y0, x1, y1, ... */
int pp_outline(const PPOptions* options, double* xy, int max_points);
int pp_bridge(const PPOptions* options, int which, double* xy, int max_points);
void pp_ears(const PPOptions* options, double* xyz);

/* The adjustable quantities, as one table (engine/src/params.rs): sliders first, then
   switches, then positions on the floor plan. Values are read and written on a PPOptions. */
typedef struct PPParam {
    int kind;            /* 0 slider, 1 switch, 2 position on the floor plan */
    int tab;             /* 0 Space, 1 Hammers, 2 Strings, 3 Soundboard */
    const char* name;
    double lo, hi;
    bool log;            /* best shown on a logarithmic slider */
    const char* format;  /* printf format of the value with its unit */
    const char* help;
} PPParam;
int pp_param_count(void);
void pp_param_info(int index, PPParam* out);
double pp_param_get(const PPOptions* options, int index);
void pp_param_set(PPOptions* options, int index, double value);

/* A copy of what a display needs. Filling and copying do not allocate. */
typedef struct PPSnapshotView {
    const float* board;        /* plate displacement, grid storage, metres */
    int board_len;
    const float* string_level; /* per string: largest displacement, metres */
    int string_count;
    const float* shape[3];     /* displacement along the strings of the focus note */
    int shape_nodes[3];
    float hammer_force[3];
    int focus_midi;
    int awake;
} PPSnapshotView;
PPSnapshot* pp_snapshot_new(const PPPiano* piano);
void pp_snapshot_free(PPSnapshot* snapshot);
void pp_snapshot_fill(const PPPiano* piano, PPSnapshot* snapshot, int focus_midi);
void pp_snapshot_copy(PPSnapshot* dst, const PPSnapshot* src);
void pp_snapshot_view(const PPSnapshot* snapshot, PPSnapshotView* out);

/* MIDI file as timed events, notes led by their hammer's flight time. NULL if nothing was read.
   kind: 0 note off (b release velocity), 1 note on (b velocity), 2 sustain, 3 soft, 4 sostenuto (a 0..127). */
PPMidi* pp_midi_read(const char* path, double let_off_metres);
void pp_midi_free(PPMidi* midi);
int pp_midi_count(const PPMidi* midi);
void pp_midi_get(const PPMidi* midi, int index, double* seconds, int* kind, int* a, int* b);

#ifdef __cplusplus
}
#endif
#endif
