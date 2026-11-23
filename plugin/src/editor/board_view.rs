//! Unified Soundboard visual: combines the live simulated displacement heatmap with
//! overlaid spruce ribs, rim clamping geometry, crown contour cues, and clearly labeled
//! active string-to-bridge drive pins in one cohesive canvas.

use super::*;
use piano_engine::design::point_in_polygon;

const RIB_WOOD: Color32 = Color32::from_rgb(174, 146, 102);
const BRIDGE_WOOD: Color32 = Color32::from_rgb(208, 198, 178);

/// Clip a 2D line segment (p0 -> p1) against a convex/simple polygon via dense sampling,
/// returning visible sub-segments inside the soundboard outline.
fn clip_line_to_board(poly: &[Point], p0: Point, p1: Point) -> Vec<(Point, Point)> {
    let steps = 48;
    let mut segs = Vec::new();
    let mut start: Option<Point> = None;
    let mut prev = p0;
    for k in 0..=steps {
        let t = k as f64 / steps as f64;
        let q = Point {
            x: p0.x + (p1.x - p0.x) * t,
            y: p0.y + (p1.y - p0.y) * t,
        };
        if point_in_polygon(poly, q) {
            if start.is_none() {
                start = Some(q);
            }
            prev = q;
        } else if let Some(s) = start.take() {
            segs.push((s, prev));
        }
    }
    if let Some(s) = start {
        segs.push((s, prev));
    }
    segs
}

pub(super) fn paint(state: &State, ui: &mut Ui, size: Vec2) {
    let o = state.params.options();
    let design = &state.preview_design;
    let active = PARAMS[state.active_param].name;

    let (r, response) = ui.allocate_exact_size(size, Sense::hover());
    #[cfg(test)]
    trace_left_target(ui, "board-panel", response.rect);
    response.on_hover_text(
        "Live soundboard vibration overlaid with spruce ribs underneath, rim clamping around the perimeter, and glowing bridge pins where active strings drive the wood.",
    );

    let p = ui.painter_at(r);
    p.rect_filled(r, 8.0, PANEL);

    let Some(v) = &state.view else {
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            "Building the instrument…",
            FontId::proportional(16.0),
            MUTED,
        );
        return;
    };

    let rib_hl = active == "Rib spacing" || active == "Rib height";
    let rim_hl = active == "Rim grip";
    let crown_hl = active == "Crown radius" || active == "Thickness";
    let bridge_hl = active == "Bridge height";

    // Fit the soundboard generously inside the single canvas
    let aspect = ((v.max.x - v.min.x) / (v.max.y - v.min.y)) as f32;
    let avail_h = (r.height() - 78.0).max(80.0);
    let avail_w = (r.width() - 180.0).max(80.0);
    let h = avail_h.min(avail_w / aspect);
    let center = pos2(r.center().x, r.top() + 30.0 + avail_h * 0.5);
    let br = Rect::from_center_size(center, vec2(h * aspect, h));

    let to = |q: Point| -> Pos2 {
        pos2(
            br.left() + (q.x - v.min.x) as f32 / (v.max.x - v.min.x) as f32 * br.width(),
            br.bottom() - (q.y - v.min.y) as f32 / (v.max.y - v.min.y) as f32 * br.height(),
        )
    };

    // 1. Live simulated displacement heatmap
    if let Some(t) = &state.texture {
        p.image(
            t.id(),
            br,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    // 2. Spruce ribs overlaid underneath across the grain (spacing & height respond live!)
    let grain = design.board.grain_angle;
    // Ribs run perpendicular to the spruce grain (dir = grain + 90 deg), stepped along grain
    let rib_dir = Point { x: -grain.sin(), y: grain.cos() };
    let step_dir = Point { x: grain.cos(), y: grain.sin() };
    let mid_board = Point {
        x: 0.5 * (v.min.x + v.max.x),
        y: 0.5 * (v.min.y + v.max.y),
    };
    let spacing = design.board.rib_spacing.clamp(0.06, 0.30);
    let rib_w = (1.2 + ((o.rib_height_mm - 10.0) / 26.0) as f32 * 2.4).clamp(1.0, 4.0);
    let rib_col = if rib_hl {
        AMBER.gamma_multiply(0.58)
    } else {
        RIB_WOOD.gamma_multiply(0.24)
    };
    let mut rib_count = 0usize;
    let span = 1.6;
    let max_k = (1.4 / spacing).ceil() as i32;
    for k in -max_k..=max_k {
        let off = k as f64 * spacing;
        let c = Point {
            x: mid_board.x + step_dir.x * off,
            y: mid_board.y + step_dir.y * off,
        };
        let p0 = Point { x: c.x - rib_dir.x * span, y: c.y - rib_dir.y * span };
        let p1 = Point { x: c.x + rib_dir.x * span, y: c.y + rib_dir.y * span };
        let segs = clip_line_to_board(&v.outline, p0, p1);
        if !segs.is_empty() {
            rib_count += 1;
        }
        for (a, b) in segs {
            p.line_segment([to(a), to(b)], Stroke::new(rib_w, rib_col));
        }
    }

    // 3. Subtle crown dome contour rings (tighter & brighter when board is more domed / crown_radius smaller)
    if o.crown_radius_m > 0.1 {
        let dome_strength = (14.0 / o.crown_radius_m).clamp(0.15, 1.4) as f32;
        let crown_col = if crown_hl {
            AMBER.gamma_multiply(0.55)
        } else {
            Color32::from_rgb(200, 196, 180).gamma_multiply((0.12 * dome_strength).clamp(0.04, 0.22))
        };
        let dome_c = Point { x: 0.56, y: 0.68 };
        for ring_i in 1..=3 {
            let rad = 0.13 * ring_i as f64 / dome_strength.sqrt().max(0.6) as f64;
            let pts: Vec<Pos2> = (0..=36)
                .filter_map(|k| {
                    let a = k as f64 / 36.0 * std::f64::consts::TAU;
                    let q = Point {
                        x: dome_c.x + rad * a.cos(),
                        y: dome_c.y + rad * 1.15 * a.sin(),
                    };
                    point_in_polygon(&v.outline, q).then(|| to(q))
                })
                .collect();
            if pts.len() >= 4 {
                p.add(Shape::line(pts, Stroke::new(if crown_hl { 1.3 } else { 0.8 }, crown_col)));
            }
        }
    }

    // 4. Outer soundboard rim perimeter — unmistakably visualizes `Soundboard rim clamping` (0..1)!
    let clamp = o.rim_clamp.clamp(0.0, 1.0) as f32;
    let mut outline_pts: Vec<Pos2> = v.outline.iter().map(|q| to(*q)).collect();
    if let Some(first) = outline_pts.first().copied() {
        outline_pts.push(first);
    }
    let rim_col = if rim_hl {
        AMBER
    } else {
        Color32::from_rgb(
            (115.0 + 68.0 * clamp) as u8,
            (118.0 + 58.0 * clamp) as u8,
            (114.0 + 38.0 * clamp) as u8,
        )
    };
    let rim_width = 1.2 + 2.4 * clamp;
    if clamp < 0.25 {
        // Nearly free / hinged rim: draw dashed outline so "free edge" reads immediately
        for seg in outline_pts.windows(2) {
            let d = seg[1] - seg[0];
            let len = d.length().max(1.0);
            let steps = (len / 10.0).ceil() as usize;
            for s in (0..steps).step_by(2) {
                let t0 = s as f32 / steps as f32;
                let t1 = ((s + 1) as f32 / steps as f32).min(1.0);
                p.line_segment(
                    [seg[0] + d * t0, seg[0] + d * t1],
                    Stroke::new(rim_width, rim_col),
                );
            }
        }
    } else {
        p.add(Shape::line(outline_pts.clone(), Stroke::new(rim_width, rim_col)));
    }
    // Clamping ticks around outer rim proportional to rim_clamp (or highlighted on hover!)
    if clamp >= 0.2 || rim_hl {
        let tick_len = 2.5 + 4.5 * clamp;
        let tick_alpha = if rim_hl { 0.95 } else { 0.22 + 0.45 * clamp };
        for seg in outline_pts.windows(2) {
            let d = seg[1] - seg[0];
            let len = d.length();
            if len < 8.0 {
                continue;
            }
            // Outward normal for screen-space Y-down CCW outline
            let out_n = vec2(d.y / len, -d.x / len);
            let count = ((len / 18.0).round() as usize).max(1);
            for k in 1..count {
                let pt = seg[0] + d * (k as f32 / count as f32);
                p.line_segment(
                    [pt, pt + out_n * tick_len],
                    Stroke::new(1.1, rim_col.gamma_multiply(tick_alpha)),
                );
            }
        }
    }

    // 5. Bass & Treble bridges + clearly styled active string drive pins
    let bridge_w = (2.0 + ((o.bridge_height_mm - 20.0) / 30.0) as f32 * 2.2).clamp(1.8, 4.4);
    for (index, bridge) in v.bridges.iter().enumerate() {
        p.add(Shape::line(
            bridge.iter().map(|q| to(*q)).collect(),
            Stroke::new(
                bridge_w,
                if bridge_hl { AMBER } else { BRIDGE_WOOD },
            ),
        ));
        if let Some(mid) = bridge.get(bridge.len() / 2) {
            let point = to(*mid);
            let x = if index == 0 {
                br.left() - 14.0
            } else {
                br.right() + 14.0
            };
            p.line_segment([point, pos2(x, point.y)], Stroke::new(0.7, MUTED));
            p.text(
                pos2(x, point.y),
                if index == 0 {
                    Align2::RIGHT_CENTER
                } else {
                    Align2::LEFT_CENTER
                },
                if index == 0 { "Bass bridge" } else { "Treble bridge" },
                FontId::proportional(11.0),
                if bridge_hl { AMBER } else { Color32::from_gray(200) },
            );
        }
    }

    // Active string-to-bridge drive pins (subtle resting pins + luminous rings when vibrating!)
    for (i, q) in v.string_bridge.iter().enumerate() {
        let pt = to(*q);
        let lv = state.snap.string_level.get(i).copied().unwrap_or(0.0);
        if lv > 2e-7 {
            let a = (0.35 + 0.14 * (lv / 2e-7).log10()).clamp(0.0, 1.0);
            p.circle_stroke(pt, 3.5 + 3.0 * a, Stroke::new(1.0, AMBER.gamma_multiply(0.45 * a)));
            p.circle_filled(pt, 2.8, AMBER.gamma_multiply(a));
        } else if i % 4 == 0 {
            p.circle_filled(pt, 1.1, Color32::from_gray(120).gamma_multiply(0.45));
        }
    }

    // Callout for rim clamping when hovered
    if rim_hl {
        let rim_pt = to(Point { x: 1.33, y: 0.62 });
        let lbl_x = br.right() + 14.0;
        p.line_segment([rim_pt, pos2(lbl_x, rim_pt.y)], Stroke::new(0.9, AMBER));
        p.text(
            pos2(lbl_x, rim_pt.y),
            Align2::LEFT_CENTER,
            format!("Rim ({:.0}% clamped)", clamp * 100.0),
            FontId::proportional(11.0),
            AMBER,
        );
    }

    // Top bar inside canvas
    p.text(
        r.left_top() + vec2(14.0, 10.0),
        Align2::LEFT_TOP,
        "Soundboard · live plate vibration + spruce ribs & rim",
        FontId::proportional(11.5),
        Color32::from_rgb(224, 224, 214),
    );
    p.text(
        pos2(r.right() - 14.0, r.top() + 10.0),
        Align2::RIGHT_TOP,
        format!("±{:.2} μm", state.board_scale * 1e6),
        FontId::monospace(10.5),
        MUTED,
    );

    // Keyboard edge label just under board outline
    p.text(
        pos2(br.center().x, br.bottom() + 6.0),
        Align2::CENTER_TOP,
        "Keyboard edge",
        FontId::proportional(9.5),
        MUTED,
    );

    // Bottom legend + compact structural spec readout (zero extra panel needed!)
    p.text(
        pos2(r.left() + 14.0, r.bottom() - 22.0),
        Align2::LEFT_BOTTOM,
        "Amber: up · Blue: down · Glowing bridge pins: vibrating strings driving the wood",
        FontId::proportional(10.0),
        AMBER.gamma_multiply(0.9),
    );
    p.text(
        pos2(r.left() + 14.0, r.bottom() - 7.0),
        Align2::LEFT_BOTTOM,
        format!(
            "{:.1} mm spruce · {} ribs ({:.0} mm tall @ {:.0} mm) · {:.0} m crown · rim {:.0}% clamped",
            o.board_thickness_mm,
            rib_count,
            o.rib_height_mm,
            o.rib_spacing_mm,
            o.crown_radius_m,
            clamp * 100.0,
        ),
        FontId::proportional(10.0),
        MUTED,
    );
}
