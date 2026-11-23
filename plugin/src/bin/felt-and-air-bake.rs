//! Small offline SFZ exporter. No GUI, device, sampler, installer or live-engine access.
use physical_piano_plugin::{
    bake::{bake, BakeRequest},
    setup::Setup,
};
use std::{collections::BTreeMap, path::PathBuf, sync::atomic::AtomicBool};

const HELP: &str = "Felt & Air — bake an SFZ SAMPLED APPROXIMATION\n\nRun with nice -n 15; the renderer uses zero helper threads. No device is opened.\n\nfelt-and-air-bake --output NEW_DIRECTORY --defaults \\\n  --keys 60,64,67 --velocities 32,80,120 --sample-rate 48000 \\\n  --hold-seconds 0.25 --end-fade-seconds 0.02 --release-seconds 0.15\n\nOr replace --defaults with --setup-file EXISTING_SETUPS_JSON --setup-name NAME.\nThis reads the SAME named-setup map saved by the editor; it does not capture a running app.\nKeys accept a comma list or inclusive MIDI range (e.g.21-108 is explicit full keyboard).\nC4-only/key60 is a smoke/one-note bank, NOT a full instrument.\nHold is configurable/finite, with an explicit END-only fade, no loops/normalization.\nSFZ release is a sampler envelope approximation, not physical damper capture.\nCtrl-C leaves a clearly marked INCOMPLETE bank; existing directories are never overwritten.\n";

fn list(text: &str) -> Result<Vec<u8>, String> {
    let mut values = Vec::new();
    for part in text.split(',') {
        if let Some((lo, hi)) = part.split_once('-') {
            let lo: u8 = lo.parse().map_err(|_| "Invalid range")?;
            let hi: u8 = hi.parse().map_err(|_| "Invalid range")?;
            if lo > hi {
                return Err("Reversed range".into());
            }
            values.extend(lo..=hi);
        } else {
            values.push(part.parse().map_err(|_| "Invalid number list")?);
        }
    }
    values.sort_unstable();
    if values.windows(2).any(|w| w[0] == w[1]) {
        return Err("Duplicate key/velocity".into());
    }
    Ok(values)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Bake failed: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut options = BTreeMap::new();
    let mut defaults = false;
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            print!("{HELP}");
            return Ok(());
        }
        if arg == "--defaults" {
            if defaults {
                return Err("Duplicate --defaults".into());
            }
            defaults = true;
            continue;
        }
        if ![
            "--output",
            "--keys",
            "--velocities",
            "--sample-rate",
            "--hold-seconds",
            "--end-fade-seconds",
            "--release-seconds",
            "--setup-file",
            "--setup-name",
        ]
        .contains(&arg.as_str())
        {
            return Err(format!("Unknown option {arg}; use --help").into());
        }
        let value = args.next().ok_or("Missing option value")?;
        if options.insert(arg, value).is_some() {
            return Err("Duplicate option".into());
        }
    }
    let required = |name: &str| {
        options
            .get(name)
            .ok_or_else(|| format!("Missing {name}; use --help"))
    };
    let setup = if defaults {
        if options.contains_key("--setup-file") || options.contains_key("--setup-name") {
            return Err("Choose defaults OR a named saved setup".into());
        }
        Setup::default()
    } else {
        let path = required("--setup-file")?;
        if std::fs::metadata(path)?.len() > 1024 * 1024 {
            return Err("Setup file exceeds 1MiB".into());
        }
        let mut setups: BTreeMap<String, Setup> = serde_json::from_slice(&std::fs::read(path)?)?;
        setups
            .remove(required("--setup-name")?)
            .ok_or("Named setup not found")?
    };
    let request = BakeRequest {
        setup,
        keys: list(required("--keys")?)?,
        velocities: list(required("--velocities")?)?,
        sample_rate: required("--sample-rate")?.parse()?,
        hold_seconds: required("--hold-seconds")?.parse()?,
        end_fade_seconds: required("--end-fade-seconds")?.parse()?,
        release_seconds: required("--release-seconds")?.parse()?,
    };
    let plan = request.plan()?;
    eprintln!("{} samples, {:.2} MiB WAV, {:.2} seconds of full-engine rendering (NOT a wall-time estimate); helpers0. Holds are finite sampled approximations.", plan.samples, plan.estimated_wav_bytes as f64 / 1048576.0, plan.engine_audio_seconds);
    let output = PathBuf::from(required("--output")?);
    let cancel = AtomicBool::new(false);
    let mut completed = 0;
    let manifest = bake(request, &output, &cancel, |p| {
        if p.completed_samples != completed {
            completed = p.completed_samples;
            eprintln!("Baked {completed}/{}", plan.samples);
        }
    })?;
    let live_ends = manifest
        .samples
        .iter()
        .filter(|s| s.unfaded_end_rms > 1e-6)
        .count();
    eprintln!("Wrote {}. {live_ends}/{} samples had nonzero end RMS above -120dBFS before the explicit finite-end fade. Read README.txt; SFZ host import/playback remains unverified.", output.display(), manifest.samples.len());
    Ok(())
}
