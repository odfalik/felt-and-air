//! Validation suite. Each test checks a physical or numerical property of the model and
//! prints the measured numbers, so the output doubles as a report:
//!   cargo test --release -- --nocapture --test-threads=1

use piano_engine::acoustics::Radiation;
use piano_engine::design::*;
use piano_engine::util::peak_frequency;
use piano_engine::{Piano, PianoOptions};
use std::f64::consts::PI;

fn options(oversample: usize) -> PianoOptions {
    PianoOptions { oversample, ..PianoOptions::default() }
}

fn one_note(key: i32) -> PianoDesign {
    make_grand(&GrandOptions { first_key: key, last_key: key, ..GrandOptions::default() })
}

fn make_lossless(d: &mut PianoDesign) {
    d.longitudinal = false; // driven one-way by the transverse motion, so not part of the energy balance
    d.air_loading = false; // the air carries energy away
    d.damper_press = 0.0; // outside forces
    d.mechanism_noise = 0.0;
    d.action.let_off = 1e-5; // start at the string, with no gravity doing work on the way
    d.action.gravity = false;
    d.board.loss_factor = 0.0;
    d.board.sigma0 = 0.0;
    for n in &mut d.notes {
        n.hammer_mu = 0.0;
        n.shank_loss = 0.0;
        n.key_thump = 0.0; // an outside force: it would add energy
        for s in &mut n.strings {
            s.sigma0 = 0.0;
            s.sigma1 = 0.0;
        }
    }
}

macro_rules! check {
    ($cond:expr, $($arg:tt)*) => {{
        let ok = $cond;
        println!("    [{}] {}", if ok { "ok" } else { "FAIL" }, format!($($arg)*));
        assert!(ok, $($arg)*);
    }};
}

#[test]
fn t01_kernel_matches_assembly() {
    println!("\n1. Fast plate kernel reproduces the assembled energy operator");
    let p = Piano::new(&one_note(40), &options(1)).unwrap();
    let pl = p.plate();
    let mut seed = 12345u32;
    let mut rnd = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f64 / 16777216.0 - 0.5
    };
    let (mut w, mut v, mut r) = (vec![0.0; pl.grid_size], vec![0.0; pl.grid_size], vec![0.0; pl.grid_size]);
    for a in 0..pl.n {
        w[pl.gid[a]] = rnd();
        v[pl.gid[a]] = rnd();
    }
    let (mut wa, mut va, mut kw, mut dv) = (vec![0.0; pl.n], vec![0.0; pl.n], vec![0.0; pl.n], vec![0.0; pl.n]);
    pl.gather(&w, &mut wa);
    pl.gather(&v, &mut va);
    pl.apply(&w, &v, &mut r);
    pl.k.mul(&wa, &mut kw);
    pl.d.mul(&va, &mut dv);
    let k = p.dt();
    let (mut err, mut reference) = (0.0f64, 0.0f64);
    for a in 0..pl.n {
        let expect = -(k * k * kw[a] + k * dv[a] + k * pl.c0[a] * va[a]);
        err = err.max((expect - r[pl.gid[a]]).abs());
        reference = reference.max(expect.abs());
    }
    check!(err < 1e-9 * reference, "max relative difference {:.2e}", err / reference);
    // Symmetry of K by random probes.
    let x: Vec<f64> = (0..pl.n).map(|_| rnd()).collect();
    let y: Vec<f64> = (0..pl.n).map(|_| rnd()).collect();
    let (mut kx, mut ky) = (vec![0.0; pl.n], vec![0.0; pl.n]);
    pl.k.mul(&x, &mut kx);
    pl.k.mul(&y, &mut ky);
    let xy: f64 = (0..pl.n).map(|a| x[a] * ky[a]).sum();
    let yx: f64 = (0..pl.n).map(|a| y[a] * kx[a]).sum();
    check!((xy - yx).abs() < 1e-10 * xy.abs(), "K symmetric: x'Ky - y'Kx relative {:.2e}", (xy - yx).abs() / xy.abs());
    check!(pl.stability_bound < 4.0, "stability bound {:.4} < 4 (rigorous), power-iteration estimate {:.4}", pl.stability_bound, pl.stability_estimate);
    println!(
        "    grid: {} nodes, {:.1} x {:.1} mm, {} stiffener nodes, factor fill {}, rim correction {} entries",
        pl.n,
        pl.hx * 1e3,
        pl.hy * 1e3,
        pl.stiff_nodes,
        pl.factor_nnz,
        pl.correction_nnz()
    );
}

#[test]
fn t02_energy_conservation() {
    println!("\n2. Lossless instrument conserves the discrete energy (strike, contact, shank, coupling)");
    let mut d = one_note(40);
    make_lossless(&mut d);
    let mut p = Piano::new(&d, &options(1)).unwrap();
    p.note_on(60, 3.0);
    let e0 = p.energy();
    let (mut worst, mut peak_force, mut n) = (0.0f64, 0.0f64, 0);
    while n < 24000 {
        p.step();
        if !p.hammer_active(60) {
            break; // the hammer is dropped from the system once it has left
        }
        peak_force = peak_force.max(p.hammer_force(0));
        worst = worst.max((p.energy() - e0).abs() / e0);
        n += 1;
    }
    println!("    hammer in play for {:.2} ms", n as f64 / 48.0);
    check!(worst < 1e-9, "relative energy drift {:.2e} through the strike (peak felt force {:.1} N)", worst, peak_force);
    check!(p.hammer_iterations_max() < 20, "contact solver: at most {} Newton iterations", p.hammer_iterations_max());
}

#[test]
fn t03_energy_after_release() {
    println!("\n3. Lossless free vibration after the hammer leaves");
    let mut d = one_note(40);
    make_lossless(&mut d);
    let mut p = Piano::new(&d, &options(1)).unwrap();
    p.note_on(60, 3.0);
    for _ in 0..4800 {
        p.step();
    }
    let e0 = p.energy();
    let mut worst = 0.0f64;
    for n in 0..96000 {
        p.step();
        if n % 64 == 0 {
            worst = worst.max((p.energy() - e0).abs() / e0);
        }
    }
    check!(worst < 1e-9, "relative drift {:.2e} over 2 s ({:.3} mJ in strings and board)", worst, e0 * 1e3);
}

#[test]
fn t04_passivity() {
    println!("\n4. Lossy instrument: energy never increases (strike, hold, damper)");
    let mut d = one_note(40);
    d.notes[0].key_thump = 0.0; // the key's landing is an outside force and would add energy
    d.longitudinal = false; // one-way drive, outside the energy balance
    d.air_loading = false; // acts through delays, outside the step-by-step energy argument
    d.damper_press = 0.0; // the damper's weight is an outside force
    d.mechanism_noise = 0.0;
    d.action.gravity = false;
    d.damped_strings_ring = false; // so that the sleep check below applies
    let mut p = Piano::new(&d, &options(1)).unwrap();
    p.note_on(60, 4.0);
    let e0 = p.energy();
    let (mut prev, mut worst_rise, mut n) = (e0, 0.0f64, 0);
    while n < 48000 {
        if n == 24000 {
            p.note_off(60, 1.0);
        }
        p.step();
        if p.awake_strings() == 0 {
            break;
        }
        let e = p.energy();
        worst_rise = worst_rise.max((e - prev) / e0);
        prev = e;
        n += 1;
    }
    check!(worst_rise < 1e-12, "largest step-to-step rise {:.2e} of the initial energy", worst_rise);
    check!(p.awake_strings() == 0, "damped strings went to sleep {:.0} ms after key release", (n as f64 - 24000.0) / 48.0);
}

#[test]
fn t05_plate_analytic() {
    println!("\n5. Orthotropic plate against the analytic simply supported rectangle");
    let mut d = one_note(40);
    make_lossless(&mut d);
    d.board.outline = [(0.0, 0.0), (1.2, 0.0), (1.2, 0.8), (0.0, 0.8)].iter().map(|&(x, y)| Vec2 { x, y }).collect();
    d.board.grain_angle = 0.0;
    d.board.ribs = false;
    d.board.crown_radius = 0.0;
    d.board.rim_clamp = 0.0;
    d.board.bridges.clear();
    for s in &mut d.notes[0].strings {
        s.attached = false;
    }
    for os in [1usize, 2] {
        let mut p = Piano::new(&d, &options(os)).unwrap();
        let (n, hx, hy, gi, gj, gid) = {
            let pl = p.plate();
            (pl.n, pl.hx, pl.hy, pl.gi.clone(), pl.gj.clone(), pl.gid.clone())
        };
        let (i0, i1) = (*gi.iter().min().unwrap(), *gi.iter().max().unwrap());
        let (j0, j1) = (*gj.iter().min().unwrap(), *gj.iter().max().unwrap());
        // supports sit one spacing outside
        let (la, lb) = ((i1 - i0 + 2) as f64 * hx, (j1 - j0 + 2) as f64 * hy);
        let bd = &d.board;
        let h = bd.thickness;
        let nuyx = bd.nuxy * bd.ey / bd.ex;
        let den = 12.0 * (1.0 - bd.nuxy * nuyx);
        let (dx, dy, dk) = (bd.ex * h * h * h / den, bd.ey * h * h * h / den, bd.gxy * h * h * h / 12.0);
        let d1 = nuyx * dx;
        let mut worst = 0.0f64;
        for (m, q) in [(1, 1), (2, 1), (1, 2), (3, 2), (2, 5)] {
            let (mf, qf) = (m as f64, q as f64);
            let exact = 0.5 * PI * ((dx * (mf / la).powi(4) + 2.0 * (d1 + 2.0 * dk) * (mf * qf / (la * lb)).powi(2) + dy * (qf / lb).powi(4)) / (bd.rho * h)).sqrt();
            p.plate_v_mut().fill(0.0);
            let mut probe = 0;
            {
                let u = p.plate_u_mut();
                u.fill(0.0);
                for a in 0..n {
                    let sx = (mf * PI * (gi[a] - i0 + 1) as f64 * hx / la).sin();
                    let sy = (qf * PI * (gj[a] - j0 + 1) as f64 * hy / lb).sin();
                    u[gid[a]] = 1e-6 * sx * sy;
                    if (sx * sy).abs() > 0.9 {
                        probe = gid[a];
                    }
                }
            }
            // Count zero crossings of the probe node.
            let mut prev = p.plate_u()[probe];
            let (mut first, mut last, mut crossings) = (-1.0, -1.0, 0);
            let steps = (0.4 / p.dt()) as usize;
            for s in 0..steps {
                p.step();
                let cur = p.plate_u()[probe];
                if (prev > 0.0) != (cur > 0.0) {
                    let t = (s as f64 + prev / (prev - cur)) * p.dt();
                    if first < 0.0 {
                        first = t;
                    }
                    last = t;
                    crossings += 1;
                }
                prev = cur;
            }
            let f = 0.5 * (crossings - 1) as f64 / (last - first);
            let err = 100.0 * (f - exact) / exact;
            worst = worst.max(err.abs());
            println!("      mode ({m},{q}): analytic {exact:8.2} Hz, simulated {f:8.2} Hz ({err:+.3} %)");
        }
        check!(worst < if os == 1 { 1.5 } else { 0.8 }, "oversampling {os} ({n} nodes): worst error {worst:.3} %");
    }
}

#[test]
fn t06_string_partials() {
    println!("\n6. Stiff string partials (rigid terminations)");
    for key in [16, 40, 64] {
        let mut d = one_note(key);
        make_lossless(&mut d);
        d.notes[0].strings.truncate(1);
        d.notes[0].duplex.clear();
        d.notes[0].strings[0].attached = false;
        let sd = d.notes[0].strings[0].clone();
        let midi = d.notes[0].midi;
        let mut p = Piano::new(&d, &options(1)).unwrap();
        p.note_on(midi, 2.0);
        let nodes = p.string_nodes(0);
        let node = ((0.31 * nodes as f64) as usize).max(1);
        let rec: Vec<f64> = (0..96000)
            .map(|_| {
                p.step();
                p.string_u(0)[node]
            })
            .collect();
        let b = PI * PI * sd.ei / (sd.tension * sd.length * sd.length);
        println!("      key {key} ({:.1} Hz), {nodes} nodes, inharmonicity B = {b:.2e}, dispersion fitted at partial {}", sd.f0, p.fit_partial(0));
        let mut worst = 0.0f64;
        for q in [1, 2, 3, 5, 8, 12, 16, 24, 32] {
            let pred = p.discrete_partial(0, q);
            if pred <= 0.0 || pred > 20000.0 {
                continue;
            }
            let meas = peak_frequency(&rec, 48000.0, pred * 0.985, pred * 1.015);
            let cents = 1200.0 * (meas / pred).log2();
            let ideal = p.continuum_partial(0, q);
            worst = worst.max(cents.abs());
            println!(
                "        partial {q:2}: scheme {pred:9.3} Hz, simulated {meas:9.3} Hz ({cents:+.2} c), continuum {ideal:9.3} Hz (grid error {:+.1} c)",
                1200.0 * (pred / ideal).log2()
            );
        }
        check!(worst < 0.5, "simulation matches the scheme's closed form within {worst:.2} cents");
        check!((p.discrete_partial(0, 1) - sd.f0).abs() < 1e-6, "fundamental tuned to {:.4} Hz", p.discrete_partial(0, 1));
    }
}

#[test]
fn t07_hammer() {
    println!("\n7. Hammer felt contact: pulse shape and time step convergence");
    let mut reference = 0.0;
    for os in [1usize, 2, 4] {
        let mut p = Piano::new(&one_note(40), &options(os)).unwrap();
        p.note_on(60, 2.0);
        let (mut peak, mut impulse, mut first, mut last) = (0.0f64, 0.0, -1i64, -1i64);
        let steps = (0.012 / p.dt()) as i64;
        for n in 0..steps {
            p.step();
            let f = p.hammer_force(1);
            peak = peak.max(f);
            impulse += f * p.dt();
            if f > 0.0 {
                if first < 0 {
                    first = n;
                }
                last = n;
            }
        }
        let dur = (last - first + 1) as f64 * p.dt() * 1e3;
        println!(
            "      oversampling {os}: strikes {:.2} ms after let-off, peak {peak:.3} N, contact {dur:.3} ms, impulse {:.3} mN s, Newton max {}",
            first as f64 * p.dt() * 1e3,
            impulse * 1e3,
            p.hammer_iterations_max()
        );
        if os == 1 {
            check!(peak > 3.0 && peak < 40.0, "peak force per string plausible for mezzo forte C4");
            check!(dur > 0.5 && dur < 5.0, "contact duration plausible");
        } else if os == 2 {
            check!((peak - reference).abs() / reference < 0.1, "peak force changes {:.1} % from 48 to 96 kHz", 100.0 * (peak - reference).abs() / reference);
        }
        reference = peak;
    }
}

#[test]
fn t08_reciprocity() {
    println!("\n8. Reciprocity between two strings through bridge and soundboard");
    let mut d = one_note(40);
    make_lossless(&mut d);
    let mut resp = [0.0; 2];
    for dir in 0..2 {
        let mut p = Piano::new(&d, &options(1)).unwrap();
        p.set_sustain(true); // lifts the dampers without a hammer
        for _ in 0..6000 {
            p.step(); // let the dampers finish lifting
        }
        let (from, to, nf, nt) = if dir == 0 { (0, 2, 20, 41) } else { (2, 0, 41, 20) };
        let m = p.string_node_mass(from);
        p.string_v_mut(from)[nf] = 1e-9 / m; // unit momentum
        let mut acc = 0.0;
        for n in 0..6000 {
            p.step();
            if n % 500 == 499 {
                acc += p.string_u(to)[nt] * (1 + n) as f64;
            }
        }
        resp[dir] = acc;
    }
    let rel = (resp[0] - resp[1]).abs() / resp[0].abs();
    check!(rel < 1e-6, "string 0 -> string 2 response equals 2 -> 0 within {rel:.2e}");
    check!(resp[0].abs() > 0.0, "strings are coupled (transfer {:.3e})", resp[0]);
}

#[test]
fn t09_coupling() {
    println!("\n9. Two-way bridge coupling changes the string decay");
    let mut level = [0.0; 2];
    for attached in 0..2 {
        let mut d = one_note(40);
        for s in &mut d.notes[0].strings {
            s.attached = attached == 1;
        }
        d.notes[0].duplex.clear();
        let mut p = Piano::new(&d, &options(1)).unwrap();
        p.note_on(60, 2.0);
        let (mut early, mut late) = (0.0, 0.0);
        for n in 0..96000 {
            p.step();
            let u = p.string_u(1)[30];
            if (9600..19200).contains(&n) {
                early += u * u;
            }
            if n >= 86400 {
                late += u * u;
            }
        }
        level[attached] = 10.0 * (late / early).log10();
        println!("      {}: string level changes {:.2} dB between 0.3 s and 1.9 s", if attached == 1 { "on the bridge " } else { "rigid supports" }, level[attached]);
    }
    check!(level[1] < level[0] - 1.0, "the soundboard drains the strings ({:.1} dB extra decay)", level[0] - level[1]);
}

#[test]
fn t10_robustness() {
    println!("\n10. Robustness: fortissimo repeated strikes across the compass");
    let mut p = Piano::new(&make_grand(&GrandOptions::default()), &options(1)).unwrap();
    println!("      full compass: {} strings, {} string nodes, {} board nodes", p.string_count(), p.total_string_nodes(), p.plate().n);
    let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
    let (mut peak, mut finite) = (0.0f32, true);
    let keys = [21, 22, 36, 48, 60, 72, 84, 96, 107, 108];
    for block in 0..1500 {
        if block % 40 == 0 {
            keys.iter().for_each(|&k| p.note_on(k, 6.0));
        }
        if block % 40 == 30 {
            keys.iter().for_each(|&k| p.note_off(k, 1.0));
        }
        if block == 600 {
            p.set_sustain(true);
        }
        if block == 900 {
            p.set_soft_pedal(true);
            p.set_sostenuto(true);
        }
        if block == 1200 {
            p.set_sustain(false);
            p.set_sostenuto(false);
        }
        p.process(&mut l, &mut r);
        for x in &l {
            finite = finite && x.is_finite();
            peak = peak.max(x.abs());
        }
    }
    check!(finite, "output finite through 2 s of hammering with all three pedals");
    check!(peak > 1e-4 && peak < 100.0, "peak output {peak:.3} (linear, before any limiter)");
    check!(p.hammer_iterations_max() < 40, "contact solver: at most {} Newton iterations", p.hammer_iterations_max());
    let e = p.energy();
    check!(e.is_finite() && e < 10.0, "stored energy {e:.4} J stays bounded");
}

#[test]
fn t11_threads() {
    println!("\n11. Helper threads give bit-identical output");
    let mut out: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
    for (which, helpers) in [0usize, 3].into_iter().enumerate() {
        let mut p = Piano::new(&make_grand(&GrandOptions::default()), &options(1)).unwrap();
        p.set_helper_threads(helpers, None, None);
        let (mut l, mut r) = (vec![0.0f32; 64], vec![0.0f32; 64]);
        let mut shared = false;
        p.set_sustain(true);
        for block in 0..300 {
            if block % 25 == 0 {
                p.note_on(30 + (block / 25 * 7) % 60, 3.0);
            }
            p.process(&mut l, &mut r);
            shared = shared || p.helpers_busy();
            out[which].extend_from_slice(&l);
        }
        if helpers > 0 {
            check!(shared, "helpers were sharing the work");
        }
    }
    let diff = out[0].iter().zip(&out[1]).filter(|(a, b)| a != b).count();
    check!(diff == 0, "{diff} of {} samples differ between 0 and 3 helpers", out[0].len());
}

#[test]
fn t12_room() {
    println!("\n12. Room: arrival time, mirror images, reverberant decay");
    let d = one_note(40);
    let p = Piano::new(&d, &options(1)).unwrap();
    let pl = p.plate();
    let it = pl.interp(d.notes[0].strings[0].bridge);
    let node = (0..4).max_by(|&a, &b| it.w[a].partial_cmp(&it.w[b]).unwrap()).map(|q| it.idx[q]).unwrap();
    let a = (0..pl.n).find(|&i| pl.gid[i] == node).unwrap();
    let pos = pl.node_position(a);
    let response = |dd: &PianoDesign, steps: usize| {
        let mut rad = Radiation::new(pl, dd, 48000.0);
        let mut r = vec![0.0; pl.grid_size];
        let out: Vec<f64> = (0..steps)
            .map(|n| {
                r[node] = if n == 0 { 1.0 } else { 0.0 };
                rad.step(&r).0
            })
            .collect();
        (out, rad)
    };
    let mut dry = d.clone();
    dry.room.enabled = false;
    dry.lid.open = false;
    dry.air_loading = false;
    let (h0, _) = response(&dry, 4800);
    let first = h0.iter().position(|&x| x != 0.0).unwrap();
    let ear = d.ears[0];
    let dist = ((pos.x - ear.x).powi(2) + (pos.y - ear.y).powi(2) + ear.z * ear.z).sqrt();
    check!((first as f64 - dist / 343.0 * 48000.0).abs() <= 1.0, "direct sound arrives after {:.2} ms for a {dist:.2} m path", first as f64 / 48.0);

    let (h, rad) = response(&d, 48000 * 3);
    println!("      {} mirror images over {} board patches; Sabine reverberation time {:.2} s", rad.image_count, rad.patch_count, rad.reverb_time);
    let direct = h0[first] * h0[first];
    let split = first + (0.08 * 48000.0) as usize;
    let early: f64 = h[..split].iter().map(|x| x * x).sum();
    let late: f64 = h[split..].iter().map(|x| x * x).sum();
    println!("      energy relative to the direct sound: first 80 ms {:+.1} dB, later {:+.1} dB", 10.0 * (early / direct).log10(), 10.0 * (late / direct).log10());
    // Decay time from the backward integrated tail, fitted between -5 and -25 dB.
    let mut sch = vec![0.0; h.len()];
    let mut acc = 0.0;
    for n in (0..h.len()).rev() {
        acc += h[n] * h[n];
        sch[n] = acc;
    }
    let db = |n: usize| 10.0 * (sch[n] / sch[split]).log10();
    let n5 = (split..h.len()).find(|&n| db(n) <= -5.0).unwrap();
    let n25 = (n5..h.len()).find(|&n| db(n) <= -25.0).unwrap();
    let t60 = 3.0 * (n25 - n5) as f64 / 48000.0;
    check!((t60 - rad.reverb_time).abs() < 0.2 * rad.reverb_time, "measured decay time {t60:.2} s against Sabine {:.2} s", rad.reverb_time);
    check!(late > 0.0 && late < 30.0 * direct, "reverberant level is finite and plausible");
}

#[test]
fn t13_longitudinal() {
    println!("\n13. Longitudinal string motion: phantom partial and longitudinal mode");
    let d = make_grand(&GrandOptions { first_key: 28, last_key: 28, two_polarisations: false, ..GrandOptions::default() });
    let mut p = Piano::new(&d, &options(1)).unwrap();
    p.note_on(d.notes[0].midi, 4.5);
    let mut cells = 0;
    let dt: Vec<f64> = (0..1 << 16)
        .map(|_| {
            p.step();
            cells = cells.max(p.longitudinal_cells(0));
            p.tension_change(0)
        })
        .collect();
    let peak = dt.iter().fold(0.0f64, |m, x| m.max(x.abs()));
    let sd = &d.notes[0].strings[0];
    let f_long = (sd.ea / sd.rho_a).sqrt() / (2.0 * sd.length);
    let phantom = peak_frequency(&dt, 48000.0, 1.9 * sd.f0, 2.1 * sd.f0);
    let mode = peak_frequency(&dt, 48000.0, 0.9 * f_long, 1.1 * f_long);
    println!("      {cells} longitudinal cells; tension swings {peak:.1} N around {:.0} N at fortissimo", p.string_tension(0));
    check!((phantom / (2.0 * sd.f0) - 1.0).abs() < 0.01, "tension pulses at twice the fundamental: {phantom:.1} Hz against {:.1} Hz", 2.0 * sd.f0);
    check!((mode / f_long - 1.0).abs() < 0.03, "longitudinal mode at {mode:.0} Hz against c_L / 2L = {f_long:.0} Hz");
    check!(peak > 0.5 && peak < 0.2 * p.string_tension(0), "tension change is a small fraction of the tension");
}

#[test]
fn t14_air_loading() {
    println!("\n14. Air loading: the air's mass lowers the board's resonances, and it stays stable");
    let mut first = [0.0; 2];
    for air in 0..2 {
        let d = make_grand(&GrandOptions { first_key: 40, last_key: 40, air_loading: air == 1, ..GrandOptions::default() });
        let mut p = Piano::new(&d, &options(1)).unwrap();
        let at = d.notes[0].strings[0].bridge;
        p.push_board(at, 1.0);
        let v: Vec<f64> = (0..1 << 16)
            .map(|_| {
                p.step();
                p.board_velocity(at)
            })
            .collect();
        first[air] = peak_frequency(&v, 48000.0, 30.0, 60.0);
    }
    let shift = 100.0 * (first[1] / first[0] - 1.0);
    check!(shift < -2.0 && shift > -20.0, "lowest resonance {:.1} Hz in vacuum, {:.1} Hz in air ({shift:.1} %)", first[0], first[1]);
    let mut p = Piano::new(&make_grand(&GrandOptions::default()), &options(1)).unwrap();
    p.set_sustain(true);
    let (mut early, mut late) = (0.0, 0.0);
    for i in 0..48000 * 6 {
        if i % 6000 == 0 && i < 48000 * 2 {
            p.note_on(24 + (i / 6000 * 7) % 80, 5.0);
        }
        let (a, _) = p.step();
        if i > 48000 * 2 && i < 48000 * 3 {
            early += a * a;
        }
        if i > 48000 * 5 {
            late += a * a;
        }
    }
    check!(late.is_finite() && late < early, "fortissimo with pedal rings down: last second {:.1} dB below the third", 10.0 * (early / late).log10());
}

#[test]
fn t15_action() {
    println!("\n15. Action after let-off: flight time, key on its felt, hammer caught and released");
    for (speed, lo, hi) in [(0.3, 5.0, 12.0), (2.0, 0.8, 1.3), (5.0, 0.3, 0.6)] {
        let mut p = Piano::new(&one_note(40), &options(1)).unwrap();
        for _ in 0..4800 {
            p.step();
        }
        p.note_on(60, speed);
        let mut strike = None;
        let mut finite = true;
        for i in 0..48000 {
            if i == 24000 {
                p.note_off(60, 1.0);
            }
            let (a, _) = p.step();
            finite = finite && a.is_finite();
            if strike.is_none() && p.hammer_force(0) > 0.0 {
                strike = Some(i);
            }
        }
        let ms = strike.map_or(f64::NAN, |i| i as f64 / 48.0);
        check!(finite && ms > lo && ms < hi, "hammer let go at {speed:.1} m/s reaches the string after {ms:.2} ms");
    }
}

#[test]
fn t16_pedal_stir() {
    println!("\n16. Pedal: lifting the dampers lets the strings stir");
    let rms = |p: &mut Piano, steps: usize| {
        let e: f64 = (0..steps).map(|_| p.step().0.powi(2)).sum();
        (e / steps as f64).sqrt()
    };
    let mut p = Piano::new(&make_grand(&GrandOptions::default()), &options(1)).unwrap();
    let quiet = rms(&mut p, 24000);
    let mut e = 0.0;
    for i in 0..48000 {
        if i < 5760 && i % 96 == 0 {
            p.set_pedal(i as f64 / 5760.0); // pedal down over 120 ms
        }
        if i == 5760 {
            p.set_pedal(1.0);
        }
        e += p.step().0.powi(2);
    }
    let stir = (e / 48000.0f64).sqrt();
    let mut q = Piano::new(&make_grand(&GrandOptions::default()), &options(1)).unwrap();
    q.note_on(60, 2.0);
    let note = rms(&mut q, 48000);
    let spl = |x: f64| 20.0 * (x / 2e-5).log10();
    println!("      before: {:.0} dB SPL; pedal going down: {:.0} dB SPL; one mezzo forte C4: {:.0} dB SPL", spl(quiet.max(1e-12)), spl(stir), spl(note));
    check!(stir > 30.0 * quiet.max(1e-9) && stir < 0.1 * note, "the stir is audible and far below a played note ({:.0} dB under)", spl(note) - spl(stir));
}

#[test]
fn t17_single_note_c4_grazing_uses_felt_contact_not_output_level() {
    // C4-only design, not a full-compass loading/threshold calibration.
    // A missed hammer can still produce action/preload sound. Test the actual felt force,
    // with the key held, not pressure amplitude or an arbitrary minimum note duration.
    // Fixed legacy launch reference, independent of experimental MIDI touch curves.
    let legacy_launch = 0.25 + 5.5 * (1.0_f64 / 127.0).powf(1.7);
    for rate in [44100.0, 48000.0, 96000.0] {
        for speed in [0.185, 0.200, legacy_launch] {
            let d = one_note(40);
            let contacts = d.notes[0].strings.len();
            let mut p = Piano::new(&d, &PianoOptions { sample_rate: rate, ..options(1) }).unwrap();
            p.note_on(60, speed);
            let (mut first, mut peak, mut impulse) = (None, 0.0f64, 0.0);
            for sample in 0..(0.2 * rate) as usize {
                let (left, right) = p.step();
                assert!(left.is_finite() && right.is_finite());
                let force: f64 = (0..contacts).map(|i| p.hammer_force(i)).sum();
                assert!(force.is_finite() && force >= 0.0);
                if force > 1e-8 { first.get_or_insert(sample); }
                peak = peak.max(force);
                impulse += force / rate;
            }
            println!("      {rate} Hz, {speed:.9} m/s: first {first:?} samples, peak {peak:.6} N, impulse {impulse:.9} N s");
            if speed == 0.185 {
                check!(first.is_none() && peak == 0.0, "subthreshold launch has no felt contact");
            } else if speed == 0.200 {
                check!(first.is_some() && peak > 0.01 && peak < 0.2 && impulse > 0.0,
                    "near-threshold launch makes a weak real contact");
            } else {
                let onset = first.expect("fixed legacy launch must contact in this reference case") as f64 / rate;
                check!(onset > 0.008 && onset < 0.012 && peak > 0.5 && peak < 3.0,
                    "fixed 0.251458466... m/s reference keeps its original physical contact bounds");
            }
        }
    }
}


#[test]
fn t18_unison_profile_is_stable_centered_and_geometry_invariant() {
    let geometry = |s: &StringDesign| ([s.length, s.rho_a, s.ei, s.ea, s.tension,
        s.sigma0, s.sigma1, s.bridge.x, s.bridge.y].map(f64::to_bits), s.attached);
    for a4 in [432.0, 440.0, 442.0] {
        for stretch in [false, true] {
            let options = GrandOptions { a4_hz: a4, stretch_tuning: stretch, unison_detune_cents: 0.0, ..Default::default() };
            let aligned = make_grand(&options);
            for amount in [0.0, 0.6, 6.0] {
                let options = GrandOptions { unison_detune_cents: amount, ..options.clone() };
                let design = make_grand(&options);
                let rebuilt = make_grand(&options);
                let mut patterns = std::collections::BTreeSet::new();
                for ((base, note), again) in aligned.notes.iter().zip(&design.notes).zip(&rebuilt.notes) {
                    let center = base.strings[0].f0;
                    let cents: Vec<_> = note.strings.iter().map(|s| 1200.0 * (s.f0 / center).log2()).collect();
                    assert!(cents.iter().sum::<f64>().abs() / (cents.len() as f64) < 1e-9);
                    for ((s, zero), repeat) in note.strings.iter().zip(&base.strings).zip(&again.strings) {
                        assert_eq!(s.f0.to_bits(), repeat.f0.to_bits());
                        assert_eq!(geometry(s), geometry(zero));
                        if amount == 0.0 || note.strings.len() == 1 { assert_eq!(s.f0.to_bits(), center.to_bits()); }
                    }
                    assert_eq!(note.duplex.len(), base.duplex.len());
                    for (s, zero) in note.duplex.iter().zip(&base.duplex) { assert_eq!(geometry(s), geometry(zero)); }
                    if amount > 0.0 && note.strings.len() > 1 {
                        assert!(note.strings.windows(2).all(|w| w[0].f0 < w[1].f0));
                        if cents.len() == 3 { patterns.insert(cents.iter().map(|c| (c * 1000.0).round() as i64).collect::<Vec<_>>()); }
                    }
                }
                if amount > 0.0 { assert!(patterns.len() > 50, "profile is not the same triple on every key"); }
            }
        }
    }
}

#[test]
fn t19_unison_targets_have_finite_partials_and_identical_helper_output() {
    for amount in [0.0, 0.6, 6.0] {
        let design = make_grand(&GrandOptions { unison_detune_cents: amount, ..Default::default() });
        let options = PianoOptions { board_grid_scale: 1.5, ..options(1) };
        let mut serial = Piano::new(&design, &options).unwrap();
        let mut parallel = Piano::new(&design, &options).unwrap();
        parallel.set_helper_threads(3, None, None);
        for midi in [36, 60, 84, 96] {
            let note = &design.notes[(midi - 21) as usize];
            for (index, string) in serial.strings_of_note(midi).into_iter().zip(&note.strings) {
                let fundamental = serial.discrete_partial(index, 1);
                assert!((1200.0 * (fundamental / string.f0).log2()).abs() < 1e-6);
                let partials: Vec<_> = (1..=3).map(|p| serial.discrete_partial(index, p)).collect();
                assert!(partials.iter().all(|f| f.is_finite() && *f > 0.0 && *f < 24000.0));
                assert!(partials.windows(2).all(|w| w[0] < w[1]));
            }
        }
        let (mut left, mut right, mut pl, mut pr) = ([0.0f32; 64], [0.0f32; 64], [0.0f32; 64], [0.0f32; 64]);
        let mut peak = 0.0f32;
        for block in 0..32 {
            for piano in [&mut serial, &mut parallel] {
                match block {
                    0 => { piano.note_on(36, 2.0); piano.note_on(60, 2.0); }
                    4 => piano.note_on(96, 3.0),
                    8 => piano.note_off(36, 1.0),
                    12 => piano.set_pedal(0.5),
                    16 => piano.note_off(60, 1.0),
                    24 => piano.set_pedal(0.0),
                    _ => {}
                }
            }
            serial.process(&mut left, &mut right);
            parallel.process(&mut pl, &mut pr);
            for i in 0..64 {
                assert!(left[i].is_finite() && right[i].is_finite());
                assert_eq!((left[i].to_bits(), right[i].to_bits()), (pl[i].to_bits(), pr[i].to_bits()));
                peak = peak.max(left[i].abs()).max(right[i].abs());
            }
        }
        assert!(peak > 0.0);
        assert_eq!(serial.energy().to_bits(), parallel.energy().to_bits());
        println!("      unison amount {amount}: real full-compass partial targets, finite stereo and 0/3-helper equality, peak {peak:.6}");
    }
}


#[test]
fn t20_quiet_midi_one_matches_direct_launch_in_full_compass_without_gain_scaling() {
    // Contact/miss is an observation, not an assertion that every positive velocity sounds.
    let design = make_grand(&GrandOptions::default());
    assert_eq!(design.notes.len(), 88);
    for midi in [21, 36, 60, 84, 90, 96, 108] {
        let mut mapped = Piano::new(&design, &options(1)).unwrap();
        let mut direct = Piano::new(&design, &options(1)).unwrap();
        mapped.note_on(midi, midi_velocity_to_hammer_speed(1));
        direct.note_on(midi, 0.200);
        let contacts = mapped.strings_of_note(midi);
        let (mut peak, mut impulse, mut first) = (0.0f64, 0.0, None);
        for frame in 0..2400 {
            let a = mapped.step(); let b = direct.step();
            assert!(a.0.is_finite() && a.1.is_finite());
            assert_eq!((a.0.to_bits(), a.1.to_bits()), (b.0.to_bits(), b.1.to_bits()));
            let mut force = 0.0;
            for &index in &contacts {
                let f = mapped.hammer_force(index);
                assert!(f.is_finite() && f >= 0.0);
                assert_eq!(f.to_bits(), direct.hammer_force(index).to_bits());
                force += f;
            }
            if force > 0.0 { first.get_or_insert(frame); }
            peak = peak.max(force); impulse += force / 48000.0;
        }
        assert!(mapped.energy().is_finite());
        println!("      full-compass MIDI{midi}/velocity1: direct launch0.200m/s bit-identical, first={first:?} samples, peak={peak:.9e}N, impulse={impulse:.9e}Ns; no audibility guarantee");
    }
}
