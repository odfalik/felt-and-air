//! A piano simulated from its mechanics. See docs/MODEL.md.
pub mod acoustics;
mod action_contact;
pub mod design;
pub mod ffi;
pub mod midi;
pub mod params;
pub mod piano;
pub mod plate;
pub mod util;

pub use design::{make_grand, midi_velocity_to_hammer_speed, GrandOptions, PianoDesign, Vec2};
pub use piano::{Piano, PianoOptions, Snapshot};
