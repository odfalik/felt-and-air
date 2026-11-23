//! Presentation copy and formatting for instrument controls.
//! Parameter ranges, defaults, and writes stay owned by `piano_engine::params::PARAMS`.
use super::*;
use piano_engine::design::RoomShape;

pub(super) const TABS: [&str; 4] = ["Room", "Action", "Strings", "Soundboard"];

pub(super) fn category_help(tab: usize) -> &'static str {
    match tab {
        0 => "Room shape, walls, ceiling, lid opening, and interactive piano/listener placement.",
        1 => "Live hammer, shank, damper, and unison-wire cutaway driven by the running simulation.",
        2 => "Individual unison wires in separate lanes with live motion, strike position, and damper.",
        _ => "Live soundboard displacement overlaid with spruce ribs, rim clamping, and active bridge pins.",
    }
}

/// Plain-language presentation copy; never defines parallel ranges or defaults.
pub(super) fn copy(name: &str) -> (&str, &str) {
    match name {
        "Room length" => ("Room length", "Length along the keyboard axis; changes reflection timing and room volume."),
        "Room width" => ("Room width", "Depth along the strings; shapes side-wall reflections and room resonance."),
        "Room height" => ("Ceiling height", "Average height of the ceiling above the floor."),
        "Room shape" => ("Room shape", "Floor-plan geometry used for real 1st- and 2nd-order planar reflections."),
        "Ceiling slope" => ("Ceiling slope", "Tilts the ceiling along the room length to break vertical flutter echoes."),
        "Wall splay" => ("Wall splay", "Tapers side walls wider or narrower toward the listener (or drag floor-plan corners)."),
        "Wall skew" => ("Wall skew", "Angles opposing walls to suppress parallel flutter echoes (or drag floor-plan corners)."),
        "Binaural head shadow" => ("Binaural head shadow", "Adds spherical head-shadow filtering to each ear for natural 3D headphone depth."),
        "Wall absorption" => ("Wall absorption", "Fraction of sound energy absorbed per bounce. Low is lively; high is dry."),
        "Ear height" => ("Listener height", "Height of the listener’s ears above the floor. Soundboard sits at 0.75 m."),
        "Ear spacing" => ("Ear spacing", "Distance between the left and right pickup points; controls stereo width."),
        "Lid angle" => ("Lid opening", "Opens the grand lid to reflect upper soundboard radiation toward the listener."),
        "Felt hardness" => ("Hammer felt hardness", "Stiffer felt produces a briefer contact and brighter attack."),
        "Hammer mass" => ("Hammer weight", "Scales hammer mass across the compass; heavier hammers stay in contact longer."),
        "Felt damping" => ("Felt damping", "Internal energy absorbed inside the wool felt during impact."),
        "Strike point" => ("Strike position", "Where the hammer hits along the string, shaping which harmonics are excited."),
        "Action knocks" => ("Action knocks", "Keybed landing, return rail, and backcheck knocks conducted into the case."),
        "Let-off distance" => ("Let-off gap", "Free-flight distance from jack escapement to the string."),
        "Shank springiness" => ("Hammer shank stiffness", "Higher resonance frequency means a stiffer wooden shank during impact."),
        "Damper strength" => ("Damper stopping power", "How quickly a released damper silences its vibrating wires."),
        "Damper weight" => ("Damper resting weight", "Downward force of the damper felt on the wires when at rest."),
        "Pedal noise" => ("Pedal noise", "Sustain-pedal rail knocks and mechanism friction."),
        "Pitch of A4" => ("Tuning reference: A4", "Concert pitch reference used to tune all 88 notes."),
        "Unison detune" => ("Unison spread", "Subtle pitch differences between wires of the same note create slow beating."),
        "Internal loss" => ("String damping", "Internal damping of the steel and copper-wound wire itself."),
        "Wire stiffness" => ("Wire bending stiffness", "Increases inharmonicity, stretching upper overtones sharper."),
        "Downbearing angle" => ("Bridge downbearing", "Angle the strings break over the bridge; couples string tension into the board."),
        "Strike angle" => ("Hammer strike angle", "Off-vertical strike component that excites sideways string vibration."),
        "Thickness" => ("Wood thickness", "Spruce soundboard thickness; thicker wood is stiffer and shifts body modes."),
        "Loss factor" => ("Wood damping", "Internal damping of the spruce soundboard."),
        "Rib height" => ("Rib height", "Height of the spruce stiffening ribs glued beneath the soundboard."),
        "Rib spacing" => ("Distance between ribs", "Center-to-center spacing of the ribs running across the spruce grain."),
        "Crown radius" => ("Soundboard crown", "Dome curvature of the board; smaller radius means a stiffer, more domed plate."),
        "Rim grip" => ("Soundboard rim clamping", "How rigidly the outer wooden rim clamps the soundboard perimeter: 0 lets the edge flex, 1 locks it stiffly."),
        "Bridge height" => ("Bridge height", "Height of the hardwood bridges coupling string motion into the soundboard."),
        "Room (walls, floor, ceiling, reverberation)" => ("Room acoustics", "Enables walls, floor, ceiling reflections and late room reverberation. The lid is independent."),
        "Stretch tuning (octaves tuned by matching overtones)" => ("Stretch-tuned octaves", "Matches upper overtones across octaves like a concert tuner."),
        "Longitudinal motion (stretching: phantom partials, bass ring)" => ("String stretching motion", "Includes axial wire stretching, adding phantom partials and bass bloom."),
        "Duplex lengths behind the bridge (treble shimmer)" => ("Duplex wire (behind bridge)", "Unstruck rear wire segments behind the treble bridge ring sympathetically."),
        "Damped strings keep ringing faintly (costs as much as holding the pedal)" => ("Keep damped strings active", "Simulates faint sympathetic response even under resting dampers (higher CPU)."),
        "Sideways string motion (of struck strings)" => ("Sideways string motion", "Simulates horizontal string polarization alongside vertical motion."),
        "Air loading (the air's mass and drag on the board)" => ("Air loading on soundboard", "Surrounding air mass and radiation drag acting back on the wooden panel."),
        "Piano position, along the keyboard" => ("Piano left / right", "Horizontal piano placement on the room floor plan."),
        "Piano position, along the strings" => ("Piano front / back", "Depth placement of the piano on the room floor plan."),
        "Listener position, along the keyboard" => ("Listener left / right", "Horizontal listener placement on the room floor plan."),
        "Listener position, along the strings" => ("Listener front / back", "Depth placement of the listener on the room floor plan."),
        _ => (name, "Adjusts this part of the simulation."),
    }
}

pub(super) fn value(p: &piano_engine::params::Param, v: f64) -> String {
    if p.kind == piano_engine::params::Kind::Switch {
        if v >= 0.5 { "On".into() } else { "Off".into() }
    } else if p.name == "Room shape" {
        RoomShape::from_value(v).name().into()
    } else if p.name == "Ceiling slope" {
        let deg = v.round() as i32;
        if deg == 0 { "0° (flat)".into() } else { format!("{deg:+}°") }
    } else if p.name == "Wall splay" || p.name == "Wall skew" {
        let pct = (v * 100.0).round() as i32;
        if pct == 0 { "0%".into() } else { format!("{pct:+}%") }
    } else if p.name == "Wall absorption" {
        format!("{:.0}%", v * 100.0)
    } else if p.name == "Rim grip" {
        format!("{:.0}%", v * 100.0)
    } else if p.format.starts_with("x ") {
        format!("{v:.2}×")
    } else {
        value_label(p.format, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_engine_control_has_a_plain_explanation_without_parallel_ranges() {
        for p in PARAMS {
            let (label, why) = copy(p.name);
            assert!(!label.is_empty() && !why.is_empty(), "{}", p.name);
            assert_ne!(
                why, "Adjusts this part of the simulation.",
                "Missing specific explanation for {}",
                p.name
            );
        }
    }
}
