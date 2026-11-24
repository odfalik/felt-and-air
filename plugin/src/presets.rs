//! Built-in complete instrument presets. Every preset resets `GrandOptions` to factory
//! defaults first so switching presets spans all settings cleanly with zero bleed-over.
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
    *o = d;
    match i {
        0 => {}
        1 => {
            o.listener_x = d.piano_x + 0.7;
            o.listener_y = d.piano_y - 0.45;
            o.ear_height_m = 1.2;
            o.ear_spacing_m = 0.18;
            o.lid_angle_deg = 32.0;
            o.binaural_head = true;
        }
        2 => {
            o.room_shape = 2.0; // Angled Studio
            o.room_lx = 5.0;
            o.room_ly = 4.2;
            o.room_lz = 2.7;
            o.room_absorption = 0.40;
            o.piano_x = 1.2;
            o.piano_y = 1.3;
            o.listener_x = 3.6;
            o.listener_y = 1.9;
            o.ear_height_m = 1.3;
            o.ear_spacing_m = 0.5;
            o.lid_angle_deg = 28.0;
        }
        3 => {
            o.room_shape = 1.0; // Concert Hall (Fan)
            o.ceiling_slope_deg = 4.0;
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
            o.lid_angle_deg = 52.0;
            o.hammer_hardness = 1.25;
            o.binaural_head = true;
        }
        4 => {
            o.ceiling_slope_deg = 8.0;
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
            o.hammer_hardness = 0.80;
            o.felt_damping = 1.15;
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
            o.lid_angle_deg = 48.0;
        }
        7 => {
            o.hammer_hardness = 0.45;
            o.felt_damping = 1.6;
            o.hammer_mass = 1.1;
            o.room_absorption = 0.28;
            o.lid_angle_deg = 22.0;
        }
        8 => {}
        9 => {
            o.room_lx = 5.6;
            o.room_ly = 4.6;
            o.room_lz = 2.8;
            o.room_absorption = 0.30;
            o.piano_x = 1.4;
            o.piano_y = 1.4;
            o.listener_x = 3.6;
            o.listener_y = 2.0;
            o.lid_angle_deg = 16.0;
            o.unison_detune_cents = 5.0;
            o.hammer_hardness = 1.85;
            o.felt_damping = 0.55;
            o.board_thickness_mm = 6.8;
        }
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
            o.binaural_head = true;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_preset_resets_all_prior_custom_settings_cleanly() {
        for i in 0..NAMES.len() {
            let mut from_clean = GrandOptions::default();
            apply(i, &mut from_clean);

            let mut from_dirty = GrandOptions::default();
            from_dirty.room_lx = 24.0;
            from_dirty.room_shape = 3.0;
            from_dirty.ceiling_slope_deg = -12.0;
            from_dirty.wall_splay = 0.25;
            from_dirty.wall_skew = -0.20;
            from_dirty.binaural_head = true;
            from_dirty.hammer_hardness = 3.8;
            from_dirty.unison_detune_cents = 4.5;
            from_dirty.board_thickness_mm = 11.5;
            from_dirty.rim_clamp = 0.15;
            apply(i, &mut from_dirty);

            assert_eq!(from_clean, from_dirty, "preset {i} ({}) leaked prior state", NAMES[i]);
        }
    }
}
