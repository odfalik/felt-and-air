// Soundboard: an orthotropic Kirchhoff plate on a grain-aligned grid, with ribs and bridges
// as glued-on beams. The discrete stiffness is assembled from a discrete strain energy, so
// it is symmetric positive semidefinite by construction for any outline and stiffener layout.
//
// Time stepping (done by Piano) is explicit for the bare plate and implicit (theta = 1/4)
// for the stiffeners, whose fast bending waves would otherwise dictate a coarse grid.
#pragma once
#include "Design.h"
#include <vector>

// Precision of the run time state. Setup, assembly and the factorisation are always double.
// The validation suite runs in double; single precision is an option for the realtime build
// and its added noise is measured against the double reference (see docs/MODEL.md).
#ifndef PP_PLATE_REAL
#define PP_PLATE_REAL double
#endif
#ifndef PP_STRING_REAL
#define PP_STRING_REAL double
#endif

namespace pp {

using RealP = PP_PLATE_REAL;   // soundboard state
using RealS = PP_STRING_REAL;  // string state

struct Csr {
    int n = 0;
    std::vector<int> rowPtr, col;
    std::vector<double> val;
    void mul(const double* x, double* y) const;       // y = A x
    double quad(const double* x) const;               // x' A x
};

struct Interp {  // bilinear interpolation onto plate nodes (grid storage index); dropped weights are zero
    int idx[4] = {0, 0, 0, 0};
    double w[4] = {0, 0, 0, 0};
    template <class T> double eval(const T* f) const {
        return w[0] * (double)f[idx[0]] + w[1] * (double)f[idx[1]] + w[2] * (double)f[idx[2]] + w[3] * (double)f[idx[3]];
    }
};

// Something bolted to the board at a point: lumped mass and a viscous dashpot that may be
// switched on at run time. Both enter the stability bound.
struct Attachment {
    // The attached point moves as sum(coefficient * board displacement at point).
    std::vector<std::pair<Vec2, double>> points;
    double mass = 0;
    double dashpot = 0;
};

// The same combination expressed on plate nodes (grid storage index).
struct Coupling {
    int n = 0;
    int idx[8] = {0};
    double w[8] = {0};
    template <class T> double eval(const T* f) const {
        double s = 0;
        for (int q = 0; q < n; q++) s += w[q] * (double)f[idx[q]];
        return s;
    }
};

class Plate {
public:
    // dt: time step. The grid spacing is chosen as fine as the stability bound allows.
    // gridScale >= 1 coarsens the grid beyond that limit.
    Plate(const BoardDesign& board, const std::vector<Attachment>& attachments, double dt, double gridScale = 1.0);

    int n = 0;                     // active nodes
    int Nx = 0, Ny = 0;
    double hx = 0, hy = 0;
    std::vector<int> nodeIndex;    // Nx*Ny -> active index or -1
    std::vector<int> gi, gj;       // active index -> grid coordinates

    // Run time fields live on the padded rectangular grid, stored column by column (the
    // across-grain direction is contiguous), with zeros at every node outside the board.
    int stride = 0, gridSize = 0;
    std::vector<int> gid;          // active index -> grid storage index
    struct Span { int start, len; };
    std::vector<Span> spans;       // contiguous runs of active nodes
    template <class T> void gather(const T* grid, double* active) const { for (int a = 0; a < n; a++) active[a] = grid[gid[a]]; }

    // r = -(dt^2 K w + dt D v + dt C0 v), the plate's internal force times dt^2. Allocation free.
    void apply(const RealP* w, const RealP* v, RealP* r) const;
    // The same in two parts, so the bulk can be shared between threads: the uniform stencil
    // on a range of spans (independent of every other range), then everything else, once.
    void applyStencil(const RealP* w, const RealP* v, RealP* r, int spanFrom, int spanTo) const;
    void applyExtras(const RealP* w, RealP* r) const;
    std::vector<double> mass;      // lumped mass per node, kg
    std::vector<double> c0;        // mass proportional damping per node, kg/s
    Csr K;                         // full stiffness, N/m
    Csr Kimp;                      // the part of K that is integrated implicitly
    Csr D;                         // stiffness-like damping, kg/s

    double stabilityBound = 0;     // Gershgorin upper bound on eig(M^-1 (dt^2 Kexp + 2 dt D)); < 4 is stable
    double stabilityEstimate = 0;  // power iteration estimate of the same quantity
    int stiffNodes = 0, factorNnz = 0;

    Interp interp(Vec2 piano) const;
    Coupling couple(const std::vector<std::pair<Vec2, double>>& points) const;
    Vec2 nodePosition(int node) const;  // piano coordinates
    std::vector<int> ribColumns;

    // A^-1 x with A = M + dt^2/4 Kimp + dt/2 C0, on grid storage, in two parts so that the
    // caller can fold the diagonal part into its own loop: solveStiff() overwrites the
    // entries of the stiffened block with the solution, and every entry must then be
    // multiplied by diagScale() (which is 1 on the stiffened block). Allocation free.
    void solveStiff(RealP* x) const;
    const RealP* diagScale() const { return invDiag_.data(); }
    int correctionNnz() const { return (int)corrCol_.size(); }

private:
    void build(const BoardDesign& board, const std::vector<Attachment>& att, double dt);
    void factor(double dt);
    Vec2 toGrid(Vec2 piano) const;
    Interp interpActive(Vec2 piano) const;  // same, but indexing active nodes
    bool inside(const BoardDesign& board, Vec2 piano) const;

    double ex_[2], ey_[2];         // grid axes in piano coordinates
    double gx0_ = 0, gy0_ = 0;
    std::vector<int> bridgeNodes_;
    double dt_ = 0;

    // Kernel data, all premultiplied by dt^2 (stiffness) or dt (damping).
    double a0_ = 0, a1x_ = 0, a1y_ = 0, a2x_ = 0, a2y_ = 0, a11_ = 0, b0_ = 0, bx_ = 0, by_ = 0;
    std::vector<RealP> c0dt_;                  // grid storage
    std::vector<int> corrRow_, corrPtr_, corrCol_;  // boundary correction to the uniform stencil
    std::vector<RealP> corrVal_;
    struct RibRun { int start, len, coef; };   // coef: offset into ribCoef_
    std::vector<RibRun> ribRuns_;
    std::vector<RealP> ribCoef_;
    struct BeamForm { int idx[12]; RealP c[12]; RealP coef; };
    std::vector<BeamForm> beams_;
    mutable std::vector<RealP> ribScr_;

    // Cholesky factor of the stiffened block of A. Rib segments that hang off a bridge are
    // eliminated first; each is a pentadiagonal chain, and all chains are swept together,
    // one position per step, so the sweep vectorises across ribs. What is left (the bridges
    // and their junctions with the ribs) is a small general sparse factor.
    std::vector<RealP> invDiag_;        // grid storage: 1/A_ii off the stiffened block, 1 on it, 0 outside the board
    int lanes_ = 0, levels_ = 0, slots_ = 0, coreN_ = 0;
    std::vector<RealP> f1_, f2_, fd_;   // forward sweep coefficients per slot
    std::vector<RealP> g1_, g2_, gd_;   // backward sweep coefficients per slot
    std::vector<int> chainSlot_, chainGid_;      // real chain nodes
    std::vector<int> virtSlot_, virtCore_;       // slots that mirror a core unknown
    std::vector<int> coreGid_;
    std::vector<int> cfPtr_, cfCol_;    // core rows of L (columns index the work vector)
    std::vector<RealP> cfVal_, cInvDiag_;
    std::vector<int> cbPtr_, cbRow_;    // core columns of L restricted to the core
    std::vector<RealP> cbVal_;
    mutable std::vector<RealP> work_;
};

}  // namespace pp
