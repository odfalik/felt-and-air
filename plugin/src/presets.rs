//! Built-in setups from the original app. Each preset changes only its named part of the
//! instrument, except the reference setup which resets everything.
use piano_engine::GrandOptions;
pub const NAMES: &[&str] = &[
    "Recital room · listener by the lid",
    "At the keyboard",
    "Small studio · close",
    "Concert hall · tenth row",
    "Stone chapel",
    "Open air · over the strings",
    "Voicing · bright",
    "Voicing · mellow",
    "Voicing · reference",
    "Honky-tonk tuning",
    "Light · busy machine",
    "Full · every part on",
];
pub fn apply(i: usize, o: &mut GrandOptions) {
    let d = GrandOptions::default();
    match i {
        0 => *o = d,
        1 => {
            o.room_enabled = true;
            o.room_lx = d.room_lx;
            o.room_ly = d.room_ly;
            o.room_lz = d.room_lz;
            o.room_absorption = d.room_absorption;
            o.piano_x = d.piano_x;
            o.piano_y = d.piano_y;
            o.listener_x = d.piano_x + 0.7;
            o.listener_y = d.piano_y - 0.45;
            o.ear_height_m = 1.2;
            o.ear_spacing_m = 0.18;
        }
        2 => {
            o.room_enabled = true;
            o.room_lx = 5.0;
            o.room_ly = 4.2;
            o.room_lz = 2.7;
            o.room_absorption = 0.4;
            o.piano_x = 1.2;
            o.piano_y = 1.3;
            o.listener_x = 3.6;
            o.listener_y = 1.9;
            o.ear_height_m = 1.3;
            o.ear_spacing_m = 0.5;
        }
        3 => {
            o.room_enabled = true;
            o.room_lx = 28.0;
            o.room_ly = 18.0;
            o.room_lz = 12.0;
            o.room_absorption = 0.16;
            o.piano_x = 5.0;
            o.piano_y = 7.0;
            o.listener_x = 15.0;
            o.listener_y = 7.8;
            o.ear_height_m = 1.2;
            o.ear_spacing_m = 0.3;
        }
        4 => {
            o.room_enabled = true;
            o.room_lx = 16.0;
            o.room_ly = 8.0;
            o.room_lz = 9.0;
            o.room_absorption = 0.06;
            o.piano_x = 4.0;
            o.piano_y = 2.5;
            o.listener_x = 9.0;
            o.listener_y = 3.2;
            o.ear_height_m = 1.2;
            o.ear_spacing_m = 0.4;
        }
        5 => {
            o.room_enabled = false;
            o.lid_angle_deg = 0.0;
            o.listener_x = o.piano_x + 0.7;
            o.listener_y = o.piano_y + 0.5;
            o.ear_height_m = 1.65;
            o.ear_spacing_m = 0.7;
        }
        6 => {
            o.hammer_hardness = 2.2;
            o.felt_damping = 0.6;
            o.hammer_mass = 0.9;
        }
        7 => {
            o.hammer_hardness = 0.45;
            o.felt_damping = 1.6;
            o.hammer_mass = 1.1;
        }
        8 => {
            o.hammer_hardness = d.hammer_hardness;
            o.felt_damping = d.felt_damping;
            o.hammer_mass = d.hammer_mass;
            o.strike_position = d.strike_position;
        }
        9 => o.unison_detune_cents = 5.0,
        10 => {
            o.two_polarisations = false;
            o.duplex = false;
            o.air_loading = false;
            o.damped_strings_ring = false;
        }
        11 => {
            o.two_polarisations = true;
            o.duplex = true;
            o.air_loading = true;
            o.longitudinal = true;
            o.damped_strings_ring = true;
        }
        _ => {}
    }
}
