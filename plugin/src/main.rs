//! Standalone shell. CoreAudio output only; no audio inputs are requested. Automatic MIDI
//! is owned by the discovery thread, not the audio callback. `--backend dummy` bypasses all
//! MIDI construction as well as audio devices; explicit --midi-input remains NIH-owned.
mod midi_input;
use midi_input::MidiWatcher;
use physical_piano_plugin::{
    shared::{EXTERNAL_MIDI, STANDALONE},
    PhysicalPiano,
};
use std::sync::atomic::Ordering;

fn main() {
    // Dummy/no-MIDI also touches this queue in process(): allocate its backing storage now.
    std::sync::LazyLock::force(&EXTERNAL_MIDI);
    STANDALONE.store(true, Ordering::Relaxed);
    let mut args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|a| a == "--input-device" || a.starts_with("--input-device="))
    {
        eprintln!("Felt & Air is output-only; audio input is not supported.");
        std::process::exit(2);
    }
    if backend(&args).is_none() {
        args.extend(["--backend".into(), "core-audio".into()]);
    }
    let midi = if auto_midi(&args) {
        match MidiWatcher::start() {
            Ok(watcher) => Some(watcher),
            Err(e) => {
                eprintln!("Could not start MIDI discovery: {e}");
                None
            }
        }
    } else {
        None
    };
    let ok = nih_plug::prelude::nih_export_standalone_with_args::<PhysicalPiano, _>(args);
    // Join the owner and retire its connections before returning or exiting with an error.
    drop(midi);
    if !ok {
        std::process::exit(1);
    }
}

fn backend(args: &[String]) -> Option<&str> {
    args.iter().enumerate().find_map(|(i, a)| {
        if a == "--backend" || a == "-b" {
            args.get(i + 1).map(String::as_str)
        } else {
            a.strip_prefix("--backend=")
                .or_else(|| a.strip_prefix("-b").filter(|v| !v.is_empty()))
        }
    })
}
fn auto_midi(args: &[String]) -> bool {
    backend(args) != Some("dummy")
        && !args.iter().any(|a| {
            matches!(
                a.as_str(),
                "--help" | "-h" | "--version" | "-V" | "--midi-input"
            ) || a.starts_with("--midi-input=")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).into()).collect()
    }
    #[test]
    fn no_midi_construction_for_dummy_help_or_explicit_midi() {
        for values in [
            vec!["app", "--backend", "dummy"],
            vec!["app", "--backend=dummy"],
            vec!["app", "-b", "dummy"],
            vec!["app", "-bdummy"],
            vec!["app", "--help"],
            vec!["app", "-h"],
            vec!["app", "--version"],
            vec!["app", "--midi-input", "Keys"],
            vec!["app", "--midi-input=Keys"],
        ] {
            assert!(!auto_midi(&args(&values)), "{values:?}");
        }
        assert!(auto_midi(&args(&["app"])));
        assert!(auto_midi(&args(&["app", "--output-device", "dummy"])));
        assert_eq!(backend(&args(&["app", "-bdummy"])), Some("dummy"));
    }
}
