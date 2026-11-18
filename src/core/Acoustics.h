// From board motion to sound at the listener's ears.
//
// Each side of the soundboard radiates into its own half space (Rayleigh integral), the top
// side with one sign and the underside with the opposite one. Sound then reaches each ear:
//   - directly, summed node by node;
//   - by way of the floor, the open lid, the walls and the ceiling, as mirror images up to
//     second order, summed over patches of the board;
//   - as a late reverberant field, from a feedback delay network whose decay time and level
//     follow Sabine's formula for the same room.
// The first two are geometry; the third is a statistical stand-in, not a simulation.
// Diffraction around the case and the lid's shadow are not modelled.
#pragma once
#include "Design.h"
#include "Plate.h"
#include <vector>

namespace pp {

class Radiation {
public:
    void build(const Plate& plate, const PianoDesign& design, double sampleRate);
    // r: change of the plate increment this step (dt^2 * acceleration), grid storage.
    void step(const RealP* r, double& left, double& right);

    double reverbTime = 0;       // Sabine, seconds (0 when there is no room)

    // Reaction of the air on the board, per patch, as dt^2 * force on each node of the patch,
    // from the motion up to the step last passed to step(). Empty when air loading is off.
    const std::vector<RealP>& airForce() const { return airForce_; }
    const std::vector<int>& patchPtr() const { return patchPtr_; }
    const std::vector<int>& patchNode() const { return patchNode_; }
    int imageCount = 0, patchCount = 0;

private:
    struct Bins {  // sources grouped by their delay to one ear
        int first = 0;
        std::vector<int> ptr, idx;
        std::vector<RealP> gain;
        void build(const std::vector<int>& delay, const std::vector<int>& source, const std::vector<double>& g);
    };
    Bins node_[2];
    std::vector<int> imgDelay_[2], imgPatch_[2];  // reflections: one entry per patch and mirror image
    std::vector<RealP> imgGain_[2];
    std::vector<int> patchPtr_, patchNode_;
    std::vector<RealP> patchAcc_;
    std::vector<double> ring_[2];
    int ringPos_ = 0, ringMask_ = 0;

    // Air loading: each patch feels its own motion (a piston: pressure rho c [v(t) - v(t - a/c)]
    // at its centre, which is mass-like at low and resistive at high frequency) and the delayed
    // acceleration of every other patch. Both faces of the board count.
    bool air_ = false;
    std::vector<RealP> airForce_;
    std::vector<double> patchVel_, velHist_, accHist_, invCount_;
    std::vector<int> selfDelay_, mutDelay_, mutSrc_, mutPtr_;
    std::vector<double> mutGain_;
    double selfGain_ = 0;
    int histPos_ = 0, histMask_ = 0;

    // Late field.
    static const int kLines = 8;
    bool reverb_ = false;
    std::vector<double> line_[kLines];
    int linePos_[kLines] = {0}, lineLen_[kLines] = {0};
    double lineGain_[kLines] = {0}, lineLp_[kLines] = {0};
    double lpCoef_ = 0, reverbGain_ = 0, hpState_ = 0, hpCoef_ = 0, feed_ = 0;
    void reverbTick(double in, double& l, double& r);
};

}  // namespace pp
