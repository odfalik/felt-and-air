// The coupled instrument: stiff strings, felt hammers, dampers, soundboard, radiation.
//
// All parts form one discrete mechanical system  M q'' + C q' + K q = f_hammer  with a
// symmetric K, stepped by a leapfrog scheme. The discrete energy returned by energy() is
// conserved to rounding error when all losses are switched off, and can only decrease
// otherwise. docs/MODEL.md has the derivation.
#pragma once
#include "Design.h"
#include "Plate.h"
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
    // onStart runs once on each helper thread (for example to join an audio workgroup).
    void setHelperThreads(int count, std::function<void()> onStart = {});
    bool helpersBusy() const;

    // Control. Call from the same thread as process(); none of these allocate.
    void noteOn(int midi, double hammerSpeed);
    void noteOff(int midi);
    void setSustain(bool down);

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
    const std::vector<RealP>& plateDisplacement() const { return w_; }  // grid storage
    int stringCount() const { return (int)strings_.size(); }
    int awakeStrings() const;
    int totalStringNodes() const;
    int hammerIterationsMax() const { return hammerItersMax_; }
    bool hammerActive(int midi) const { int n = noteOfMidi_[midi & 127]; return n >= 0 && notes_[n].hammerActive; }

    struct StringView {
        int midi; int nodes; double h; const RealS* u; double hammerForce; bool awake; double tension;
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
        double T_h = 0, EI_h3 = 0, damp_h = 0;  // energy coefficients
        bool attached = true, awake = false, hasDamper = false;
        double hamW = 1;   // share of the hammer direction seen by this polarisation
        Vec2 bridge;
        Interp at;
        int hamL = 0; double hamG[2] = {0, 0}; double hamA = 0;
        int dmp0 = 0, dmp1 = 0; double dampRate = 0, dampTarget = 0;
        int dampedSteps = 0;   // steps spent fully damped
        int fitPartial = 1;    // partial used to fit the grid dispersion
        double f0 = 0, inharm = 0;
        double zeta = 0, zetaPrev = 0, force = 0;
        double endForce = 0;  // dt^2 * force on the bridge from the latest step
    };
    struct NoteRT {
        int midi = 0, first = 0, count = 0;  // count contacts; each may have a second polarisation
        int total = 0;                       // strings owned, both polarisations
        bool twin = false;                   // string first+count+i is the sideways motion of first+i
        double hMass = 0, hK = 0, hP = 0, hMu = 0;
        double eta = 0, veta = 0;
        bool hammerActive = false, keyDown = false, hasDamper = false;
    };

    void wake(StringRT& s);
    void sleep(StringRT& s);
    void stepString(StringRT& s);
    void stepHammer(NoteRT& n);
    void housekeeping();
    struct Pool;
    std::unique_ptr<Pool> pool_;
    std::vector<std::pair<int, int>> chunks_;  // string index ranges of similar size
    void runChunks(int phase);
    void chunkWork(int phase, int chunk);
    template <class T> double uEnd(const StringRT& s, const T* field) const { return s.attached ? s.at.eval(field) : 0.0; }

    double k_ = 0, fs_ = 0, outGain_ = 0;
    int os_ = 1;
    std::unique_ptr<Plate> plate_;
    std::vector<StringRT> strings_;
    std::vector<NoteRT> notes_;
    int noteOfMidi_[128];
    bool sustain_ = false;

    // All string nodes. Each string owns N+3 entries: a ghost node, nodes 0..N, a ghost node.
    // The per-step increments are double buffered (vb_ selects the current one).
    std::vector<RealS> u_, v_[2];
    int vb_ = 0;
    std::vector<RealP> w_, vw_, r_;      // plate displacement, increment, right hand side (grid storage)

    // Radiation (Rayleigh integral): nodes are grouped by their delay to each ear, so one
    // step is a short dot product per delay bin.
    std::vector<int> tapIdx_[2], binPtr_[2];
    std::vector<RealP> tapGain_[2];
    int binFirst_[2] = {0, 0};
    std::vector<double> ring_[2];
    int ringPos_ = 0, ringMask_ = 0;

    // Decimation when oversampling.
    std::vector<double> fir_, hist_[2];
    int histPos_ = 0;

    Interp extAt_; double extF_ = 0;
    long stepCount_ = 0;
    int hammerItersMax_ = 0;
    double dampMax_ = 1500.0;
};

}  // namespace pp
