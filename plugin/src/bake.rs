//! Offline SFZ sampled approximation. Call on an owned worker, never the audio thread.
use crate::setup::Setup;
use piano_engine::{
    make_grand, midi_velocity_to_hammer_speed, util::write_wav_float_to, Piano, PianoOptions,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, BufWriter, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

// Optional build-time provenance, never runtime `git HEAD` from a changing checkout.
const SOURCE_REVISION: Option<&str> = option_env!("FELT_AND_AIR_BUILD_REVISION");
const _: () = {
    if let Some(revision) = SOURCE_REVISION {
        let bytes = revision.as_bytes();
        assert!(
            bytes.len() == 40,
            "FELT_AND_AIR_BUILD_REVISION must be a full lowercase Git SHA"
        );
        let mut i = 0;
        while i < bytes.len() {
            let c = bytes[i];
            assert!(
                (c >= b'0' && c <= b'9') || (c >= b'a' && c <= b'f'),
                "FELT_AND_AIR_BUILD_REVISION must be a full lowercase Git SHA"
            );
            i += 1;
        }
    }
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BakeRequest {
    pub setup: Setup,
    pub keys: Vec<u8>,
    pub velocities: Vec<u8>,
    pub sample_rate: u32,
    /// Finite held-key recording length, including its explicitly artificial end fade.
    pub hold_seconds: f64,
    pub end_fade_seconds: f64,
    /// Sampler envelope approximation, NOT a recording of physical damper return.
    pub release_seconds: f64,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Plan {
    pub samples: usize,
    pub frames_per_sample: usize,
    pub estimated_wav_bytes: u64,
    pub engine_audio_seconds: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub completed_samples: usize,
    pub rendered_frames: u64,
    pub total_frames: u64,
}

#[derive(Debug)]
pub enum BakeError {
    Invalid(String),
    Io(io::Error),
    Cancelled,
}
impl std::fmt::Display for BakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) => f.write_str(s),
            Self::Io(e) => e.fmt(f),
            Self::Cancelled => f.write_str("Bake cancelled; incomplete output retained"),
        }
    }
}
impl std::error::Error for BakeError {}
impl From<io::Error> for BakeError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Sample {
    pub key: u8,
    pub velocity: u8,
    pub lovel: u8,
    pub hivel: u8,
    pub file: String,
    pub unfaded_peak: f32,
    pub unfaded_end_rms: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub renderer_version: String,
    pub source_revision: Option<String>,
    pub request: BakeRequest,
    pub samples: Vec<Sample>,
}

impl BakeRequest {
    pub fn plan(&self) -> Result<Plan, BakeError> {
        self.setup.options().map_err(BakeError::Invalid)?;
        let sorted_in = |s: &[u8], lo, hi| {
            !s.is_empty()
                && s.iter().all(|&v| (lo..=hi).contains(&v))
                && s.windows(2).all(|w| w[0] < w[1])
        };
        if !sorted_in(&self.keys, 21, 108)
            || !sorted_in(&self.velocities, 1, 127)
            || self.velocities.len() > 16
        {
            return Err(BakeError::Invalid("Keys must be sorted/unique MIDI21..108; 1..16 sorted unique velocities1..127 (zero is NoteOff)".into()));
        }
        if ![44100, 48000, 96000].contains(&self.sample_rate)
            || !self.hold_seconds.is_finite()
            || !(0.02..=120.0).contains(&self.hold_seconds)
            || !self.end_fade_seconds.is_finite()
            || self.end_fade_seconds < 2.0 / self.sample_rate as f64
            || self.end_fade_seconds > self.hold_seconds / 2.0
            || !self.release_seconds.is_finite()
            || !(0.0..=10.0).contains(&self.release_seconds)
        {
            return Err(BakeError::Invalid("Use 44.1/48/96kHz, finite hold0.02..120s, end fade2frames..half hold, sampler release0..10s".into()));
        }
        let frames = (self.hold_seconds * self.sample_rate as f64).round() as usize;
        let samples = self.keys.len() * self.velocities.len();
        let estimated_wav_bytes = samples as u64 * (44 + frames as u64 * 8);
        let engine_audio_seconds = samples as f64 * frames as f64 / self.sample_rate as f64;
        if estimated_wav_bytes > 2 * 1024 * 1024 * 1024 || engine_audio_seconds > 3600.0 {
            return Err(BakeError::Invalid("Bake exceeds 2GiB WAV / one hour rendered-audio resource bound; choose a smaller explicit range/duration".into()));
        }
        Ok(Plan {
            samples,
            frames_per_sample: frames,
            estimated_wav_bytes,
            engine_audio_seconds,
        })
    }
}

fn exclusive(path: &Path, data: &[u8]) -> Result<(), BakeError> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(data)?;
    Ok(())
}
fn check_cancel(cancel: &AtomicBool) -> Result<(), BakeError> {
    if cancel.load(Ordering::Relaxed) {
        Err(BakeError::Cancelled)
    } else {
        Ok(())
    }
}

/// Owns the captured request. Never reads live parameters, installs a bank, opens a device,
/// or overwrites an existing directory. Failure/cancellation leaves a marked partial bank.
pub fn bake(
    request: BakeRequest,
    output: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Progress),
) -> Result<Manifest, BakeError> {
    let plan = request.plan()?;
    check_cancel(cancel)?;
    fs::create_dir(output)?; // Exclusive reservation: even an empty existing bank is refused.
    exclusive(&output.join("INCOMPLETE.txt"), b"Not a usable bank. Baking failed, was cancelled, or is still running. No files were installed.\n")?;
    fs::create_dir(output.join("Samples"))?;
    let design = make_grand(&request.setup.options().map_err(BakeError::Invalid)?);
    let grid = request.setup.grid_scale().map_err(BakeError::Invalid)?;
    let mut samples = Vec::with_capacity(plan.samples);
    let mut audio = vec![0.0f32; plan.frames_per_sample * 2];
    let total_frames = plan.frames_per_sample as u64 * plan.samples as u64;
    let mut rendered_frames = 0;
    for &key in &request.keys {
        for (layer, &velocity) in request.velocities.iter().enumerate() {
            check_cancel(cancel)?;
            // Keep the whole compass, board loading and room; every sample starts fresh.
            let mut piano = Piano::new(
                &design,
                &PianoOptions {
                    sample_rate: request.sample_rate as f64,
                    board_grid_scale: grid,
                    ..Default::default()
                },
            )
            .map_err(BakeError::Invalid)?;
            check_cancel(cancel)?;
            piano.note_on(key as i32, midi_velocity_to_hammer_speed(velocity as i32));
            let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
            for from in (0..plan.frames_per_sample).step_by(64) {
                check_cancel(cancel)?;
                let count = 64.min(plan.frames_per_sample - from);
                piano.process(&mut left[..count], &mut right[..count]);
                for i in 0..count {
                    // Identical output transform to the plugin, exactly once; no normalization.
                    for (channel, value) in [left[i], right[i]].into_iter().enumerate() {
                        let scaled = value * request.setup.volume;
                        if !value.is_finite() || !scaled.is_finite() {
                            return Err(BakeError::Invalid(
                                "Nonfinite engine output; partial bank retained".into(),
                            ));
                        }
                        audio[2 * (from + i) + channel] = scaled.tanh();
                    }
                }
                rendered_frames += count as u64;
                progress(Progress {
                    completed_samples: samples.len(),
                    rendered_frames,
                    total_frames,
                });
            }
            let peak = audio.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let tail_frames = (request.sample_rate as usize / 100).min(plan.frames_per_sample);
            let end_rms = (audio[audio.len() - 2 * tail_frames..]
                .iter()
                .map(|&x| (x as f64).powi(2))
                .sum::<f64>()
                / (2 * tail_frames) as f64)
                .sqrt();
            let fade = (request.end_fade_seconds * request.sample_rate as f64).round() as usize;
            for i in 0..fade {
                let gain = (0.5
                    * (1.0 + (std::f64::consts::PI * i as f64 / (fade - 1) as f64).cos()))
                    as f32;
                let frame = plan.frames_per_sample - fade + i;
                audio[2 * frame] *= gain;
                audio[2 * frame + 1] *= gain;
            }
            let file = format!("Samples/n{key:03}_v{velocity:03}.wav");
            check_cancel(cancel)?;
            let mut f = BufWriter::new(
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output.join(&file))?,
            );
            write_wav_float_to(&mut f, &audio, 2, request.sample_rate)?;
            f.flush()?;
            let lovel = if layer == 0 {
                1
            } else {
                ((request.velocities[layer - 1] as u16 + velocity as u16) / 2 + 1) as u8
            };
            let hivel = request
                .velocities
                .get(layer + 1)
                .map(|&v| ((velocity as u16 + v as u16) / 2) as u8)
                .unwrap_or(127);
            samples.push(Sample {
                key,
                velocity,
                lovel,
                hivel,
                file,
                unfaded_peak: peak,
                unfaded_end_rms: end_rms,
            });
            progress(Progress {
                completed_samples: samples.len(),
                rendered_frames,
                total_frames,
            });
        }
    }
    check_cancel(cancel)?;
    let mut sfz = format!("// Felt & Air sampled approximation; see README.txt and manifest.json\n<global> amp_veltrack=0 ampeg_attack=0 ampeg_decay=0 ampeg_sustain=100 ampeg_release={} loop_mode=no_loop\n", request.release_seconds);
    for s in &samples {
        sfz.push_str(&format!(
            "<region> sample={} key={} pitch_keycenter={} lovel={} hivel={}\n",
            s.file, s.key, s.key, s.lovel, s.hivel
        ));
    }
    let manifest = Manifest {
        renderer_version: env!("CARGO_PKG_VERSION").into(),
        source_revision: SOURCE_REVISION.map(str::to_owned),
        request,
        samples,
    };
    let json =
        serde_json::to_vec_pretty(&manifest).map_err(|e| BakeError::Invalid(e.to_string()))?;
    exclusive(&output.join("manifest.json"), &json)?;
    exclusive(&output.join("README.txt"), b"Felt & Air SFZ SAMPLED APPROXIMATION; not a physical piano engine.\n\nOnly explicitly selected keys are included. Layers use baked physical strike velocities, nearest-layer bins and amp_veltrack=0: no normalization or additional velocity gain. Every WAV captures a fresh full-compass instrument with the frozen Setup and plugin volume/tanh.\n\nSamples are finite held-key recordings, never looped. An explicit END-only cosine fade prevents a hard audible last-sample cut; there is no attack fade. Holds beyond file length end in this artificial fade. manifest.json records the unfaded end RMS/peak, so a nonzero end means real decay has been shortened. Sampler ampeg_release is an explicit envelope approximation, NOT a physical damper-return recording.\n\nIndependent samples cannot reproduce shared polyphonic string/board coupling, retrigger phase, finite under-damper resonances, re-pedalling, sostenuto or dynamic una-corda. Sampler pedal behavior depends on its host, not a captured engine response. Baked room and per-note nonlinear output/tanh do not equal the live polyphonic room/bus. Adjusting these requires rebaking. Float32 stereo WAV/SFZ host import/playback is not verified by exporting. No files are installed automatically.\n")?;
    let staged = output.join("preset.sfz.pending");
    exclusive(&staged, sfz.as_bytes())?;
    OpenOptions::new().write(true).open(&staged)?.sync_all()?;
    check_cancel(cancel)?;
    // Same-directory hard-link publication is atomic and fails if the final path exists.
    // Do NOT use rename(), which can replace an externally created final SFZ.
    fs::hard_link(&staged, output.join("preset.sfz"))?;
    fs::remove_file(staged)?;
    fs::remove_file(output.join("INCOMPLETE.txt"))?;
    Ok(manifest)
}

#[cfg(test)]
#[path = "bake_tests.rs"]
mod tests;
