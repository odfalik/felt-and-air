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
    double EA = 0;      // axial stiffness of the load-bearing wire, N
    double T = 0;       // nominal tension, N (the solver retunes tension to hit f0)
    double sigma0 = 0;  // frequency independent loss, 1/s
    double sigma1 = 0;  // frequency dependent loss, m^2/s
    Vec2 bridge;        // where the string meets the bridge, piano coordinates
    bool attached = true;  // false: rigid termination (used by validation tests)
    // The bridge yields along one axis only. This is the tilt of that axis away from the
    // soundboard normal, as seen by this string; it is what couples the two polarisations.
    double bridgeTilt = 0;
};

struct NoteDesign {
    int midi = 60;
    std::vector<StringDesign> strings;
    // Lengths of the same wires behind the bridge that are left free to ring (rear duplex).
    // Never struck, never damped; the bridge alone sets them going.
    std::vector<StringDesign> duplex;
    double strikeRatio = 0.12;  // hammer position / speaking length
    double hammerMass = 0.003;  // kg, whole hammer
    double hammerK = 4.5e9;     // felt stiffness per string contact, N/m^p
    double hammerP = 2.5;       // felt exponent
    double hammerMu = 0.1;      // Hunt-Crossley felt damping, s/m
    // The hammer head rides on a springy wooden shank. shankShare of the hammer's mass sits
    // behind that spring; head and shank together swing at shankHz when free.
    double shankShare = 0.2, shankHz = 300, shankLoss = 0.05;
    bool hasDamper = true;
    double hammerTilt = 0;      // strike direction away from the soundboard normal, rad
    bool twoPolarisations = true;
    // The key landing on the keybed sends a knock through the case into the board's front
    // edge. Impulse reaching the board per unit hammer speed (N s per m/s), and where.
    double keyThump = 0;
    Vec2 keyThumpAt;
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
    // The board is glued up as a shallow dome (the crown). Pushing a dome flat stretches it,
    // which resists normal motion like a spring bed of stiffness sqrt(Ex Ey) H / R^2 per unit
    // area. That is the long-wavelength limit of shallow shell theory with the rim holding the
    // edge; it stiffens the lowest modes and fades out above them. 0 means a flat board.
    double crownRadius = 18.0;
    // How firmly the rim holds the board's edge slope: 0 simply supported, 1 clamped.
    double rimClamp = 1.0;
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

// Ear position in piano coordinates, z measured upward from the soundboard.
struct Listener { double x = 0, y = 0, z = 1; };

// A rectangular room. The piano's coordinate origin (bass end of the keyboard side of the
// board) sits at (pianoX, pianoY) on the floor plan, the board boardHeight above the floor.
struct RoomDesign {
    bool enabled = true;
    double Lx = 9, Ly = 7, Lz = 4;
    double pianoX = 3.0, pianoY = 2.2, boardHeight = 0.75;
    double absorption = 0.18;  // mean absorption coefficient of all surfaces
};

// The grand's lid, hinged along the bass side.
struct LidDesign {
    bool open = true;
    double angleDeg = 38, hingeHeight = 0.28, width = 1.45, length = 1.55, reflection = 0.9;
};

struct PianoDesign {
    BoardDesign board;
    // Longitudinal string motion: stretching driven by the transverse wave (the source of the
    // 'phantom' partials and the bass strings' ring). The tension change it makes at the bridge
    // presses on the board through the downbearing angles, front plus back.
    bool longitudinal = true;
    double downbearingDeg = 1.5;
    // The air pushes back on the moving board: it adds mass at low frequency and carries
    // energy away as sound.
    bool airLoading = true;
    // Keep simulating strings that sit under their dampers, so that they answer faintly to
    // the rest of the instrument as real damped strings do. Otherwise a settled damped
    // string is replaced by a dashpot at the bridge, which is cheaper.
    bool dampedStringsRing = true;
    // What the player's hands and feet set in motion besides the hammers.
    // Each damper rests on its string with this force (weight plus spring, per string). The
    // string sits slightly deflected under it; lifting the damper lets it go, which is the
    // faint stir of the strings when the pedal goes down.
    double damperPress = 0.12;
    // Knocks and friction of the pedal work and the returning keys, as forces on the case
    // that reach the board. 1 is the reference guess; 0 turns them off.
    double mechanismNoise = 1.0;
    double damperStrength = 1.0;
    std::vector<NoteDesign> notes;
    Listener ears[2];
    RoomDesign room;
    LidDesign lid;
    double airRho = 1.2, airC = 343.0;
};

// Everything adjustable about the reference grand, in physical units where the quantity
// has one, and as a multiple of the reference where it varies from note to note.
struct GrandOptions {
    int firstKey = 1, lastKey = 88;  // piano key numbers, A0 = 1, C4 = 40

    // Strings and tuning.
    double a4Hz = 440.0;
    bool stretchTuning = true;       // octaves tuned by matching partials, as on a real piano
    double unisonDetuneCents = 0.6;  // spread of the strings inside one note
    double stringLoss = 1.0;         // x reference: the string's own damping
    double stringStiffness = 1.0;    // x reference: bending stiffness, sets the inharmonicity
    bool longitudinal = true;        // stretching of the strings along their length
    bool airLoading = true;          // reaction of the air on the soundboard
    bool duplex = true;              // free string lengths behind the bridge in the treble
    bool dampedStringsRing = true;   // damped strings stay in the simulation
    double downbearingDeg = 1.5;     // angle the strings make over the bridge, each side
    bool twoPolarisations = true;
    double bridgeTiltDeg = 6.0;      // assumed; see docs/MODEL.md
    double hammerTiltDeg = 4.0;      // assumed

    // Hammers and action.
    double hammerHardness = 1.0;     // x reference: felt stiffness
    double hammerMass = 1.0;         // x reference
    double feltDamping = 1.0;        // x reference
    double strikePosition = 1.0;     // x reference: strike point along the string
    double keyThump = 1.0;           // x reference: knock of the key on the keybed
    double shankHz = 300;            // springiness of the hammer shank; high = rigid
    double damperStrength = 1.0;     // x reference
    double damperPressN = 0.12;      // resting force of a damper on one string
    double mechanismNoise = 1.0;     // x reference: pedal work and key return

    // Soundboard.
    double boardThicknessMm = 8.0;
    double boardLossPercent = 2.0;   // structural loss factor
    double ribHeightMm = 22.0;
    double ribSpacingMm = 125.0;
    double crownRadiusM = 18.0;      // 0: flat board
    double rimClamp = 1.0;           // 0 simply supported, 1 clamped
    double bridgeHeightMm = 34.0;

    // Space. Room coordinates in metres from one corner; the piano is placed by the bass
    // end of its keyboard side, keyboard along x, tail toward +y.
    bool roomEnabled = true;
    double roomLx = 9.0, roomLy = 7.0, roomLz = 4.0;
    double roomAbsorption = 0.18;
    double pianoX = 3.0, pianoY = 2.2;
    double listenerX = 6.4, listenerY = 2.9;  // centre of the head
    double earHeightM = 1.2;                  // above the floor
    double earSpacingM = 0.5;                 // a spaced pair; the head turns to face the piano
    double lidAngleDeg = 38;                  // 0 closes the lid
};

// A generic 2 m class grand. Not a model of any specific instrument.
PianoDesign makeGrand(const GrandOptions& o = {});

inline int keyToMidi(int key) { return key + 20; }
// MIDI velocity (1..127) to hammer speed at let-off, m/s.
double midiVelocityToHammerSpeed(int velocity);

}  // namespace pp
