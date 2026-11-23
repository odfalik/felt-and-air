//! Merlin-inspired grouped dark-matte control cards and tactile rotary knobs.
//! Every parameter change uses proper `ParamSetter` begin/set/end host gestures.

use super::*;
use nih_plug::prelude::Param as NihParam;
use piano_engine::params::Param as EngineParam;

const CARD_BG: Color32 = Color32::from_rgb(22, 26, 30);
const CARD_BORDER: Color32 = Color32::from_rgb(44, 51, 56);
const DIAL_BG: Color32 = Color32::from_rgb(29, 34, 39);
const DIAL_HOVER: Color32 = Color32::from_rgb(38, 45, 51);
const TRACK_BG: Color32 = Color32::from_rgb(52, 60, 66);

pub(super) fn control_card(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(CARD_BG)
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(7.0)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 2.0).max(60.0));
            ui.horizontal(|ui| {
                let (dot_r, _) = ui.allocate_exact_size(vec2(5.0, 5.0), Sense::hover());
                ui.painter().circle_filled(dot_r.center(), 2.1, AMBER.gamma_multiply(0.85));
                ui.label(
                    RichText::new(title)
                        .size(10.0)
                        .strong()
                        .color(Color32::from_rgb(166, 174, 178)),
                );
            });
            ui.add_space(5.0);
            body(ui);
        });
    ui.add_space(7.0);
}

#[inline]
fn to_norm(v: f32, p: &EngineParam) -> f32 {
    let lo = p.lo as f32;
    let hi = p.hi as f32;
    if p.log && lo > 0.0 && hi > lo {
        ((v.clamp(lo, hi) / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0)
    } else {
        ((v - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0)
    }
}

#[inline]
fn from_norm(t: f32, p: &EngineParam) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let lo = p.lo as f32;
    let hi = p.hi as f32;
    if p.log && lo > 0.0 && hi > lo {
        lo * (hi / lo).powf(t)
    } else {
        lo + (hi - lo) * t
    }
}

/// Renders one tactile rotary knob cell (dial + label + monospace value + tiny reset trigger)
/// inside a fixed `cell_width`. Returns true if the parameter was hovered or interacted with.
pub(super) fn rotary_knob_cell(
    ui: &mut Ui,
    setter: &ParamSetter,
    p: &EngineParam,
    param: &FloatParam,
    label: &str,
    help: &str,
    cell_width: f32,
    active: bool,
    preset: &mut String,
) -> bool {
    let mut interacted = false;
    ui.allocate_ui_with_layout(
        vec2(cell_width, 76.0),
        Layout::top_down(Align::Center),
        |ui| {
            ui.set_max_width(cell_width);
            let dial_size = 38.0;
            let (rect, response) = ui.allocate_exact_size(vec2(dial_size, dial_size), Sense::drag());
            response.widget_info(|| WidgetInfo::slider(ui.is_enabled(), param.value() as f64, label));
            let response = response.on_hover_text(format!("{label}\n{help}\nDrag vertically (Shift for fine) · double-click to reset."));
            #[cfg(test)]
            trace_control(ui, "control", param, &response);

            if response.hovered() || response.dragged() || response.has_focus() {
                interacted = true;
            }
            if response.has_focus() {
                let inc = ui.input(|i| i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowUp));
                let dec = ui.input(|i| i.key_pressed(Key::ArrowLeft) || i.key_pressed(Key::ArrowDown));
                if inc || dec {
                    let step = if ui.input(|i| i.modifiers.shift) { 0.005 } else { 0.025 };
                    let cur = to_norm(param.value(), p);
                    let next = from_norm(cur + if inc { step } else { -step }, p);
                    set_number(setter, param, next);
                    *preset = "Custom settings".into();
                }
            }
            if response.drag_started() {
                setter.begin_set_parameter(param);
            }
            if response.dragged() {
                let fine = ui.input(|i| i.modifiers.shift);
                let sens = if fine { 720.0 } else { 170.0 };
                let delta = -response.drag_delta().y / sens;
                if delta.abs() > 0.0 {
                    let cur = to_norm(param.value(), p);
                    let next = from_norm(cur + delta, p);
                    setter.set_parameter(param, next);
                    *preset = "Custom settings".into();
                }
            }
            if response.drag_stopped() {
                setter.end_set_parameter(param);
            }
            if response.hovered() && ui.input(|i| i.pointer.button_double_clicked(PointerButton::Primary)) {
                set_number(setter, param, param.default_plain_value());
                *preset = "Custom settings".into();
                interacted = true;
            }

            // Paint tactile rotary dial
            let painter = ui.painter();
            let center = rect.center();
            let outer_r = dial_size * 0.48;
            let inner_r = dial_size * 0.34;
            let norm = to_norm(param.value(), p);
            let hovered = response.hovered() || response.dragged();

            // 270 deg sweep: start at 135 deg (bottom-left) clockwise to 405 deg (bottom-right) in Y-down screen coords
            let start_ang = std::f32::consts::FRAC_PI_4 * 3.0; // +135 deg (down-left)
            let sweep = std::f32::consts::PI * 1.5; // 270 deg clockwise
            let arc_pts = |t0: f32, t1: f32, r: f32, steps: usize| -> Vec<Pos2> {
                (0..=steps)
                    .map(|k| {
                        let a = start_ang + sweep * (t0 + (t1 - t0) * k as f32 / steps as f32);
                        pos2(center.x + r * a.cos(), center.y + r * a.sin())
                    })
                    .collect()
            };

            // Recessed background arc
            painter.add(Shape::line(arc_pts(0.0, 1.0, outer_r, 28), Stroke::new(2.4, TRACK_BG)));

            // Active amber arc (from center for bipolar params like Ceiling slope / Wall splay / Wall skew, else from 0)
            let bipolar = p.lo < 0.0 && p.hi > 0.0;
            let (a0, a1) = if bipolar {
                let z = to_norm(0.0, p);
                (z.min(norm), z.max(norm))
            } else {
                (0.0, norm)
            };
            if (a1 - a0).abs() > 0.004 {
                painter.add(Shape::line(
                    arc_pts(a0, a1, outer_r, 24),
                    Stroke::new(2.6, if hovered || active { AMBER } else { AMBER.gamma_multiply(0.86) }),
                ));
            }

            // Inner matte cap with subtle bevel ring
            painter.circle_filled(center, inner_r, if hovered { DIAL_HOVER } else { DIAL_BG });
            painter.circle_stroke(
                center,
                inner_r,
                Stroke::new(
                    1.0,
                    if hovered || active {
                        AMBER.gamma_multiply(0.75)
                    } else {
                        Color32::from_rgb(62, 70, 76)
                    },
                ),
            );

            // Indicator needle
            let needle_ang = start_ang + sweep * norm;
            let p_in = pos2(center.x + inner_r * 0.28 * needle_ang.cos(), center.y + inner_r * 0.28 * needle_ang.sin());
            let p_out = pos2(center.x + inner_r * 0.84 * needle_ang.cos(), center.y + inner_r * 0.84 * needle_ang.sin());
            painter.line_segment([p_in, p_out], Stroke::new(1.8, if hovered || active { AMBER } else { Color32::from_rgb(230, 228, 214) }));

            // Compact label + value + tiny factory reset
            ui.add_space(2.0);
            let short_lbl = short_label(label);
            ui.add(
                Label::new(
                    RichText::new(short_lbl)
                        .size(10.5)
                        .color(if hovered || active { Color32::from_rgb(236, 234, 222) } else { Color32::from_rgb(196, 200, 198) }),
                )
                .truncate(),
            )
            .on_hover_text(format!("{label}\n{help}"));

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(
                    RichText::new(guide::value(p, param.value() as f64))
                        .monospace()
                        .size(10.0)
                        .color(AMBER),
                );
                if default_reset(ui, setter, param, guide::value(p, param.default_plain_value() as f64)) {
                    *preset = "Custom settings".into();
                    interacted = true;
                }
            });
        },
    );
    interacted
}

fn short_label(label: &str) -> &str {
    match label {
        "Hammer felt hardness" => "Felt hardness",
        "Hammer shank stiffness" => "Shank stiffness",
        "Damper stopping power" => "Damper power",
        "Damper resting weight" => "Damper weight",
        "Tuning reference: A4" => "Pitch A4",
        "Wire bending stiffness" => "Wire stiffness",
        "Bridge downbearing" => "Downbearing",
        "Hammer strike angle" => "Strike angle",
        "Distance between ribs" => "Rib spacing",
        "Soundboard crown" => "Board crown",
        "Soundboard rim clamping" => "Rim clamping",
        "Piano left / right" => "Piano X",
        "Piano front / back" => "Piano Y",
        "Listener left / right" => "Listener X",
        "Listener front / back" => "Listener Y",
        _ => label,
    }
}
