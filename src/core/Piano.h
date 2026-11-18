// The coupled instrument: stiff strings, felt hammers, dampers, soundboard, radiation.
//
// All parts form one discrete mechanical system  M q'' + C q' + K q = f_hammer  with a
// symmetric K, stepped by a leapfrog scheme. The discrete energy returned by energy() is
// conserved to rounding error when all losses are switched off, and can only decrease
// otherwise. docs/MODEL.md has the derivation.
#pragma once
#include "Acoustics.h"
#include "Design.h"
#include "Plate.h"
#include <cstdint>
#include <functional>
#include <memory>
#include <vector>

namespace pp {

struct PianoOptions {
    double sampleRate = 48000;
    int oversample = 1;      // internal steps per output sample
    double outputGain = 0.06; // full scale per pascal
    // Soundboard grid spacing relative to the finest stable one. 1 is the finest; larger
    // values trade spatial resolution for processor time (node count falls as 1/scale^2).
    double boardGridScale = 1.0;
};

class Piano {
public:
    Piano(const PianoDesign& design, const PianoOptions& options);
    ~Piano();

    // Helper threads that share the string work when many strings are ringing. They idle
    // (asleep) until then. Call before audio starts.
    // onStart and onExit run once on each helper thread, at its start and just before it
    // ends (for example to join and leave an audio workgroup).
    void setHelperThreads(int count, std::function<void()> onStart = {}, std::function<void()> onExit = {});
    bool helpersBusy() const;

    // Control. Call from the same thread as process(); none of these allocate.
    void noteOn(int midi, double hammerSpeed);
    // releaseSpeed: how fast the key comes back up, 1 = ordinary. Sets how quickly the damper
    // lands and how hard the key knocks on its rest.
    void noteOff(int midi, double releaseSpeed = 1.0);
    void setSustain(bool down) { setPedal(down ? 1.0 : 0.0); }
    // Sustain pedal position, 0 (up) to 1 (down). Between about 0.34 and 0.47 the dampers only
    // graze the strings (half pedal).
    void setPedal(double position);

    // Renders n stereo frames. Allocation and lock free.
    void process(float* left, float* right, int n);
    // One internal time step; returns the two ear pressures in pascal at the internal rate.
    void step(double& pLeft, double& pRight);

    // A copy of what a display needs. prepare once, then fill from the audio thread
    // (allocation free).
    struct Snapshot {
        std::vector<float> board;        // plate displacement, grid storage, metres
        std::vector<float> stringLevel;  // per string: largest displacement, metres
        std::vector<float> shape[3];     // displacement along the strings of the focus note
        int shapeNodes[3] = {0, 0, 0};
        float hammerForce[3] = {0, 0, 0};
        int focusMidi = -1, awake = 0;
    };
    void prepareSnapshot(Snapshot& s) const;
    void fillSnapshot(Snapshot& s, int focusMidi) const;
    int stringMidi(int index) const { return notes_[strings_[index].note].midi; }
    Vec2 stringBridge(int index) const { return strings_[index].bridge; }

    // ---- diagnostics (not for the audio thread) ----
    double energy() const;
    double dt() const { return k_; }
    const Plate& plate() const { return *plate_; }
    const Radiation& radiation() const { return radiation_; }
    const std::vector<RealP>& plateDisplacement() const { return w_; }  // grid storage
    int stringCount() const { return (int)strings_.size(); }
    int awakeStrings() const;
    int totalStringNodes() const;
    int hammerIterationsMax() const { return hammerItersMax_; }
    bool hammerActive(int midi) const { int n = noteOfMidi_[midi & 127]; return n >= 0 && notes_[n].hammerActive; }

    struct StringView {
        int midi; int nodes; double h; const RealS* u; double hammerForce; bool awake; double tension;
        double tensionChange; int longitudinalCells;
    };
    StringView stringView(int index) const;
    std::vector<int> stringsOfNote(int midi) const;
    // Numerical partial frequency p of a string with rigid ends (closed form of the scheme).
    double discretePartial(int index, int p) const;
    // The same partial of the continuous stiff string the grid is approximating.
    double continuumPartial(int index, int p) const;
    int fitPartial(int index) const { return strings_[index].fitPartial; }
    // Test hooks: raw access to one string's displacement and per-step increment arrays.
    RealS* stringU(int index) { return &u_[strings_[index].off]; }
    RealS* stringV(int index) { return &v_[vb_][strings_[index].off]; }
    double stringNodeMass(int index) const { return strings_[index].mNode; }
    // External point force on the board for the next step only (measurement hook).
    void pushBoard(Vec2 pianoXY, double newtons) { extAt_ = plate_->interp(pianoXY); extF_ = newtons; }
    double boardVelocity(Vec2 pianoXY) const { return plate_->interp(pianoXY).eval(vw_.data()) / k_; }
    std::vector<RealP>& plateU() { return w_; }
    std::vector<RealP>& plateV() { return vw_; }

private:
    struct StringRT {
        int note = 0, N = 0, off = 0;
        double h = 0, aT = 0, aB = 0, aD = 0, s0k = 0, mNode = 0, z0 = 0, tension = 0;
        RealS cu0 = 0, cu1 = 0, cu2 = 0, cv0 = 0, cv1 = 0;  // stencil coefficients of the string update
        double T_h = 0, EI_h3 = 0, damp_h = 0;  // energy coefficients
        bool attached = true, awake = false, hasDamper = false;
        double hamW = 1;   // share of the hammer direction seen by this polarisation
        Vec2 bridge;
        bool sideways = false;  // the second polarisation of another string
        int hamL = 0; double hamG[2] = {0, 0}; double hamA = 0;
        int dmp0 = 0, dmp1 = 0; double dampRate = 0, dampTarget = 0;
        int dampedSteps = 0;   // steps spent fully damped
        double dampLand = 1;   // how fast the damper comes down, relative to ordinary
        double pressK = 0;     // dt^2 * resting damper force per node / node mass, at full engagement
        double endPeak = 0;    // recent size of the bridge motion at this string
        int fitPartial = 1;    // partial used to fit the grid dispersion
        double f0 = 0, inharm = 0;
        double zeta = 0, zetaPrev = 0, force = 0;
        // Longitudinal motion on its own coarse grid (its waves are about 15 times faster).
        static const int kLong = 24;
        int lCells = 0;            // 0: off for this string
        int lSpan = 0;             // transverse segments per longitudinal cell
        bool lOn = false;          // only strings that have been struck stretch enough to matter
        double lA = 0, lDamp = 0, lEA = 0, lBridge = 0, lPeak = 0;
        int lOff = 0;              // offset into the shared longitudinal state arrays
        double tensionChange = 0;  // at the bridge, N
    };
    struct NoteRT {
        int midi = 0, first = 0, count = 0;  // count contacts; each may have a second polarisation
        int total = 0;                       // strings owned, both polarisations
        bool twin = false;                   // string first+count+i is the sideways motion of first+i
        double hMass = 0, hK = 0, hP = 0, hMu = 0;
        double eta = 0, veta = 0;
        double sMass = 0, sK = 0, sC = 0, xi = 0, vxi = 0;  // shank: mass behind the spring, stiffness, damping, position, increment
        bool hammerActive = false, keyDown = false, hasDamper = false;
        double thump = 0; Interp thumpAt;
    };

    void wake(StringRT& s);
    void sleep(StringRT& s);
    void stepString(int index);
    void stepHammer(NoteRT& n);
    void housekeeping();
    struct Pool;
    std::unique_ptr<Pool> pool_;
    // Units of work that threads can take independently: the radiation task, ranges of
    // soundboard spans, then ranges of strings. Heavy ones come first.
    struct Chunk { int kind, from, to; };  // kind 0 radiation, 1 plate spans, 2 strings
    std::vector<Chunk> chunks_;
    std::vector<int> phaseChunks_[3];
    void runChunks(int phase);
    void chunkWork(int phase, int chunk);
    template <class T> double uEnd(const StringRT& s, const T* field) const { return s.attached ? endAt_[&s - strings_.data()].eval(field) : 0.0; }
    // What passes between strings and board each step, kept in dense arrays of their own.
    // Threads working on strings and the thread working on the board then exchange a handful
    // of cache lines instead of one or two per string.
    std::vector<Coupling> endAt_;                 // how each string's end follows the board (never changes)
    std::vector<char> endLive_;                   // 0: this string takes no part at the moment (sideways motion at rest)
    std::vector<double> endU_, endV_, endForce_;  // board displacement and increment at the string end; dt^2 * force back

    double k_ = 0, fs_ = 0, outGain_ = 0;
    int os_ = 1;
    std::unique_ptr<Plate> plate_;
    std::vector<StringRT> strings_;
    std::vector<NoteRT> notes_;
    int noteOfMidi_[128];
    double pedalDamping_ = 1.0;  // share of full damper strength the pedal position leaves on

    // All string nodes. Each string owns N+3 entries: a ghost node, nodes 0..N, a ghost node.
    // The per-step increments are double buffered (vb_ selects the current one).
    std::vector<RealS> u_, v_[2];
    std::vector<double> xi_, vxi_;  // longitudinal displacement and increment, kLong + 1 per stretching string
    int vb_ = 0;
    std::vector<RealP> w_, vw_, r_;      // plate displacement, increment, right hand side (grid storage)

    Radiation radiation_;
    // Radiation does not act back on the instrument, so it runs one step behind, alongside
    // the next step's string work: the sound of step n is computed during step n+1.
    std::vector<RealP> radIn_;
    std::vector<RealP> airNow_;  // air reaction per patch being applied this step
    double radL_ = 0, radR_ = 0;

    // Decimation when oversampling.
    std::vector<double> fir_, hist_[2];
    int histPos_ = 0;

    Interp extAt_; double extF_ = 0;
    // Key knocks in flight: short force pulses on the board.
    struct Pulse { Interp at; double peak; int step, length; };
    static const int kPulses = 32;
    Pulse pulses_[kPulses];
    int pulseCount_ = 0;
    void knock(const Interp& at, double impulse, double seconds);
    // Pedal work: where it is, how fast it is moving, and its friction noise.
    Interp pedalAt_;
    double pedalPos_ = 0, pedalSpeed_ = 0, pedalHiss_ = 0, mechNoise_ = 1;
    long pedalStep_ = 0;
    uint32_t rng_ = 0x2545F491u;
    long stepCount_ = 0;
    int quietChecks_ = 1 << 20;
    bool neverSleep_ = false;
    int hammerItersMax_ = 0;
    double dampMax_ = 1500.0;
};

}  // namespace pp
