//! Measurements that characterise the instrument rather than test it:
//! bridge driving-point impedance at several notes.
use piano_engine::util::fft;
use piano_engine::*;

fn main() {
    let fs = 48000.0;
    let d = make_grand(&GrandOptions::default());
    println!("Bridge driving-point impedance |Z| in kg/s, by octave band");
    print!("{:<6}", "note");
    let bands: Vec<f64> = (0..7).map(|i| 62.5 * 2f64.powi(i)).collect();
    for f in &bands {
        print!(" {:>6.0}-{:<5.0}", f, 2.0 * f);
    }
    println!();
    for key in [4usize, 16, 28, 40, 52, 64, 76, 86] {
        let mut p = Piano::new(&d, &PianoOptions::default()).unwrap();
        let at = d.notes[key - 1].strings[0].bridge;
        let n = 1 << 16;
        p.push_board(at, 1.0); // one step of 1 N: flat force spectrum of magnitude dt
        let mut re: Vec<f64> = (0..n)
            .map(|_| {
                p.step();
                p.board_velocity(at)
            })
            .collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        print!("{:<6}", key + 20);
        for f in &bands {
            // Mean mobility magnitude in the band, then inverted.
            let (lo, hi) = ((f / fs * n as f64) as usize, (2.0 * f / fs * n as f64) as usize);
            let mean: f64 = (lo..hi).map(|k| re[k].hypot(im[k])).sum::<f64>() / (hi - lo) as f64;
            print!(" {:>12.0}", 1.0 / mean);
        }
        println!();
    }
}
