// Soundboard: an orthotropic Kirchhoff plate on a grain-aligned grid, with ribs and bridges
// as glued-on beams. The discrete stiffness is assembled from a discrete strain energy, so
// it is symmetric positive semidefinite by construction for any outline and stiffener layout.
//
// Time stepping (done by Piano) is explicit for the bare plate and implicit (theta = 1/4)
// for the stiffeners, whose fast bending waves would otherwise dictate a coarse grid.
#pragma once
#include "Design.h"
#include <vector>

namespace pp {

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
    double eval(const double* f) const {
        return w[0] * f[idx[0]] + w[1] * f[idx[1]] + w[2] * f[idx[2]] + w[3] * f[idx[3]];
    }
};

// Something bolted to the board at a point: lumped mass and a viscous dashpot that may be
// switched on at run time. Both enter the stability bound.
struct Attachment { Vec2 pos; double mass = 0; double dashpot = 0; };

class Plate {
public:
    // dt: time step. The grid spacing is chosen as fine as the stability bound allows.
    Plate(const BoardDesign& board, const std::vector<Attachment>& attachments, double dt);

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
    void gather(const double* grid, double* active) const { for (int a = 0; a < n; a++) active[a] = grid[gid[a]]; }

    // r = -(dt^2 K w + dt D v + dt C0 v), the plate's internal force times dt^2. Allocation free.
    void apply(const double* w, const double* v, double* r) const;
    std::vector<double> mass;      // lumped mass per node, kg
    std::vector<double> c0;        // mass proportional damping per node, kg/s
    Csr K;                         // full stiffness, N/m
    Csr Kimp;                      // the part of K that is integrated implicitly
    Csr D;                         // stiffness-like damping, kg/s

    double stabilityBound = 0;     // Gershgorin upper bound on eig(M^-1 (dt^2 Kexp + 2 dt D)); < 4 is stable
    double stabilityEstimate = 0;  // power iteration estimate of the same quantity
    int stiffNodes = 0, factorNnz = 0;

    Interp interp(Vec2 piano) const;
    Vec2 nodePosition(int node) const;  // piano coordinates
    std::vector<int> ribColumns;

    // x <- A^-1 x with A = M + dt^2/4 Kimp + dt/2 C0, on grid storage. Allocation free.
    void solve(double* x) const;
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
    std::vector<double> c0dt_;                 // grid storage
    std::vector<int> corrRow_, corrPtr_, corrCol_;  // boundary correction to the uniform stencil
    std::vector<double> corrVal_;
    struct RibRun { int start, len, coef; };   // coef: offset into ribCoef_
    std::vector<RibRun> ribRuns_;
    std::vector<double> ribCoef_;
    struct BeamForm { int idx[12]; double c[12]; double coef; };
    std::vector<BeamForm> beams_;
    mutable std::vector<double> ribScr_;

    // Sparse Cholesky of the stiffened block of A, plus the plain diagonal elsewhere.
    std::vector<double> invDiag_;  // grid storage: 1/A_ii, zero outside the board
    std::vector<int> perm_;        // stiffened block: position -> grid storage index
    std::vector<int> lPtr_, lRow_;      // L by columns (backward substitution)
    std::vector<double> lVal_, lInvDiag_;
    std::vector<int> rPtr_, rCol_;      // L by rows (forward substitution)
    std::vector<double> rVal_;
    mutable std::vector<double> work_;
};

}  // namespace pp
