//! Demo recordings: real performances as MIDI files, listed in demos/demos.txt. They are
//! read once, and the audio thread plays them through the instrument. The player lives
//! outside the instrument, so a demo keeps playing while the instrument is rebuilt.

use piano_engine::midi::{lead_notes_by_flight, read_midi_file, MidiEvent};
use piano_engine::midi_velocity_to_hammer_speed;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub struct Demo {
    pub title: String,
    pub note: String,
    pub events: Vec<MidiEvent>,
    pub seconds: f64,
}

/// Optional external collection. There is no compile-time, bundled-resource or CWD lookup.
fn demo_dir() -> Option<PathBuf> {
    select_demo_dir(
        std::env::var_os("FELT_AND_AIR_DEMOS").map(PathBuf::from),
        crate::paths::support_dir(),
    )
}
fn select_demo_dir(override_dir: Option<PathBuf>, support: Option<PathBuf>) -> Option<PathBuf> {
    override_dir
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| support.map(|p| p.join("demos")))
}

fn load(let_off_m: f64) -> Vec<Demo> {
    demo_dir().map_or_else(Vec::new, |dir| load_from(&dir, let_off_m))
}
fn load_from(dir: &Path, let_off_m: f64) -> Vec<Demo> {
    let Ok(list) = std::fs::read_to_string(dir.join("demos.txt")) else {
        return Vec::new();
    };
    list.lines()
        .filter_map(|line| {
            let mut f = line.split('|');
            let (file, title, note) = (
                f.next()?.trim(),
                f.next().unwrap_or("").trim(),
                f.next().unwrap_or("").trim(),
            );
            let mut events = read_midi_file(dir.join(file).to_str()?);
            if events.is_empty() {
                return None;
            }
            lead_notes_by_flight(&mut events, let_off_m, |v| midi_velocity_to_hammer_speed(v));
            let seconds = events.last().map_or(0.0, |e| e.seconds);
            Some(Demo {
                title: title.into(),
                note: note.into(),
                events,
                seconds,
            })
        })
        .collect()
}

/// Read once per process, at the reference let-off distance (as the AppKit app did).
pub fn demos() -> Arc<Vec<Demo>> {
    static DEMOS: OnceLock<Arc<Vec<Demo>>> = OnceLock::new();
    DEMOS
        .get_or_init(|| {
            Arc::new(load(
                piano_engine::GrandOptions::default().let_off_mm * 1e-3,
            ))
        })
        .clone()
}

/// The audio thread's place in a demo.
#[derive(Default)]
pub struct Player {
    pub index: Option<usize>,
    pub next: usize,
    pub frame: u64,
}

impl Player {
    pub fn start(&mut self, index: usize) {
        self.index = Some(index);
        self.next = 0;
        self.frame = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir().join(format!(
                "physical-piano-demos-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn collection(dir: &Path, title: &str) {
        fs::create_dir_all(dir).unwrap();
        // A minimal generated SMF: C4 down at 0, up at 0.5 seconds. No recording/asset.
        let midi: &[u8] = b"MThd\0\0\0\x06\0\0\0\x01\x01\xe0MTrk\0\0\0\x0d\0\x90\x3c\x40\x83\x60\x80\x3c\x40\0\xff\x2f\0";
        fs::write(dir.join("note.mid"), midi).unwrap();
        fs::write(
            dir.join("demos.txt"),
            format!("note.mid|{title}|Generated test MIDI\n"),
        )
        .unwrap();
    }

    #[test]
    fn runtime_override_precedes_per_user_collection() {
        let temp = TempDir::new();
        let support = temp.0.join("support");
        let external = temp.0.join("external collection");
        collection(&support.join("demos"), "Per-user collection");
        collection(&external, "Runtime override");
        let selected = select_demo_dir(Some(external.clone()), Some(support.clone())).unwrap();
        assert_eq!(selected, external);
        let demos = load_from(&selected, 0.002);
        assert_eq!(demos.len(), 1);
        assert_eq!(demos[0].title, "Runtime override");
        assert_eq!(demos[0].events.len(), 2);
        assert!((demos[0].seconds - 0.5).abs() < 1e-9);
        let default = select_demo_dir(None, Some(support)).unwrap();
        assert_eq!(load_from(&default, 0.002)[0].title, "Per-user collection");
    }

    #[test]
    fn missing_external_collection_is_optional_without_fallback_or_creation() {
        let temp = TempDir::new();
        let absent = temp.0.join("absent");
        let selected = select_demo_dir(Some(absent.clone()), Some(temp.0.clone())).unwrap();
        assert_eq!(selected, absent); // a missing explicit override never falls back elsewhere
        assert!(load_from(&selected, 0.002).is_empty());
        assert!(!selected.exists());
        assert_eq!(select_demo_dir(None, None), None); // never use CWD when HOME is missing
        assert_eq!(select_demo_dir(Some(PathBuf::new()), None), None);
        assert_eq!(
            select_demo_dir(Some(PathBuf::new()), Some(temp.0.clone())),
            Some(temp.0.join("demos"))
        );
    }
}
