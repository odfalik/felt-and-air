//! Unified multi-lane Strings visual: merges geometry/tuning and live wave motion into
//! a single diagram where every unison wire (1–3) gets its own distinct horizontal lane.

use super::*;

pub(super) const WIRE_COLORS: [Color32; 3] = [
    AMBER,
    Color32::from_rgb(106, 158, 187),
    Color32::from_rgb(157, 178, 143),
];

pub(super) fn paint(state: &mut State, ui: &mut Ui, setter: &ParamSetter, size: Vec2) {
    let mut o = state.params.options();
    let active = PARAMS[state.active_param].name;
    let midi = if state.snap.focus_midi >= 21 {
        state.snap.focus_midi
    } else {
        60
    };
    let note = state
        .preview_design
        .notes
        .iter()
        .find(|n| n.midi == midi)
        .unwrap_or(&state.preview_design.notes[0]);
    let wire_count = note.strings.len().clamp(1, 3);
    let has_live = state.snap.focus_midi >= 21;

    let (r, response) = ui.allocate_exact_size(size, Sense::hover());
    #[cfg(test)]
    trace_left_target(ui, "strings-panel", response.rect);
    response.on_hover_text(
        "Each unison wire of the active note in its own lane: left to right is speaking length from agraffe to bridge.",
    );

    let p = ui.painter_at(r);
    p.rect_filled(r, 8.0, PANEL);

    // Header strip inside canvas
    let top_y = r.top() + 12.0;
    p.text(
        pos2(r.left() + 16.0, top_y),
        Align2::LEFT_TOP,
        if has_live {
            format!("{} · {} {}", note_name(midi), wire_count, if wire_count == 1 { "wire" } else { "wires" })
        } else {
            format!("{} preview · {} wires", note_name(midi), wire_count)
        },
        FontId::proportional(13.0),
        Color32::from_rgb(232, 231, 221),
    );
    let total_force: f32 = state.snap.hammer_force.iter().sum();
    let right_hdr = if has_live && total_force > 0.05 {
        format!("Felt contact: {total_force:.1} N")
    } else if has_live {
        if state.snap.damper_span[1] > state.snap.damper_span[0] {
            if state.snap.damper_lift > 0.75 {
                "Damper lifted · ringing".into()
            } else if state.snap.damper_lift < 0.25 {
                "Damper resting".into()
            } else {
                format!("Half-damped {:.0}%", (1.0 - state.snap.damper_lift) * 100.0)
            }
        } else {
            "Undamped treble register".into()
        }
    } else {
        "Play a note to see live wire vibration".into()
    };
    p.text(
        pos2(r.right() - 16.0, top_y + 1.0),
        Align2::RIGHT_TOP,
        right_hdr,
        FontId::proportional(11.0),
        if total_force > 0.05 { AMBER } else { MUTED },
    );

    // Update smooth peak auto-scale
    let peak = state
        .snap
        .shape
        .iter()
        .zip(state.snap.shape_nodes)
        .flat_map(|(s, n)| s[..n.min(s.len())].iter())
        .fold(1e-7f32, |m, x| m.max(x.abs()));
    state.string_scale = peak.max(state.string_scale * 0.98).max(1e-6);

    // Layout: speaking length [x0 .. bridge_x], optional duplex tail [bridge_x .. duplex_x], Hz column on right
    let show_duplex = o.duplex && !note.duplex.is_empty();
    let hz_col_w = 88.0;
    let x0 = r.left() + 26.0;
    let right_limit = r.right() - hz_col_w - 16.0;
    let duplex_w = if show_duplex { (right_limit - x0) * 0.14 } else { 0.0 };
    let bridge_x = right_limit - duplex_w;
    let span_w = (bridge_x - x0).max(40.0);

    let lane_top = r.top() + 50.0;
    let lane_bot = r.bottom() - 42.0;
    let total_lane_h = (lane_bot - lane_top).max(60.0);
    let lane_h = total_lane_h / wire_count as f32;
    let wave_amp = (lane_h * 0.34).min(48.0);

    let strike_hl = active == "Strike point" || active == "Strike angle";
    let bridge_hl = active == "Downbearing angle" || active == "Bridge height";
    let duplex_hl = active == "Duplex lengths behind the bridge (treble shimmer)";
    let tuning_hl = matches!(active, "Pitch of A4" | "Unison detune" | "Stretch tuning (octaves tuned by matching overtones)");

    // Structural endpoint posts: Agraffe (left) and Bridge (right)
    let post_top = lane_top - 6.0;
    let post_bot = lane_bot + 4.0;
    p.line_segment(
        [pos2(x0, post_top), pos2(x0, post_bot)],
        Stroke::new(2.2, Color32::from_rgb(138, 144, 142)),
    );
    p.line_segment(
        [pos2(bridge_x, post_top), pos2(bridge_x, post_bot)],
        Stroke::new(
            if bridge_hl { 3.6 } else { 2.6 },
            if bridge_hl { AMBER } else { Color32::from_rgb(192, 174, 140) },
        ),
    );
    p.text(
        pos2(x0, post_bot + 4.0),
        Align2::LEFT_TOP,
        "Agraffe",
        FontId::proportional(10.0),
        MUTED,
    );
    p.text(
        pos2(bridge_x, post_bot + 4.0),
        Align2::CENTER_TOP,
        "Bridge",
        FontId::proportional(10.0),
        if bridge_hl { AMBER } else { Color32::from_rgb(192, 174, 140) },
    );

    // Hammer strike position vertical guide (directly draggable left/right!)
    let strike_x = x0 + span_w * note.strike_ratio.clamp(0.03, 0.5) as f32;
    let strike_glow = (total_force / 25.0).clamp(0.0, 1.0);
    if strike_glow > 0.02 {
        p.line_segment(
            [pos2(strike_x, post_top), pos2(strike_x, post_bot)],
            Stroke::new(6.0, AMBER.gamma_multiply(0.22 * strike_glow)),
        );
    }
    p.line_segment(
        [pos2(strike_x, post_top), pos2(strike_x, post_bot)],
        Stroke::new(
            if strike_hl || strike_glow > 0.05 { 1.8 } else { 1.1 },
            AMBER.gamma_multiply(if strike_hl || strike_glow > 0.05 { 0.95 } else { 0.60 }),
        ),
    );
    let strike_handle = pos2(strike_x, post_top + 2.0);
    p.circle_filled(strike_handle, 3.8, AMBER);
    p.text(
        pos2(strike_x, post_top - 6.0),
        Align2::CENTER_BOTTOM,
        format!("Strike ({:.0}%)", note.strike_ratio * 100.0),
        FontId::proportional(10.0),
        AMBER,
    );
    let strike_hit = ui.interact(
        Rect::from_min_max(pos2(strike_x - 10.0, post_top - 4.0), pos2(strike_x + 10.0, post_bot + 4.0)),
        ui.id().with("strings-strike-drag"),
        Sense::drag(),
    );
    if strike_hit.hovered() {
        p.circle_stroke(strike_handle, 6.5, Stroke::new(1.2, AMBER));
    }
    if strike_hit.dragged() {
        let base_ratio = (note.strike_ratio / o.strike_position.max(0.1)).max(0.04);
        let d_ratio = strike_hit.drag_delta().x as f64 / span_w.max(40.0) as f64;
        o.strike_position = (o.strike_position + d_ratio / base_ratio).clamp(0.5, 1.8);
        state.apply_options(setter, &o);
        state.active_param = PARAMS.iter().position(|p| p.name == "Strike point").unwrap_or(state.active_param);
        state.preset = "Custom settings".into();
    }

    // Real damper pad indicator (drawn at exact engine damper_span, lifting with damper_lift!)
    let (d0, d1) = if has_live && state.snap.damper_span[1] > state.snap.damper_span[0] {
        (state.snap.damper_span[0], state.snap.damper_span[1])
    } else if note.has_damper {
        (0.16, 0.28)
    } else {
        (0.0, 0.0)
    };
    if d1 > d0 {
        let dx0 = x0 + span_w * d0.clamp(0.0, 1.0);
        let dx1 = x0 + span_w * d1.clamp(0.0, 1.0);
        let lift = if has_live { state.snap.damper_lift } else { 0.0 };
        // Draw subtle damper felt column across the lanes; solid when resting, lifted/faint when up
        let damp_alpha = 0.26 * (1.0 - 0.72 * lift);
        let damp_rect = Rect::from_min_max(pos2(dx0, post_top + 2.0), pos2(dx1, post_bot - 2.0));
        p.rect_filled(damp_rect, 3.0, Color32::from_rgb(176, 178, 168).gamma_multiply(damp_alpha));
        p.rect_stroke(
            damp_rect,
            3.0,
            Stroke::new(
                1.0,
                Color32::from_rgb(184, 188, 178).gamma_multiply(0.55 * (1.0 - 0.55 * lift)),
            ),
            StrokeKind::Inside,
        );
        p.text(
            pos2((dx0 + dx1) * 0.5, post_top - 4.0),
            Align2::CENTER_BOTTOM,
            if lift > 0.75 { "Damper up" } else { "Damper" },
            FontId::proportional(9.5),
            MUTED.gamma_multiply(1.0 - 0.35 * lift),
        );
    }

    // Individual separated horizontal wire lanes
    for i in 0..wire_count {
        let cy = lane_top + lane_h * (i as f32 + 0.5);
        let lane_rect = Rect::from_min_max(
            pos2(x0 + 2.0, lane_top + lane_h * i as f32 + 4.0),
            pos2(right_limit - 2.0, lane_top + lane_h * (i + 1) as f32 - 4.0),
        );
        p.rect_filled(lane_rect, 4.0, Color32::from_rgb(26, 31, 34).gamma_multiply(0.7));

        // Rest centerline for this wire
        p.line_segment(
            [pos2(x0, cy), pos2(bridge_x, cy)],
            Stroke::new(0.8, Color32::from_gray(52)),
        );

        let col = WIRE_COLORS[i.min(2)];
        let n = state.snap.shape_nodes[i];
        if has_live && n >= 2 {
            let pts: Vec<Pos2> = (0..n)
                .map(|j| {
                    let frac = j as f32 / (n - 1) as f32;
                    let disp = state.snap.shape[i][j] / state.string_scale * wave_amp;
                    pos2(x0 + span_w * frac, cy - disp)
                })
                .collect();
            // Subtle glow behind active wire
            p.add(Shape::line(pts.clone(), Stroke::new(3.2, col.gamma_multiply(0.18))));
            p.add(Shape::line(pts, Stroke::new(1.7, col)));
        } else {
            p.line_segment([pos2(x0, cy), pos2(bridge_x, cy)], Stroke::new(1.5, col.gamma_multiply(0.8)));
        }

        // Contact dot on wire when hammer hits
        if has_live && state.snap.hammer_force[i] > 0.05 {
            p.circle_filled(pos2(strike_x, cy), 3.6, AMBER);
        }

        // Rear duplex wire segment past the bridge (when enabled & present on this note)
        if show_duplex {
            let du_end = right_limit - 4.0;
            let du_col = if duplex_hl { AMBER } else { col.gamma_multiply(0.55) };
            p.line_segment([pos2(bridge_x, cy), pos2(du_end, cy)], Stroke::new(1.2, du_col));
            p.circle_filled(pos2(du_end, cy), 2.0, du_col);
        }

        // Per-wire pitch badge on the right
        let wire = &note.strings[i];
        p.text(
            pos2(r.right() - 16.0, cy),
            Align2::RIGHT_CENTER,
            format!("{:.2} Hz", wire.f0),
            FontId::monospace(11.0),
            if tuning_hl { AMBER } else { col },
        );
    }

    if show_duplex {
        p.text(
            pos2((bridge_x + right_limit) * 0.5, post_bot + 4.0),
            Align2::CENTER_TOP,
            "Duplex",
            FontId::proportional(9.5),
            if duplex_hl { AMBER } else { MUTED },
        );
    }

    // Bottom footer readout
    p.text(
        pos2(r.left() + 16.0, r.bottom() - 8.0),
        Align2::LEFT_BOTTOM,
        format!(
            "Speaking length {:.2} m · magnified ±{:.3} mm",
            note.strings[0].length,
            state.string_scale * 1000.0
        ),
        FontId::monospace(10.0),
        MUTED,
    );
}
