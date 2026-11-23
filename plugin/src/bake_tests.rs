use super::*;
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "felt-bake-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn smoke() -> BakeRequest {
    BakeRequest {
        setup: Setup::default(),
        keys: vec![60, 64, 67],
        velocities: vec![32, 80, 120],
        sample_rate: 48000,
        hold_seconds: 0.25,
        end_fade_seconds: 0.02,
        release_seconds: 0.15,
    }
}
fn wav(path: &Path, rate: u32, frames: usize) -> Vec<f32> {
    let bytes = fs::read(path).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..16], b"WAVEfmt ");
    assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 3);
    assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), rate);
    assert_eq!(
        u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize,
        frames * 8
    );
    assert_eq!(bytes.len(), 44 + frames * 8);
    bytes[44..]
        .chunks_exact(4)
        .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
        .collect()
}

#[test]
fn defaults_are_the_editor_capture_including_float_rounding_and_the_shared_grid_mapping() {
    let params = crate::PianoParams::new();
    let defaults = Setup::default();
    assert_eq!(defaults, params.capture_setup());
    let captured = defaults.options().unwrap();
    let editor = params.options();
    let raw = piano_engine::GrandOptions::default();
    let mut saw_rounding = false;
    for p in piano_engine::params::PARAMS {
        assert_eq!(
            (p.get)(&captured).to_bits(),
            (p.get)(&editor).to_bits(),
            "{}",
            p.name
        );
        saw_rounding |= (p.get)(&captured).to_bits() != (p.get)(&raw).to_bits();
    }
    assert!(
        saw_rounding,
        "raw GrandOptions must not substitute for the actual FloatParam defaults"
    );
    assert_eq!(defaults.grid_scale().unwrap(), params.grid_scale());
    assert!(crate::setup::grid_scale(-1).is_none());
    assert!(crate::setup::grid_scale(4).is_none());
}

#[test]
fn actual_nine_wav_sfz_bank_preserves_snapshot_gain_layers_and_only_end_fades() {
    let temp = Temp::new();
    let output = temp.0.join("bank");
    let mut live = Setup::default();
    let mut request = smoke();
    request.setup = live.clone();
    // A subsequent live edit must not change the already owned snapshot.
    live.volume = 0.0;
    live.values[0] = 0.0;
    let plan = request.plan().unwrap();
    let expected = request.clone();
    let manifest = bake(request, &output, &AtomicBool::new(false), |_| {}).unwrap();
    assert_eq!(manifest.request.setup, expected.setup);
    assert_eq!(manifest.samples.len(), 9);
    assert!(!output.join("INCOMPLETE.txt").exists());
    let from_disk: Manifest =
        serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(from_disk.request.setup, expected.setup);
    assert_eq!(from_disk.source_revision.as_deref(), SOURCE_REVISION);
    let manifest_json: serde_json::Value =
        serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
    if SOURCE_REVISION.is_none() {
        assert!(manifest_json["source_revision"].is_null());
    }
    assert!(!output.join("preset.sfz.pending").exists());
    let sfz = fs::read_to_string(output.join("preset.sfz")).unwrap();
    assert!(
        sfz.contains("amp_veltrack=0")
            && sfz.contains("ampeg_attack=0")
            && sfz.contains("loop_mode=no_loop")
    );
    assert!(!sfz.contains("loop_start") && !sfz.contains("loop_end"));
    assert_eq!(sfz.matches("<region>").count(), 9);
    for &key in &expected.keys {
        let rows: Vec<_> = manifest.samples.iter().filter(|s| s.key == key).collect();
        assert_eq!((rows[0].lovel, rows[2].hivel), (1, 127));
        assert!(rows.windows(2).all(|w| w[0].hivel + 1 == w[1].lovel));
    }
    for sample in &manifest.samples {
        assert!(sfz.contains(&format!(
            "sample={} key={} pitch_keycenter={} lovel={} hivel={}",
            sample.file, sample.key, sample.key, sample.lovel, sample.hivel
        )));
        let audio = wav(
            &output.join(&sample.file),
            expected.sample_rate,
            plan.frames_per_sample,
        );
        assert!(audio.iter().all(|x| x.is_finite()));
        assert_eq!(&audio[audio.len() - 2..], &[0.0, 0.0]);
        // Independent real-engine capture, full compass and exact plugin gain transform.
        let design = make_grand(&expected.setup.options().unwrap());
        assert_eq!(design.notes.len(), 88);
        let mut p = Piano::new(
            &design,
            &PianoOptions {
                sample_rate: expected.sample_rate as f64,
                board_grid_scale: expected.setup.grid_scale().unwrap(),
                ..Default::default()
            },
        )
        .unwrap();
        p.note_on(
            sample.key as i32,
            midi_velocity_to_hammer_speed(sample.velocity as i32),
        );
        let (mut left, mut right) = ([0.0f32; 64], [0.0f32; 64]);
        let fade = (expected.end_fade_seconds * expected.sample_rate as f64).round() as usize;
        for from in (0..plan.frames_per_sample - fade).step_by(64) {
            let n = 64.min(plan.frames_per_sample - fade - from);
            p.process(&mut left[..n], &mut right[..n]);
            for i in 0..n {
                assert_eq!(
                    audio[2 * (from + i)].to_bits(),
                    (left[i] * expected.setup.volume).tanh().to_bits()
                );
                assert_eq!(
                    audio[2 * (from + i) + 1].to_bits(),
                    (right[i] * expected.setup.volume).tanh().to_bits()
                );
            }
        }
    }
}

#[test]
fn cancellation_leaves_only_marked_partial_bank_and_existing_bank_is_never_clobbered() {
    let temp = Temp::new();
    let output = temp.0.join("partial");
    let cancel = AtomicBool::new(false);
    let result = bake(smoke(), &output, &cancel, |p| {
        if p.completed_samples == 1 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert!(matches!(result, Err(BakeError::Cancelled)));
    assert!(output.join("INCOMPLETE.txt").exists());
    assert!(!output.join("preset.sfz").exists());
    assert_eq!(fs::read_dir(output.join("Samples")).unwrap().count(), 1);
    let first = fs::read(output.join("Samples/n060_v032.wav")).unwrap();
    assert!(
        matches!(bake(smoke(),&output,&AtomicBool::new(false),|_|{}),Err(BakeError::Io(e)) if e.kind()==io::ErrorKind::AlreadyExists)
    );
    assert_eq!(
        fs::read(output.join("Samples/n060_v032.wav")).unwrap(),
        first
    );
    let mut bad = smoke();
    bad.velocities = vec![0];
    assert!(bad.plan().is_err());
    bad = smoke();
    bad.setup.volume = f32::NAN;
    assert!(bad.plan().is_err());
    bad = smoke();
    bad.keys = (21..=108).collect();
    bad.velocities = (1..=16).collect();
    bad.hold_seconds = 120.;
    assert!(bad.plan().is_err());
}

#[test]
fn cancel_during_render_and_final_sfz_collision_fail_closed_on_real_files() {
    let temp = Temp::new();
    let output = temp.0.join("cancel-mid-note");
    let cancel = AtomicBool::new(false);
    let mut request = smoke();
    request.keys = vec![60];
    request.velocities = vec![80];
    let result = bake(request.clone(), &output, &cancel, |p| {
        if p.completed_samples == 0 && p.rendered_frames >= 64 {
            cancel.store(true, Ordering::Relaxed);
        }
    });
    assert!(matches!(result, Err(BakeError::Cancelled)));
    assert!(output.join("INCOMPLETE.txt").exists());
    assert!(!output.join("preset.sfz").exists());
    assert_eq!(fs::read_dir(output.join("Samples")).unwrap().count(), 0);

    let output = temp.0.join("sfz-collision");
    request.hold_seconds = 0.02;
    request.end_fade_seconds = 0.005;
    let mut inserted = false;
    let result = bake(request, &output, &AtomicBool::new(false), |p| {
        if p.completed_samples == 1 && !inserted {
            exclusive(&output.join("preset.sfz"), b"external file; do not replace").unwrap();
            inserted = true;
        }
    });
    assert!(matches!(result, Err(BakeError::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists));
    assert_eq!(
        fs::read(output.join("preset.sfz")).unwrap(),
        b"external file; do not replace"
    );
    assert!(output.join("INCOMPLETE.txt").exists());
    let staged = fs::read_to_string(output.join("preset.sfz.pending")).unwrap();
    assert!(staged.contains("<global>") && staged.contains("<region>"));
}
