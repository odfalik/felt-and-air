//! Realtime budget measurement. Runs the engine block by block, the way an audio callback
//! would, and reports how much of each block's deadline the physics uses.
//!   bench [seconds per scenario] [block size] [board grid scale] [helper threads]
//! "wall" counts time the thread was kept waiting by other work on the machine too, which is
//! what an audio callback would experience. For a number that does not depend on how busy
//! the machine is, use the `cycles` tool.
use piano_engine::piano::request_performance_core;
use piano_engine::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let seconds: f64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(4.0);
    let block: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(128);
    let grid: f64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1.5);
    let helpers: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
    let fs = 48000.0;
    request_performance_core();
    let mut piano = Piano::new(&make_grand(&GrandOptions::default()), &PianoOptions { board_grid_scale: grid, ..PianoOptions::default() }).unwrap();
    piano.set_helper_threads(helpers, None, None);
    println!(
        "engine: {} strings / {} string nodes, board {} nodes ({:.1} x {:.1} mm), block {} ({:.2} ms), {} helper threads",
        piano.string_count(),
        piano.total_string_nodes(),
        piano.plate().n,
        piano.plate().hx * 1e3,
        piano.plate().hy * 1e3,
        block,
        1e3 * block as f64 / fs,
        helpers
    );
    println!("{:<34} {:>9} {:>9} {:>9}  {}", "scenario", "wall med", "wall p99", "wall max", "awake");
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    let deadline = block as f64 / fs;
    let blocks = (seconds * fs / block as f64) as usize;
    let mut run = |piano: &mut Piano, name: &str, events: &mut dyn FnMut(&mut Piano, usize)| {
        let mut wall = Vec::with_capacity(blocks);
        let mut awake = 0;
        for b in 0..blocks {
            events(piano, b);
            let t0 = std::time::Instant::now();
            piano.process(&mut l, &mut r);
            wall.push(t0.elapsed().as_secs_f64() / deadline);
            awake = awake.max(piano.awake_strings());
        }
        wall.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("{:<34} {:>8.1}% {:>8.1}% {:>8.1}%  {}", name, 100.0 * wall[wall.len() / 2], 100.0 * wall[wall.len() * 99 / 100], 100.0 * wall[wall.len() - 1], awake);
    };
    let settle = |piano: &mut Piano| {
        piano.set_sustain(false);
        (21..=108).for_each(|m| piano.note_off(m, 1.0));
    };
    run(&mut piano, "nothing played", &mut |_, _| {});
    run(&mut piano, "six note chord held", &mut |p, b| {
        if b == 0 {
            [36, 48, 55, 60, 64, 72].iter().for_each(|&m| p.note_on(m, 2.5));
        }
    });
    settle(&mut piano);
    let period = ((0.08 * fs / block as f64) as usize).max(1); // a note every 80 ms
    run(&mut piano, "fast playing, no pedal", &mut |p, b| {
        if b % period == 0 {
            let i = (b / period) as i32;
            p.note_off(36 + (i * 7 + 60 - 7) % 60, 1.0);
            p.note_on(36 + (i * 7) % 60, 3.0);
        }
    });
    settle(&mut piano);
    let period = ((0.1 * fs / block as f64) as usize).max(1);
    run(&mut piano, "pedal down, a note every 100 ms", &mut |p, b| {
        if b == 0 {
            p.set_sustain(true);
        }
        if b % period == 0 {
            p.note_on(30 + ((b / period) as i32 * 11) % 70, 4.0);
        }
    });
}
