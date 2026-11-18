// The adjustable quantities of the instrument, as one table for every front end: the app's
// Instrument window and the plug-in's parameters. Every one is a physical number.
#pragma once
#include <cmath>
#include "piano_engine.h"

namespace pp {

// One adjustable quantity of the design. Every one is a physical number.
struct Knob { int tab; const char* name; double PPOptions::* field; double lo, hi; bool logScale; const char* format; const char* help; };
enum { kSpace, kHammers, kStrings, kBoard, kTabCount };
inline const char* const kTabNames[] = {"Space", "Hammers", "Strings", "Soundboard"};
static const Knob kKnobs[] = {
    {kSpace, "Room length", &PPOptions::room_lx, 3.0, 30.0, true, "%.1f m", "Along the keyboard direction."},
    {kSpace, "Room width", &PPOptions::room_ly, 3.0, 30.0, true, "%.1f m", "Along the strings."},
    {kSpace, "Room height", &PPOptions::room_lz, 2.2, 15.0, true, "%.1f m", ""},
    {kSpace, "Wall absorption", &PPOptions::room_absorption, 0.03, 0.7, true, "%.2f", "Share of sound each surface soaks up. Low is a tiled hall, high is a carpeted studio."},
    {kSpace, "Ear height", &PPOptions::ear_height_m, 0.6, 2.5, false, "%.2f m", "Above the floor. The soundboard sits at 0.75 m."},
    {kSpace, "Ear spacing", &PPOptions::ear_spacing_m, 0.1, 1.5, false, "%.2f m", "A spaced pair; wider gives a wider image."},
    {kSpace, "Lid angle", &PPOptions::lid_angle_deg, 0.0, 60.0, false, "%.0f deg", "0 takes the lid's reflection away."},
    {kHammers, "Felt hardness", &PPOptions::hammer_hardness, 0.2, 5.0, true, "x %.2f", "Felt stiffness. Harder is brighter."},
    {kHammers, "Hammer mass", &PPOptions::hammer_mass, 0.5, 2.0, true, "x %.2f", "Heavier stays on the string longer: louder and duller."},
    {kHammers, "Felt damping", &PPOptions::felt_damping, 0.0, 5.0, false, "x %.2f", "Energy the felt loses as it is squeezed."},
    {kHammers, "Strike point", &PPOptions::strike_position, 0.5, 1.8, false, "x %.2f", "Distance from the agraffe; the reference is about 1/8 of the string."},
    {kHammers, "Key knock", &PPOptions::key_thump, 0.0, 30.0, false, "x %.1f", "Knock of the key landing on the keybed."},
    {kHammers, "Let-off distance", &PPOptions::let_off_mm, 0.5, 5.0, false, "%.1f mm", "Gap to the string when the hammer is let go. It flies the rest, slowed by gravity."},
    {kHammers, "Shank springiness", &PPOptions::shank_hz, 80.0, 2000.0, true, "%.0f Hz", "The wooden shank flexes at impact. High is a rigid hammer."},
    {kHammers, "Damper strength", &PPOptions::damper_strength, 0.2, 4.0, true, "x %.2f", "How fast a released note is stopped."},
    {kHammers, "Damper weight", &PPOptions::damper_press_n, 0.0, 0.6, false, "%.2f N", "Force each damper rests on its string with. Lifting it lets the string stir."},
    {kHammers, "Pedal and key noise", &PPOptions::mechanism_noise, 0.0, 10.0, false, "x %.1f", "Knocks and friction of the pedal work and returning keys."},
    {kStrings, "Pitch of A4", &PPOptions::a4_hz, 415.0, 466.0, false, "%.1f Hz", ""},
    {kStrings, "Unison detune", &PPOptions::unison_detune_cents, 0.0, 6.0, false, "%.2f cents", "Spread between the strings of one note. More gives beating, then honky-tonk."},
    {kStrings, "Internal loss", &PPOptions::string_loss, 0.25, 5.0, true, "x %.2f", "The string's own damping, apart from what the board takes."},
    {kStrings, "Wire stiffness", &PPOptions::string_stiffness, 0.25, 4.0, true, "x %.2f", "Bending stiffness. More makes the overtones sharper."},
    {kStrings, "Downbearing angle", &PPOptions::downbearing_deg, 0.0, 5.0, false, "%.1f deg", "Bend of the strings over the bridge; sets how hard stretching presses on the board."},
    {kStrings, "Strike angle", &PPOptions::hammer_tilt_deg, 0.0, 15.0, false, "%.1f deg", "How far off the upright the hammer hits; this is what sets strings moving sideways."},
    {kBoard, "Thickness", &PPOptions::board_thickness_mm, 5.0, 13.0, false, "%.1f mm", ""},
    {kBoard, "Loss factor", &PPOptions::board_loss_percent, 0.5, 8.0, true, "%.1f %%", "Internal damping of the wood."},
    {kBoard, "Rib height", &PPOptions::rib_height_mm, 10.0, 36.0, false, "%.0f mm", ""},
    {kBoard, "Rib spacing", &PPOptions::rib_spacing_mm, 80.0, 220.0, false, "%.0f mm", ""},
    {kBoard, "Crown radius", &PPOptions::crown_radius_m, 6.0, 80.0, true, "%.0f m", "Smaller is a more domed, stiffer board."},
    {kBoard, "Rim grip", &PPOptions::rim_clamp, 0.0, 1.0, false, "%.2f", "0: edge free to tilt. 1: edge clamped."},
    {kBoard, "Bridge height", &PPOptions::bridge_height_mm, 20.0, 50.0, false, "%.0f mm", ""},
};
static const int kKnobCount = sizeof(kKnobs) / sizeof(kKnobs[0]);
// Fields that have no slider but belong to a saved setup.
inline double PPOptions::* const kPlanFields[] = {&PPOptions::piano_x, &PPOptions::piano_y, &PPOptions::listener_x, &PPOptions::listener_y};
inline double knobToSlider(const Knob& k, double v) { return k.logScale ? std::log(v / k.lo) / std::log(k.hi / k.lo) : (v - k.lo) / (k.hi - k.lo); }
inline double sliderToKnob(const Knob& k, double t) { return k.logScale ? k.lo * std::pow(k.hi / k.lo, t) : k.lo + t * (k.hi - k.lo); }

struct Toggle { int tab; const char* name; bool PPOptions::* field; };
static const Toggle kToggles[] = {
    {kSpace, "Room (walls, floor, ceiling, reverberation)", &PPOptions::room_enabled},
    {kStrings, "Stretch tuning (octaves tuned by matching overtones)", &PPOptions::stretch_tuning},
    {kStrings, "Longitudinal motion (stretching: phantom partials, bass ring)", &PPOptions::longitudinal},
    {kStrings, "Duplex lengths behind the bridge (treble shimmer)", &PPOptions::duplex},
    {kStrings, "Damped strings keep ringing faintly (needed for the pedal stir)", &PPOptions::damped_strings_ring},
    {kStrings, "Sideways string motion (of struck strings)", &PPOptions::two_polarisations},
    {kBoard, "Air loading (the air's mass and drag on the board)", &PPOptions::air_loading},
};
static const int kToggleCount = sizeof(kToggles) / sizeof(kToggles[0]);


}  // namespace pp
