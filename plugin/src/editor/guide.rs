//! Explanatory UI only. Parameter ranges/writes and preview geometry come from the engine.
//! Schematics and the Sabine estimate are deliberately separate from live motion snapshots.
use super::*;
use piano_engine::PianoDesign;

pub(super) const TABS: [&str; 4] = ["Room", "Action", "Strings", "Soundboard"];

pub(super) fn category_help(tab: usize) -> &'static str {
    match tab {
        0 => "Move the piano or listener in the room plan; compare an estimated room tail as you adjust.",
        1 => "Authored hammer section driven by measured hammer motion and return, with recorded contact cues. The engine begins at let-off; no full lever upswing is simulated. Waiting/paused states are explicit.",
        2 => "Model geometry and target tuning above; actual simulated wire motion below.",
        _ => "The wooden panel below the strings. Simulated displacement is magnified, not loudness or air pressure.",
    }
}

/// Presentation copy, not another parameter/default/range table.
pub(super) fn copy(name: &str) -> (&str, &str) {
    match name {
        "Room length" => ("Room length", "A larger room usually lets reflections linger longer."),
        "Room width" => ("Room width", "Changes reflection paths and how long the room rings."),
        "Room height" => ("Ceiling height", "A higher ceiling changes the room volume and decay."),
        "Wall absorption" => ("Wall softness", "More absorption means fewer reflections and a shorter tail."),
        "Ear height" => ("Pickup height", "Moves both virtual listening points above or below the board."),
        "Ear spacing" => ("Stereo spacing", "Moves the two pickup points apart; changes the stereo image."),
        "Lid angle" => ("Lid opening", "Changes the lid reflection. Zero removes that reflection."),
        "Felt hardness" => ("Hammer felt hardness", "Harder felt usually gives a brighter, sharper attack."),
        "Hammer mass" => ("Hammer weight", "Scales each key’s hammer mass. Heavier hammers stay in contact longer."),
        "Felt damping" => ("Felt energy loss", "More energy is lost inside the felt during a strike."),
        "Strike point" => ("Where the hammer strikes", "Changes which overtones the hammer excites, not just volume."),
        "Action knocks" => ("Action knocks", "Strength of keybed landing, key return rail and hammer backcheck/rest knocks."),
        "Let-off distance" => ("Hammer’s free-flight gap", "The hammer flies this last distance without the action pushing it."),
        "Shank springiness" => ("Hammer-shank stiffness", "Higher resonance means a less flexible shank during impact."),
        "Damper strength" => ("Damper stopping power", "Controls how strongly a released string is damped."),
        "Damper weight" => ("Damper resting force", "Sets how firmly the damper rests on each string."),
        "Pedal noise" => ("Pedal noise", "Sustain-pedal endpoint knocks and moving friction; not returning-key sounds."),
        "Pitch of A4" => ("Tuning reference: A4", "Sets the reference pitch used to tune the instrument."),
        "Unison detune" => ("Spread within one note", "Slightly different wire pitches create slow beating; more spread beats faster."),
        "Internal loss" => ("String energy loss", "More internal loss makes the strings’ own vibration decay faster."),
        "Wire stiffness" => ("Wire bending stiffness", "Stiffness stretches upper partials sharp; the model also tunes each note toward its target."),
        "Downbearing angle" => ("String bend over the bridge", "Changes how string stretching presses on the soundboard."),
        "Strike angle" => ("Hammer sideways angle", "An off-upright strike excites the other direction of string motion."),
        "Thickness" => ("Wood thickness", "Thicker wood is harder to bend; this changes the body’s resonances."),
        "Loss factor" => ("Wood energy loss", "More internal wood loss makes the board’s motion die away faster."),
        "Rib height" => ("Rib height", "Taller ribs stiffen the wooden panel from underneath."),
        "Rib spacing" => ("Distance between ribs", "Wider spacing means fewer supports across the panel."),
        "Crown radius" => ("Board curvature", "A smaller radius means a more domed, stiffer soundboard."),
        "Rim grip" => ("How tightly the edge is held", "Zero lets the edge tilt; one clamps it firmly."),
        "Bridge height" => ("Bridge height", "Changes how sideways string motion couples to the wooden panel."),
        "Room (walls, floor, ceiling, reverberation)" => ("Room reflections & reverb", "Enables walls, floor, ceiling and the late room tail. The lid is separate."),
        "Stretch tuning (octaves tuned by matching overtones)" => ("Stretch-tuned octaves", "Matches overtones across octaves instead of using equal-tempered fundamentals alone."),
        "Longitudinal motion (stretching: phantom partials, bass ring)" => ("String stretching motion", "Adds the simulated longitudinal motion and its extra ringing components."),
        "Duplex lengths behind the bridge (treble shimmer)" => ("Free treble wire behind the bridge", "Lets those unstruck wire lengths ring along with the instrument."),
        "Damped strings keep ringing faintly (costs as much as holding the pedal)" => ("Keep damped strings active", "Includes their faint response to the board, at a substantial CPU cost."),
        "Sideways string motion (of struck strings)" => ("Second string vibration direction", "Follows sideways motion of struck strings as well as upright motion."),
        "Air loading (the air's mass and drag on the board)" => ("Air’s load on the wood", "Adds the surrounding air’s mass and drag; this is not room reverb."),
        "Piano position, along the keyboard" => ("Piano left / right", "Move the piano in the room plan; the sound paths change."),
        "Piano position, along the strings" => ("Piano front / back", "Move the piano in the room plan; the sound paths change."),
        "Listener position, along the keyboard" => ("Listener left / right", "Move the virtual listening position, not the piano."),
        "Listener position, along the strings" => ("Listener front / back", "Move the virtual listening position, not the piano."),
        _ => (name, "Adjusts this part of the simulation."),
    }
}

pub(super) fn value(p: &piano_engine::params::Param, v: f64) -> String {
    if p.kind == piano_engine::params::Kind::Switch {
        if v >= 0.5 {
            "On".into()
        } else {
            "Off".into()
        }
    } else if p.name == "Wall absorption" {
        format!("{:.0}% absorbed", v * 100.0)
    } else if p.format.starts_with("x ") {
        format!("{v:.2}× reference")
    } else {
        value_label(p.format, v)
    }
}

/// An estimate of the room’s late decay, not a sampled pressure envelope.
/// Calls the pure engine estimate, never constructing a Piano while dragging.
pub(super) fn room_decay(o: &GrandOptions) -> f64 {
    if !o.room_enabled {
        return 0.0;
    }
    piano_engine::acoustics::room_decay_time(o.room_lx, o.room_ly, o.room_lz, o.room_absorption)
}
fn text(p: &Painter, pos: Pos2, align: Align2, s: impl ToString, color: Color32) {
    p.text(pos, align, s, FontId::proportional(11.0), color);
}

pub(super) fn decay(ui: &mut Ui, o: &GrandOptions, plot_height: f32) {
    let tail = room_decay(o);
    ui.label(
        RichText::new(if tail > 0.0 {
            format!("Room tail ≈ {tail:.2} s")
        } else {
            "Room tail off".into()
        })
        .strong()
        .color(AMBER),
    );
    ui.label(
        RichText::new("Time for echoes to become very quiet (−60 dB)")
            .small()
            .color(MUTED),
    );
    let (r, _response) = ui.allocate_exact_size(vec2(ui.available_width(), plot_height), Sense::hover());
    #[cfg(test)]
    trace_left_target(ui,"decay-plot",_response.rect);
    let plot = Rect::from_min_max(r.min + vec2(42.0, 8.0), r.max - vec2(8.0, 18.0));
    let p = ui.painter();
    text(
        p,
        pos2(plot.left() - 5.0, plot.top()),
        Align2::RIGHT_TOP,
        "0 dB",
        MUTED,
    );
    text(
        p,
        pos2(plot.left() - 5.0, plot.bottom()),
        Align2::RIGHT_BOTTOM,
        "−60 dB",
        MUTED,
    );
    p.line_segment(
        [plot.left_top(), plot.left_bottom()],
        Stroke::new(0.7, MUTED),
    );
    for second in 0..=4 {
        let x = plot.left() + plot.width() * second as f32 / 4.0;
        p.line_segment(
            [pos2(x, plot.top()), pos2(x, plot.bottom())],
            Stroke::new(0.5, Color32::from_gray(55)),
        );
        text(
            p,
            pos2(x, r.bottom()),
            Align2::CENTER_BOTTOM,
            format!("{second}s"),
            MUTED,
        );
    }
    for (time, color) in [
        (room_decay(&GrandOptions::default()), Color32::from_gray(94)),
        (tail, AMBER),
    ] {
        if time <= 0.0 {
            continue;
        }
        let points = (0..=80)
            .map(|i| {
                let t = 4.0 * i as f64 / 80.0;
                let db = (-60.0 * t / time).max(-60.0);
                pos2(
                    plot.left() + plot.width() * i as f32 / 80.0,
                    plot.top() + plot.height() * (-db / 60.0) as f32,
                )
            })
            .collect();
        p.add(Shape::line(points, Stroke::new(1.6, color)));
    }
    ui.label(
        RichText::new("First 4 s · Sabine estimate, not live audio. Gray: default room.")
            .small()
            .color(MUTED),
    );
}

pub(super) fn preview_note(
    design: &PianoDesign,
    focus: i32,
) -> Option<&piano_engine::design::NoteDesign> {
    let midi = if (21..=108).contains(&focus) {
        focus
    } else {
        60
    };
    design.notes.iter().find(|note| note.midi == midi)
}

pub(super) fn instrument(
    ui: &mut Ui,
    tab: usize,
    o: &GrandOptions,
    design: &PianoDesign,
    focus: i32,
    height: f32,
) {
    let Some(note) = preview_note(design, focus) else {
        return;
    };
    let midi = note.midi;
    let (r, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    #[cfg(test)]
    {
        let panel_snapshot=(r,ui.clip_rect());
        ui.ctx().data_mut(|data|data.insert_temp(Id::new(format!("compact:anatomy-panel:{tab}")),panel_snapshot));
    }
    response.on_hover_text(match tab {
        2 => "Geometry / target tuning preview, not measured sound. Colored wires make one note.",
        _ => "Wood / ribs cross-section, exaggerated and not to scale. Air loading is not room reverb.",
    });
    let p = ui.painter();
    p.rect_filled(r, 5.0, PANEL);
    let a = r.shrink2(vec2(14.0, 22.0));
    let zoom = (height / 125.0).clamp(0.85, 2.4);
    match tab {
        2 => {
            for (i, wire) in note.strings.iter().enumerate() {
                let y = a.top() + 15.0 + i as f32 * 20.0;
                p.line_segment(
                    [pos2(a.left(), y), pos2(a.right() - 80.0, y)],
                    Stroke::new(
                        1.4,
                        [AMBER, Color32::LIGHT_BLUE, Color32::from_rgb(157, 178, 143)][i.min(2)],
                    ),
                );
                text(
                    p,
                    pos2(a.right(), y),
                    Align2::RIGHT_CENTER,
                    format!("{:.2} Hz", wire.f0),
                    MUTED,
                );
            }
            let strike_x = a.left() + (a.width() - 80.0) * note.strike_ratio as f32;
            p.line_segment(
                [pos2(strike_x, a.top()), pos2(strike_x, a.bottom())],
                Stroke::new(1.0, AMBER),
            );
            text(
                p,
                pos2(strike_x, r.top() + 4.0),
                Align2::CENTER_TOP,
                "hammer strike",
                AMBER,
            );
            text(
                p,
                pos2(a.left(), r.bottom() - 5.0),
                Align2::LEFT_BOTTOM,
                format!(
                    "{} • {} wires • {:.2} m speaking length",
                    note_name(midi),
                    note.strings.len(),
                    note.strings[0].length
                ),
                MUTED,
            );
        }
        _ => {
            let y = a.top() + 42.0 * zoom;
            let thickness = (design.board.thickness * 1600.0) as f32 * zoom;
            let crown = (28.0 * 12.0 / o.crown_radius_m).min(32.0) as f32 * zoom;
            let top: Vec<_> = (0..=40)
                .map(|i| {
                    let x = i as f32 / 40.0;
                    pos2(a.left() + a.width() * x, y - crown * 4.0 * x * (1.0 - x))
                })
                .collect();
            p.add(Shape::line(
                top,
                Stroke::new(thickness, AMBER.gamma_multiply(0.7)),
            ));
            let spacing = (design.board.rib_spacing as f32 * 350.0).max(25.0);
            let mut x = a.left() + spacing / 2.0;
            while x < a.right() {
                let f = (x - a.left()) / a.width();
                let base = y - crown * 4.0 * f * (1.0 - f) + thickness / 2.0;
                p.rect_filled(
                    Rect::from_min_size(
                        pos2(x - 4.0, base),
                        vec2(8.0, (design.board.rib_height * 700.0) as f32),
                    ),
                    1.0,
                    Color32::from_rgb(131, 102, 68),
                );
                x += spacing;
            }
            let bridge_x = a.left() + a.width() * 0.55;
            let base = y - crown * 4.0 * 0.55 * 0.45 - thickness / 2.0;
            let bridge_h = (design.board.bridge_height * 400.0) as f32;
            p.rect_filled(
                Rect::from_min_max(
                    pos2(bridge_x - 10.0, base - bridge_h),
                    pos2(bridge_x + 10.0, base),
                ),
                1.0,
                Color32::from_rgb(195, 159, 106),
            );
            text(
                p,
                pos2(bridge_x + 14.0, base - bridge_h / 2.0),
                Align2::LEFT_CENTER,
                "Bridge",
                AMBER,
            );
            text(
                p,
                pos2(a.left() + 5.0, y + 24.0),
                Align2::LEFT_TOP,
                "Ribs beneath the wood",
                MUTED,
            );
            let caption = format!(
                "{:.1} mm wood • {:.0} mm ribs • {:.0} mm apart",
                o.board_thickness_mm, o.rib_height_mm, o.rib_spacing_mm
            );
            let galley = p.layout(caption, FontId::proportional(11.0), AMBER, a.width());
            p.galley(pos2(a.left(), r.bottom() - 7.0 - galley.size().y), galley, AMBER);
        }
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
    #[test]
    fn room_estimate_matches_applied_default_engine_and_responds_to_absorption() {
        let mut o = GrandOptions::default();
        let piano = piano_engine::Piano::new(
            &piano_engine::make_grand(&o),
            &piano_engine::PianoOptions {
                board_grid_scale: 1.5,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(room_decay(&o).to_bits(), piano.reverb_time().to_bits());
        let before = room_decay(&o);
        o.room_absorption *= 2.0;
        assert!((room_decay(&o) - before / 2.0).abs() < 1e-9);
        o.room_enabled = false;
        assert_eq!(room_decay(&o), 0.0);
    }
}
