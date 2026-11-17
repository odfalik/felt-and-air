// Physical description of the instrument. Everything here is in SI units and describes
// geometry and materials, not numerics. See docs/MODEL.md for sources and caveats.
#pragma once
#include <vector>

namespace pp {

struct Vec2 { double x = 0, y = 0; };

struct StringDesign {
    double f0 = 0;      // target fundamental of this string, Hz (unison detune included)
    double L = 0;       // speaking length, m
    double rhoA = 0;    // linear mass density, kg/m
    double EI = 0;      // bending stiffness, N m^2
    double T = 0;       // nominal tension, N (the solver retunes tension to hit f0)
    double sigma0 = 0;  // frequency independent loss, 1/s
    double sigma1 = 0;  // frequency dependent loss, m^2/s
    Vec2 bridge;        // where the string meets the bridge, piano coordinates
    bool attached = true;  // false: rigid termination (used by validation tests)
};

struct NoteDesign {
    int midi = 60;
    std::vector<StringDesign> strings;
    double strikeRatio = 0.12;  // hammer position / speaking length
    double hammerMass = 0.003;  // kg, whole hammer
    double hammerK = 4.5e9;     // felt stiffness per string contact, N/m^p
    double hammerP = 2.5;       // felt exponent
    double hammerMu = 0.1;      // Hunt-Crossley felt damping, s/m
    bool hasDamper = true;
};

struct BoardDesign {
    // Outline in piano coordinates: X along the keyboard (bass at 0), Y toward the tail.
    std::vector<Vec2> outline;
    double grainAngle = 0;  // direction of the grain in piano coordinates, rad
    double thickness = 0.008;
    double rho = 420;
    double Ex = 11.5e9;  // along grain
    double Ey = 0.75e9;  // across grain
    double Gxy = 0.70e9;
    double nuxy = 0.37;
    double lossFactor = 0.02;  // structural loss factor eta
    double sigma0 = 2.0;       // broadband loss, 1/s
    // Ribs run across the grain.
    double ribSpacing = 0.125, ribWidth = 0.024, ribHeight = 0.022;
    double ribE = 11.0e9, ribRho = 420;
    bool ribs = true;
    // Bridges are beams glued along polylines (piano coordinates).
    std::vector<std::vector<Vec2>> bridges;
    double bridgeWidth = 0.032, bridgeHeight = 0.034;
    double bridgeE = 12.0e9, bridgeRho = 680;
};

struct Listener { double x = 0, y = 0, z = 1; };

struct PianoDesign {
    BoardDesign board;
    std::vector<NoteDesign> notes;
    Listener ears[2];
    double airRho = 1.2, airC = 343.0;
};

struct GrandOptions {
    int firstKey = 1, lastKey = 88;  // piano key numbers, A0 = 1, C4 = 40
    double unisonDetuneCents = 0.6;  // spread of the strings inside one note
};

// A generic 2 m class grand. Not a model of any specific instrument.
PianoDesign makeGrand(const GrandOptions& o = {});

inline int keyToMidi(int key) { return key + 20; }
// MIDI velocity (1..127) to hammer speed at let-off, m/s.
double midiVelocityToHammerSpeed(int velocity);

}  // namespace pp
