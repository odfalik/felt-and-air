//! Experimental loss for action rail/backcheck contacts, NOT hammer/string felt.
//!
//! An instantly enabled dashpot produces a finite `C*v` force at first contact.
//! Growing contact participation instead gives C(z) = 2C/(1 + (z_ref/z)^p), z>0.
//! Thus force (and, for p>1, its onset slope) vanishes at zero compression. C retains
//! its existing units N s/m and matches its old value EXACTLY at z_ref; it is bounded
//! by 2C rather than growing without limit at large compression. K and p are unchanged.
//!
//! The reference compressions below are ASSUMPTIONS for this experiment, not measured
//! felt thicknesses or a calibrated material model. Using the elastic exponent for
//! engagement is also assumed. No additional state, delay, tone EQ or force-sign change.
//!
//! Continuous-law passivity: with elastic potential K*z_+^(p+1)/(p+1), the loss rate
//! is (F-K*z^p)*v = C(z)*v^2 >=0 while the contact force is positive. If unilateral
//! clipping makes F=0, v must be negative, so the loss rate is -K*z^p*v >=0 too.
//! This does NOT make the existing semi-implicit action integrator energy-exact, nor
//! fix/validate the existing one-way case-to-board transfer. Check timestep refinement
//! and full-engine noise-off equivalence/band energy before accepting the experiment.

/// Assumed compression where the existing 50 N s/m rail coefficient is retained.
pub(crate) const RAIL_REFERENCE: f64 = 0.00025;
/// Assumed compression where the existing 3 N s/m hammer-stop coefficient is retained.
pub(crate) const STOP_REFERENCE: f64 = 0.00020;

/// Positive compression, approach speed, K [N/m^p], exponent, C [N s/m], reference [m].
/// Physical domain: finite values, K>=0, C>=0, p>0, reference>0.
pub(crate) fn felt(z: f64, velocity: f64, k: f64, p: f64, drag: f64, reference: f64) -> f64 {
    if z <= 0.0 {
        return 0.0;
    }
    let engagement = 2.0 / (1.0 + (reference / z).powf(p));
    (k * z.powf(p) + drag * engagement * velocity).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAIL: (f64, f64, f64, f64) = (4.5e5, 1.5, 50.0, RAIL_REFERENCE);
    const STOP: (f64, f64, f64, f64) = (6.3e4, 1.5, 3.0, STOP_REFERENCE);

    #[test]
    fn contact_onset_and_reference_units() {
        for (k, p, c, reference) in [RAIL, STOP] {
            for v in [-1.0, 0.0, 0.1, 1.0] {
                assert_eq!(felt(0.0, v, k, p, c, reference), 0.0);
                assert_eq!(felt(-1e-6, v, k, p, c, reference), 0.0);
                let matched = felt(reference, v, k, p, c, reference);
                let old = (k * reference.powf(p) + c * v).max(0.0);
                assert_eq!(matched.to_bits(), old.to_bits(), "C must retain units/value at reference");
            }
            let mut previous = f64::INFINITY;
            for z in [1e-6, 1e-8, 1e-10, 1e-12] {
                let f = felt(z, 1.0, k, p, c, reference);
                println!("reference={reference:.6} m: approach 1 m/s, compression={z:.1e}, force={f:.9e} N (old onset {c} N)");
                assert!(f < previous);
                previous = f;
            }
            assert!(previous < 1e-8);
            let measured_c = (felt(reference, 0.2, k, p, c, reference)
                - felt(reference, 0.0, k, p, c, reference)) / 0.2;
            println!("reference C={measured_c:.6} N s/m, expected={c}");
            assert!((measured_c - c).abs() < 1e-12);
        }
    }

    #[test]
    fn contact_loss_is_nonnegative_and_bounded() {
        for (k, p, c, reference) in [RAIL, STOP] {
            for z in [1e-12, 1e-8, 1e-5, reference, 0.001, 0.01] {
                for v in [-5.0, -1.0, -0.1, 0.0, 0.1, 1.0, 5.0] {
                    let elastic = k * z.powf(p);
                    let force = felt(z, v, k, p, c, reference);
                    let loss = (force - elastic) * v;
                    assert!(force.is_finite() && force >= 0.0 && loss >= -1e-12);
                    if v > 0.0 {
                        assert!((force - elastic) / v <= 2.0 * c * (1.0 + 1e-12));
                    }
                    assert_eq!(felt(z, v, k, p, 0.0, reference), elastic);
                }
            }
            println!("nonnegative force/loss and damping <= {} N s/m across loading/unloading grid", 2.0 * c);
        }
    }

    // A scalar contact uses the SAME velocity-kick/position-drift ordering as step_action.
    // This tests the contact law/numerics, NOT the coupled engine, sound, or case transfer.
    fn stroke(config: (f64, f64, f64, f64), mass: f64, drive: f64, initial_v: f64, dt: f64) -> (f64, f64, f64) {
        let (k, p, c, reference) = config;
        let (mut z, mut v, mut work, mut loss, mut peak) = (-0.0008f64, initial_v, 0.0, 0.0, 0.0f64);
        let initial = 0.5 * mass * v * v;
        for _ in 0..(0.1 / dt).round() as usize {
            let force = felt(z, v, k, p, c, reference);
            let elastic = k * z.max(0.0).powf(p);
            let lost = (force - elastic) * v * dt;
            assert!(lost >= -1e-12);
            loss += lost;
            peak = peak.max(force);
            v += dt * (drive - force) / mass;
            let next = z + dt * v;
            work += drive * (next - z);
            z = next;
        }
        let energy = 0.5 * mass * v * v + k * z.max(0.0).powf(p + 1.0) / (p + 1.0);
        (energy + loss - initial - work, peak, initial + work)
    }

    #[test]
    fn scalar_energy_bookkeeping_converges_with_timestep() {
        for (config, mass, drive, speed) in [
            (RAIL, 0.1, 2.75, 0.4), // Lower rail, representative finger load.
            (RAIL, 0.1, 0.25, 0.212), // Upper rail, existing key upweight.
            (STOP, 0.00891, 0.0874, 0.8),
        ] {
            let mut errors = Vec::new();
            let mut peaks = Vec::new();
            for fs in [44100.0, 88200.0, 176400.0] {
                let (error, peak, work) = stroke(config, mass, drive, speed, 1.0 / fs);
                println!("scalar mass={mass} kg fs={fs}: peak={peak:.6} N, input={work:.9e} J, residual={error:.9e} J");
                assert!(error.abs() < 0.01 * work.abs());
                errors.push(error.abs());
                peaks.push(peak);
            }
            assert!(errors[1] < 0.7 * errors[0] && errors[2] < 0.7 * errors[1]);
            assert!((peaks[2] / peaks[0] - 1.0).abs() < 0.01);
        }
    }
}
