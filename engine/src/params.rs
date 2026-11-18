//! The adjustable quantities of the instrument, as one table for every front end: the app's
//! Instrument window, the Audio Unit's parameters and the plug-in's. Every one is a physical
//! number. Sliders come first, then switches, then positions on the floor plan.

use crate::design::GrandOptions;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Slider,
    Switch,
    /// A coordinate on the floor plan, metres.
    Position,
}

pub const TAB_NAMES: [&str; 4] = ["Space", "Hammers", "Strings", "Soundboard"];

pub struct Param {
    pub kind: Kind,
    /// Index into `TAB_NAMES`.
    pub tab: usize,
    pub name: &'static str,
    pub lo: f64,
    pub hi: f64,
    /// Best shown on a logarithmic slider.
    pub log: bool,
    /// printf-style format of the value with its unit.
    pub format: &'static str,
    pub help: &'static str,
    pub get: fn(&GrandOptions) -> f64,
    pub set: fn(&mut GrandOptions, f64),
    // The same strings with a terminating NUL, for C.
    pub(crate) c_name: &'static str,
    pub(crate) c_format: &'static str,
    pub(crate) c_help: &'static str,
}

impl Param {
    /// Writes a value, held inside the parameter's range.
    pub fn write(&self, o: &mut GrandOptions, v: f64) {
        (self.set)(o, v.clamp(self.lo, self.hi));
    }
}

macro_rules! slider {
    ($tab:expr, $name:expr, $field:ident, $lo:expr, $hi:expr, $log:expr, $format:expr, $help:expr) => {
        Param {
            kind: Kind::Slider,
            tab: $tab,
            name: $name,
            lo: $lo,
            hi: $hi,
            log: $log,
            format: $format,
            help: $help,
            get: |o| o.$field,
            set: |o, v| o.$field = v,
            c_name: concat!($name, "\0"),
            c_format: concat!($format, "\0"),
            c_help: concat!($help, "\0"),
        }
    };
}
macro_rules! switch {
    ($tab:expr, $name:expr, $field:ident) => {
        Param {
            kind: Kind::Switch,
            tab: $tab,
            name: $name,
            lo: 0.0,
            hi: 1.0,
            log: false,
            format: "%.0f",
            help: "",
            get: |o| if o.$field { 1.0 } else { 0.0 },
            set: |o, v| o.$field = v >= 0.5,
            c_name: concat!($name, "\0"),
            c_format: "%.0f\0",
            c_help: "\0",
        }
    };
}
macro_rules! position {
    ($name:expr, $field:ident) => {
        Param {
            kind: Kind::Position,
            tab: 0,
            name: $name,
            lo: 0.2,
            hi: 30.0,
            log: false,
            format: "%.2f m",
            help: "",
            get: |o| o.$field,
            set: |o, v| o.$field = v,
            c_name: concat!($name, "\0"),
            c_format: "%.2f m\0",
            c_help: "\0",
        }
    };
}

pub static PARAMS: &[Param] = &[
    slider!(0, "Room length", room_lx, 3.0, 30.0, true, "%.1f m", "Along the keyboard direction."),
    slider!(0, "Room width", room_ly, 3.0, 30.0, true, "%.1f m", "Along the strings."),
    slider!(0, "Room height", room_lz, 2.2, 15.0, true, "%.1f m", ""),
    slider!(0, "Wall absorption", room_absorption, 0.03, 0.7, true, "%.2f", "Share of sound each surface soaks up. Low is a tiled hall, high is a carpeted studio."),
    slider!(0, "Ear height", ear_height_m, 0.6, 2.5, false, "%.2f m", "Above the floor. The soundboard sits at 0.75 m."),
    slider!(0, "Ear spacing", ear_spacing_m, 0.1, 1.5, false, "%.2f m", "A spaced pair; wider gives a wider image."),
    slider!(0, "Lid angle", lid_angle_deg, 0.0, 60.0, false, "%.0f deg", "0 takes the lid's reflection away."),
    slider!(1, "Felt hardness", hammer_hardness, 0.2, 5.0, true, "x %.2f", "Felt stiffness. Harder is brighter."),
    slider!(1, "Hammer mass", hammer_mass, 0.5, 2.0, true, "x %.2f", "Heavier stays on the string longer: louder and duller."),
    slider!(1, "Felt damping", felt_damping, 0.0, 5.0, false, "x %.2f", "Energy the felt loses as it is squeezed."),
    slider!(1, "Strike point", strike_position, 0.5, 1.8, false, "x %.2f", "Distance from the agraffe; the reference is about 1/8 of the string."),
    slider!(1, "Key knock", key_thump, 0.0, 30.0, false, "x %.1f", "Knock of the key landing on the keybed."),
    slider!(1, "Let-off distance", let_off_mm, 0.5, 5.0, false, "%.1f mm", "Gap to the string when the hammer is let go. It flies the rest, slowed by gravity."),
    slider!(1, "Shank springiness", shank_hz, 80.0, 2000.0, true, "%.0f Hz", "The wooden shank flexes at impact. High is a rigid hammer."),
    slider!(1, "Damper strength", damper_strength, 0.2, 4.0, true, "x %.2f", "How fast a released note is stopped."),
    slider!(1, "Damper weight", damper_press_n, 0.0, 0.6, false, "%.2f N", "Force each damper rests on its string with. Lifting it lets the string stir."),
    slider!(1, "Pedal and key noise", mechanism_noise, 0.0, 10.0, false, "x %.1f", "Knocks and friction of the pedal work and returning keys."),
    slider!(2, "Pitch of A4", a4_hz, 415.0, 466.0, false, "%.1f Hz", ""),
    slider!(2, "Unison detune", unison_detune_cents, 0.0, 6.0, false, "%.2f cents", "Spread between the strings of one note. More gives beating, then honky-tonk."),
    slider!(2, "Internal loss", string_loss, 0.25, 5.0, true, "x %.2f", "The string's own damping, apart from what the board takes."),
    slider!(2, "Wire stiffness", string_stiffness, 0.25, 4.0, true, "x %.2f", "Bending stiffness. More makes the overtones sharper."),
    slider!(2, "Downbearing angle", downbearing_deg, 0.0, 5.0, false, "%.1f deg", "Bend of the strings over the bridge; sets how hard stretching presses on the board."),
    slider!(2, "Strike angle", hammer_tilt_deg, 0.0, 15.0, false, "%.1f deg", "How far off the upright the hammer hits; this is what sets strings moving sideways."),
    slider!(3, "Thickness", board_thickness_mm, 5.0, 13.0, false, "%.1f mm", ""),
    slider!(3, "Loss factor", board_loss_percent, 0.5, 8.0, true, "%.1f %%", "Internal damping of the wood."),
    slider!(3, "Rib height", rib_height_mm, 10.0, 36.0, false, "%.0f mm", ""),
    slider!(3, "Rib spacing", rib_spacing_mm, 80.0, 220.0, false, "%.0f mm", ""),
    slider!(3, "Crown radius", crown_radius_m, 6.0, 80.0, true, "%.0f m", "Smaller is a more domed, stiffer board."),
    slider!(3, "Rim grip", rim_clamp, 0.0, 1.0, false, "%.2f", "0: edge free to tilt. 1: edge clamped."),
    slider!(3, "Bridge height", bridge_height_mm, 20.0, 50.0, false, "%.0f mm", ""),
    switch!(0, "Room (walls, floor, ceiling, reverberation)", room_enabled),
    switch!(2, "Stretch tuning (octaves tuned by matching overtones)", stretch_tuning),
    switch!(2, "Longitudinal motion (stretching: phantom partials, bass ring)", longitudinal),
    switch!(2, "Duplex lengths behind the bridge (treble shimmer)", duplex),
    switch!(2, "Damped strings keep ringing faintly (costs as much as holding the pedal)", damped_strings_ring),
    switch!(2, "Sideways string motion (of struck strings)", two_polarisations),
    switch!(3, "Air loading (the air's mass and drag on the board)", air_loading),
    position!("Piano position, along the keyboard", piano_x),
    position!("Piano position, along the strings", piano_y),
    position!("Listener position, along the keyboard", listener_x),
    position!("Listener position, along the strings", listener_y),
];
