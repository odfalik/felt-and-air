//! Unified interactive Room canvas: coupled Top-Down Floor Plan + Side Elevation.
//! Shows non-rectangular wall geometry, floor, flat/sloped ceiling, true side-profile
//! hinged lid (zero camera edge-on artifact), interactive piano & listener dragging (X/Y/Z),
//! and real 1st-order wall/floor/ceiling/lid reflection paths from `piano_engine::acoustics`.

use super::*;
use piano_engine::acoustics::room_preview_rays;
use piano_engine::design::{clamp_point_to_polygon, Vec2 as EngineVec2};

const WALL_SOFT: Color32 = Color32::from_rgb(138, 122, 98);
const RAY_WALL: Color32 = Color32::from_rgb(218, 168, 86);
const RAY_VERT: Color32 = Color32::from_rgb(112, 168, 206);
const LID_COLOR: Color32 = Color32::from_rgb(212, 174, 114);
const FLOOR_LINE: Color32 = Color32::from_rgb(118, 124, 126);

pub(super) fn paint(
    state: &mut State,
    ui: &mut Ui,
    setter: &ParamSetter,
    size: Vec2,
) {
    let mut o = state.params.options();
    let design = &state.preview_design;
    let room = design.room;
    let lid = design.lid;
    let active = PARAMS[state.active_param].name;

    let (r, response) = ui.allocate_exact_size(size, Sense::hover());
    #[cfg(test)]
    trace_left_target(ui, "room-panel", response.rect);
    response.on_hover_text(
        "Drag the piano or listener in the floor plan, or drag the listener vertically in the elevation to change height.",
    );

    let p = ui.painter_at(r);
    p.rect_filled(r, 8.0, PANEL);

    let footer_h = 24.0;
    let inner = Rect::from_min_max(r.min + vec2(14.0, 12.0), r.max - vec2(14.0, footer_h));
    let gap = 12.0;
    let plan_h = ((inner.height() - gap) * 0.61).max(110.0);
    let elev_h = (inner.height() - gap - plan_h).max(80.0);

    let plan_box = Rect::from_min_size(inner.min, vec2(inner.width(), plan_h));
    let elev_box = Rect::from_min_size(
        pos2(inner.left(), plan_box.bottom() + gap),
        vec2(inner.width(), elev_h),
    );

    // Subtle section backgrounds
    let sub_bg = Color32::from_rgb(27, 32, 35);
    p.rect_filled(plan_box, 6.0, sub_bg);
    p.rect_filled(elev_box, 6.0, sub_bg);

    let poly = room.floor_polygon();
    let max_z = room
        .ceiling_z_at(0.0)
        .max(room.ceiling_z_at(room.lx))
        .max(room.lz)
        .max(2.2);

    // Compute coupled horizontal scale so Floor Plan and Side Elevation align cleanly along X!
    let plan_pad = vec2(26.0, 22.0);
    let elev_pad = vec2(26.0, 22.0);
    let plan_avail = plan_box.shrink2(plan_pad);
    let elev_avail = elev_box.shrink2(elev_pad);

    let scale_x_plan = plan_avail.width() / room.lx.max(0.5) as f32;
    let scale_y_plan = plan_avail.height() / room.ly.max(0.5) as f32;
    let plan_scale = scale_x_plan.min(scale_y_plan).max(4.0);

    let plan_w = room.lx as f32 * plan_scale;
    let plan_rect_h = room.ly as f32 * plan_scale;
    let plan_origin_x = plan_avail.center().x - 0.5 * plan_w;
    let plan_bottom_y = plan_avail.center().y + 0.5 * plan_rect_h;

    let to_plan = |x: f64, y: f64| -> Pos2 {
        pos2(
            plan_origin_x + x as f32 * plan_scale,
            plan_bottom_y - y as f32 * plan_scale,
        )
    };

    // Elevation uses the exact same X span if vertical fits nicely, otherwise uniform fit
    let elev_scale_x = elev_avail.width() / room.lx.max(0.5) as f32;
    let elev_scale_z = elev_avail.height() / max_z as f32;
    let elev_scale = elev_scale_x.min(elev_scale_z).max(4.0);
    // Align X with floor plan when plan_scale <= elev_scale_x, else center cleanly
    let (elev_sx, elev_ox) = if plan_w <= elev_avail.width() + 1.0 && elev_avail.height() / max_z as f32 >= plan_scale * 0.45 {
        (plan_scale, plan_origin_x)
    } else {
        (elev_scale, elev_avail.center().x - 0.5 * room.lx as f32 * elev_scale)
    };
    let elev_sz = (elev_avail.height() / max_z as f32).min(elev_sx * 1.35).max(elev_scale);
    let elev_floor_y = elev_avail.bottom() - 2.0;
    let to_elev = |x: f64, z: f64| -> Pos2 {
        pos2(
            elev_ox + x as f32 * elev_sx,
            elev_floor_y - z as f32 * elev_sz,
        )
    };

    let source_room = [o.piano_x + 0.70, o.piano_y + 0.70, room.board_height];
    let listener_room = [o.listener_x, o.listener_y, o.ear_height_m];
    let rays = room_preview_rays(&room, &lid, source_room, listener_room);

    let wall_hl = matches!(
        active,
        "Room length"
            | "Room width"
            | "Room shape"
            | "Wall splay"
            | "Wall skew"
            | "Wall absorption"
            | "Room (walls, floor, ceiling, reverberation)"
    );
    let ceil_hl = matches!(active, "Room height" | "Ceiling slope" | "Room (walls, floor, ceiling, reverberation)");
    let lid_hl = active == "Lid angle";
    let ear_hl = matches!(active, "Ear height" | "Ear spacing" | "Listener position, along the keyboard" | "Listener position, along the strings");

    // ─────────────────────────── 1. FLOOR PLAN ───────────────────────────
    p.text(
        plan_box.left_top() + vec2(10.0, 6.0),
        Align2::LEFT_TOP,
        format!("FLOOR PLAN · {}", room.shape.name().to_uppercase()),
        FontId::proportional(10.0),
        MUTED,
    );

    let poly_pts: Vec<Pos2> = poly.iter().map(|v| to_plan(v.x, v.y)).collect();
    // Soft interior floor fill
    if poly_pts.len() >= 3 {
        // Fan/ear triangulation isn't safe for concave L-alcove with convex_polygon, so draw clean floor tint on bbox or sub-rects
        if room.shape == piano_engine::design::RoomShape::LAlcove {
            let r1 = Rect::from_two_pos(to_plan(0.0, 0.0), to_plan(room.lx, 0.62 * room.ly));
            let r2 = Rect::from_two_pos(to_plan(0.0, 0.62 * room.ly), to_plan(0.60 * room.lx, room.ly));
            let fill = if o.room_enabled { Color32::from_rgb(35, 42, 46) } else { Color32::from_rgb(30, 35, 38) };
            p.rect_filled(r1, 0.0, fill);
            p.rect_filled(r2, 0.0, fill);
        } else {
            let fill = if o.room_enabled { Color32::from_rgb(35, 42, 46) } else { Color32::from_rgb(30, 35, 38) };
            p.add(Shape::convex_polygon(poly_pts.clone(), fill, Stroke::NONE));
        }
        // Wall absorption acoustic-treatment cushion inside the walls
        if o.room_enabled {
            let absorb_alpha = (o.room_absorption as f32 / 0.7).clamp(0.08, 1.0);
            let wall_w = 1.5 + 5.5 * absorb_alpha;
            p.add(Shape::closed_line(
                poly_pts.clone(),
                Stroke::new(wall_w, WALL_SOFT.gamma_multiply(0.18 + 0.35 * absorb_alpha)),
            ));
        }
        let wall_stroke_col = if !o.room_enabled {
            Color32::from_gray(68)
        } else if wall_hl {
            AMBER
        } else {
            Color32::from_rgb(156, 162, 158)
        };
        p.add(Shape::closed_line(
            poly_pts,
            Stroke::new(if wall_hl { 2.0 } else { 1.4 }, wall_stroke_col),
        ));
    }

    let piano_p = to_plan(source_room[0], source_room[1]);
    let listener_p = to_plan(o.listener_x, o.listener_y);

    // 1st-order wall reflection rays from engine geometry
    if o.room_enabled {
        let refl_alpha = ((1.0 - o.room_absorption) as f32).clamp(0.16, 0.92);
        for hit in &rays.wall_hits {
            let hp = to_plan(hit[0], hit[1]);
            p.add(Shape::line(
                vec![piano_p, hp, listener_p],
                Stroke::new(1.15, RAY_WALL.gamma_multiply(0.45 * refl_alpha)),
            ));
            p.circle_filled(hp, 2.3, RAY_WALL.gamma_multiply(0.75 * refl_alpha));
        }
    }

    // Direct path
    p.line_segment(
        [piano_p, listener_p],
        Stroke::new(1.2, Color32::LIGHT_BLUE.gamma_multiply(0.65)),
    );

    // Grand piano footprint on floor plan
    let outline_pts: Vec<Pos2> = design
        .board
        .outline
        .iter()
        .map(|q| to_plan(o.piano_x + q.x, o.piano_y + q.y))
        .collect();
    if outline_pts.len() >= 3 {
        p.add(Shape::convex_polygon(
            outline_pts.clone(),
            AMBER.gamma_multiply(0.35),
            Stroke::new(1.3, AMBER),
        ));
        // Keyboard strip along the front edge (y = piano_y)
        let kb0 = to_plan(o.piano_x + 0.05, o.piano_y + 0.04);
        let kb1 = to_plan(o.piano_x + 1.35, o.piano_y + 0.04);
        p.line_segment([kb0, kb1], Stroke::new((2.5 * (plan_scale / 35.0)).clamp(2.0, 4.5), Color32::from_rgb(232, 228, 212)));
        // Lid projection footprint when open (foreshortened by cos(angle) from bass hinge x = piano_x)
        if lid.open {
            let cos_a = o.lid_angle_deg.to_radians().cos();
            let lid_pts: Vec<Pos2> = design
                .board
                .outline
                .iter()
                .map(|q| to_plan(o.piano_x + q.x * cos_a, o.piano_y + q.y))
                .collect();
            p.add(Shape::closed_line(
                lid_pts,
                Stroke::new(if lid_hl { 1.6 } else { 1.0 }, LID_COLOR.gamma_multiply(if lid_hl { 0.95 } else { 0.55 })),
            ));
        }
    }

    // Listener head & stereo ears on floor plan
    p.circle_stroke(
        listener_p,
        6.0,
        Stroke::new(if ear_hl { 1.9 } else { 1.3 }, if ear_hl { AMBER } else { Color32::LIGHT_BLUE }),
    );
    let ear_pts: Vec<Pos2> = design
        .ears
        .iter()
        .map(|ear| to_plan(o.piano_x + ear.x, o.piano_y + ear.y))
        .collect();
    if ear_pts.len() == 2 {
        p.line_segment([ear_pts[0], ear_pts[1]], Stroke::new(1.0, Color32::LIGHT_BLUE.gamma_multiply(0.5)));
    }
    for &ep in &ear_pts {
        p.circle_filled(ep, 2.8, if ear_hl { AMBER } else { Color32::LIGHT_BLUE });
    }

    p.text(
        piano_p + vec2(0.0, 14.0),
        Align2::CENTER_TOP,
        "Piano",
        FontId::proportional(10.0),
        AMBER,
    );
    p.text(
        listener_p + vec2(9.0, 0.0),
        Align2::LEFT_CENTER,
        "Listener",
        FontId::proportional(10.0),
        if ear_hl { AMBER } else { Color32::LIGHT_BLUE },
    );

    // Interactive Floor Plan dragging (piano & listener clamped to actual polygon!)
    let piano_hit = ui.interact(
        Rect::from_center_size(piano_p, vec2((1.6 * plan_scale).max(28.0), (1.7 * plan_scale).max(28.0))),
        ui.id().with("room-piano-drag"),
        Sense::drag(),
    );
    let listener_hit = ui.interact(
        Rect::from_center_size(listener_p, vec2(30.0, 30.0)),
        ui.id().with("room-listener-drag"),
        Sense::drag(),
    );
    let mut changed = false;
    for (is_listener, hit) in [(false, piano_hit), (true, listener_hit)] {
        if hit.dragged() {
            let delta = hit.drag_delta();
            let dx = delta.x as f64 / plan_scale as f64;
            let dy = -delta.y as f64 / plan_scale as f64;
            if is_listener {
                let raw = EngineVec2 { x: o.listener_x + dx, y: o.listener_y + dy };
                let clamped = clamp_point_to_polygon(&poly, raw, 0.30);
                o.listener_x = clamped.x.clamp(0.2, room.lx - 0.2);
                o.listener_y = clamped.y.clamp(0.2, room.ly - 0.2);
            } else {
                let raw_c = EngineVec2 { x: o.piano_x + 0.70 + dx, y: o.piano_y + 0.70 + dy };
                let clamped = clamp_point_to_polygon(&poly, raw_c, 0.85);
                o.piano_x = (clamped.x - 0.70).clamp(0.2, (room.lx - 1.5).max(0.2));
                o.piano_y = (clamped.y - 0.70).clamp(0.2, (room.ly - 1.6).max(0.2));
            }
            let field = match (is_listener, dx.abs() >= dy.abs()) {
                (true, true) => "Listener position, along the keyboard",
                (true, false) => "Listener position, along the strings",
                (false, true) => "Piano position, along the keyboard",
                _ => "Piano position, along the strings",
            };
            state.active_param = PARAMS.iter().position(|p| p.name == field).unwrap_or(state.active_param);
            changed = true;
        }
    }

    // Draggable Floor-Plan corner handles to sculpt Wall splay & Wall skew directly!
    if o.room_enabled && poly.len() >= 4 {
        let corner_col = if wall_hl { AMBER } else { Color32::from_rgb(166, 174, 170) };
        for (ci, &v) in poly.iter().enumerate() {
            let cp = to_plan(v.x, v.y);
            let diamond = vec![
                cp + vec2(0.0, -4.0),
                cp + vec2(4.0, 0.0),
                cp + vec2(0.0, 4.0),
                cp + vec2(-4.0, 0.0),
            ];
            p.add(Shape::convex_polygon(diamond, corner_col.gamma_multiply(0.85), Stroke::NONE));
            let hit = ui.interact(
                Rect::from_center_size(cp, vec2(18.0, 18.0)),
                ui.id().with(("room-corner-drag", ci)),
                Sense::drag(),
            );
            if hit.hovered() {
                p.circle_stroke(cp, 6.5, Stroke::new(1.2, AMBER));
            }
            if hit.dragged() {
                let d = hit.drag_delta();
                let dx_norm = d.x as f64 / (room.lx as f32 * plan_scale).max(40.0) as f64;
                let dy_norm = -d.y as f64 / (room.ly as f32 * plan_scale).max(40.0) as f64;
                let right_side = v.x > 0.5 * room.lx;
                let top_side = v.y > 0.5 * room.ly;
                let splay_sign = if right_side == top_side { 1.0 } else { -1.0 };
                let skew_sign = if top_side { 1.0 } else { -1.0 };
                o.wall_splay = (o.wall_splay + dy_norm * splay_sign * 1.4).clamp(-0.30, 0.30);
                o.wall_skew = (o.wall_skew + dx_norm * skew_sign * 1.4).clamp(-0.25, 0.25);
                let field = if dy_norm.abs() >= dx_norm.abs() { "Wall splay" } else { "Wall skew" };
                state.active_param = PARAMS.iter().position(|p| p.name == field).unwrap_or(state.active_param);
                changed = true;
            }
        }
    }

    // ─────────────────────────── 2. SIDE ELEVATION (FLOOR / CEILING / LID) ───────────────────────────
    p.text(
        elev_box.left_top() + vec2(10.0, 6.0),
        Align2::LEFT_TOP,
        "SIDE ELEVATION · FLOOR, CEILING & LID",
        FontId::proportional(10.0),
        MUTED,
    );

    let fl_l = to_elev(0.0, 0.0);
    let fl_r = to_elev(room.lx, 0.0);
    let cl_l = to_elev(0.0, room.ceiling_z_at(0.0));
    let cl_r = to_elev(room.lx, room.ceiling_z_at(room.lx));

    // Subtle interior elevation fill
    let elev_poly = vec![fl_l, fl_r, cl_r, cl_l];
    p.add(Shape::convex_polygon(
        elev_poly.clone(),
        if o.room_enabled { Color32::from_rgb(33, 40, 44) } else { Color32::from_rgb(29, 34, 37) },
        Stroke::NONE,
    ));

    // Floor & side walls
    let side_col = if o.room_enabled { Color32::from_rgb(138, 144, 142) } else { Color32::from_gray(64) };
    p.line_segment([fl_l, cl_l], Stroke::new(1.2, side_col));
    p.line_segment([fl_r, cl_r], Stroke::new(1.2, side_col));
    p.line_segment([fl_l, fl_r], Stroke::new(1.6, FLOOR_LINE));

    // Ceiling line (highlights cleanly when dragging Ceiling height or Ceiling slope!)
    let ceil_col = if !o.room_enabled {
        Color32::from_gray(68)
    } else if ceil_hl {
        AMBER
    } else {
        Color32::from_rgb(168, 174, 170)
    };
    p.line_segment([cl_l, cl_r], Stroke::new(if ceil_hl { 2.2 } else { 1.6 }, ceil_col));
    // Interactive ceiling handles: center drags ceiling height, right corner tilts slope!
    if o.room_enabled {
        let ceil_mid = pos2((cl_l.x + cl_r.x) * 0.5, (cl_l.y + cl_r.y) * 0.5);
        p.circle_filled(ceil_mid, 3.2, ceil_col);
        p.circle_filled(cl_r, 3.4, ceil_col);
        let h_hit = ui.interact(
            Rect::from_center_size(ceil_mid, vec2(44.0, 20.0)),
            ui.id().with("room-ceil-height-drag"),
            Sense::drag(),
        );
        if h_hit.hovered() {
            p.circle_stroke(ceil_mid, 6.0, Stroke::new(1.2, AMBER));
        }
        if h_hit.dragged() {
            let dz = -h_hit.drag_delta().y as f64 / elev_sz as f64;
            o.room_lz = (o.room_lz + dz).clamp(2.2, 15.0);
            state.active_param = PARAMS.iter().position(|p| p.name == "Room height").unwrap_or(state.active_param);
            changed = true;
        }
        let s_hit = ui.interact(
            Rect::from_center_size(cl_r, vec2(22.0, 22.0)),
            ui.id().with("room-ceil-slope-drag"),
            Sense::drag(),
        );
        if s_hit.hovered() {
            p.circle_stroke(cl_r, 6.5, Stroke::new(1.2, AMBER));
        }
        if s_hit.dragged() {
            let ddeg = -s_hit.drag_delta().y as f64 * 0.45;
            o.ceiling_slope_deg = (o.ceiling_slope_deg + ddeg).clamp(-15.0, 15.0);
            state.active_param = PARAMS.iter().position(|p| p.name == "Ceiling slope").unwrap_or(state.active_param);
            changed = true;
        }
    }

    // Height tick label on right wall
    p.text(
        pos2(cl_r.x + 5.0, (cl_r.y + fl_r.y) * 0.5),
        Align2::LEFT_CENTER,
        format!("{:.1} m", room.ceiling_z_at(room.lx)),
        FontId::proportional(9.5),
        if ceil_hl { AMBER } else { MUTED },
    );
    if room.ceiling_slope_deg.abs() >= 0.5 {
        p.text(
            pos2(cl_l.x - 4.0, (cl_l.y + fl_l.y) * 0.5),
            Align2::RIGHT_CENTER,
            format!("{:.1} m", room.ceiling_z_at(0.0)),
            FontId::proportional(9.5),
            if ceil_hl { AMBER } else { MUTED },
        );
    }

    // Elevation reflection rays: floor, ceiling, and lid!
    let piano_e = to_elev(source_room[0], source_room[2]);
    let listener_e = to_elev(o.listener_x, o.ear_height_m);

    if o.room_enabled {
        let refl_alpha = ((1.0 - o.room_absorption) as f32).clamp(0.18, 0.92);
        if let Some(fh) = rays.floor_hit {
            let hp = to_elev(fh[0], fh[2]);
            p.add(Shape::line(vec![piano_e, hp, listener_e], Stroke::new(1.1, RAY_VERT.gamma_multiply(0.55 * refl_alpha))));
            p.circle_filled(hp, 2.2, RAY_VERT.gamma_multiply(0.8 * refl_alpha));
        }
        if let Some(ch) = rays.ceiling_hit {
            let hp = to_elev(ch[0], ch[2]);
            p.add(Shape::line(vec![piano_e, hp, listener_e], Stroke::new(1.1, RAY_VERT.gamma_multiply(0.55 * refl_alpha))));
            p.circle_filled(hp, 2.2, RAY_VERT.gamma_multiply(0.8 * refl_alpha));
        }
    }
    if let Some(lh) = rays.lid_hit {
        let hp = to_elev(lh[0], lh[2]);
        p.add(Shape::line(
            vec![piano_e, hp, listener_e],
            Stroke::new(if lid_hl { 1.8 } else { 1.3 }, AMBER.gamma_multiply(if lid_hl { 0.95 } else { 0.75 })),
        ));
        p.circle_filled(hp, 2.6, AMBER);
    }

    // Direct sound path in elevation
    p.line_segment([piano_e, listener_e], Stroke::new(1.1, Color32::LIGHT_BLUE.gamma_multiply(0.55)));

    // Clean Grand Piano side profile (legs, case, soundboard line, true hinged lid + continuous prop stick!)
    let px0 = o.piano_x;
    let px1 = o.piano_x + 1.45;
    let case_bot = 0.68;
    let case_top = room.board_height + lid.hinge_height; // ~1.03 m
    // Legs
    for lx in [px0 + 0.14, px1 - 0.14] {
        p.line_segment(
            [to_elev(lx, 0.0), to_elev(lx, case_bot)],
            Stroke::new(2.2, Color32::from_rgb(122, 134, 138)),
        );
    }
    // Case body
    let case_rect = Rect::from_two_pos(to_elev(px0, case_top), to_elev(px1, case_bot));
    p.rect_filled(case_rect, 2.0, Color32::from_rgb(56, 70, 76));
    p.rect_stroke(case_rect, 2.0, Stroke::new(1.1, Color32::from_rgb(168, 172, 162)), StrokeKind::Inside);
    // Soundboard line inside case (z = 0.75 m)
    p.line_segment(
        [to_elev(px0 + 0.04, room.board_height), to_elev(px1 - 0.04, room.board_height)],
        Stroke::new(1.6, AMBER.gamma_multiply(0.75)),
    );
    // Hinged lid rising smoothly from bass hinge (px0, case_top) at exact lid_angle_deg — ZERO edge-on artifact!
    let th = o.lid_angle_deg.to_radians();
    let lid_len = 1.45;
    let lid_tip_x = px0 + lid_len * th.cos();
    let lid_tip_z = case_top + lid_len * th.sin();
    let hinge_pt = to_elev(px0, case_top);
    let tip_pt = to_elev(lid_tip_x, lid_tip_z);
    if o.lid_angle_deg > 1.0 {
        // Continuous prop stick resting cleanly from rim (px0 + 1.15, case_top) to lid underside
        let prop_attach = 0.78;
        let prop_x = px0 + lid_len * prop_attach * th.cos();
        let prop_z = case_top + lid_len * prop_attach * th.sin();
        p.line_segment(
            [to_elev(px0 + 1.12, case_top), to_elev(prop_x, prop_z)],
            Stroke::new(1.4, Color32::from_rgb(166, 152, 128)),
        );
    }
    p.line_segment(
        [hinge_pt, tip_pt],
        Stroke::new(if lid_hl { 3.2 } else { 2.4 }, if lid_hl { AMBER } else { LID_COLOR }),
    );
    p.circle_filled(hinge_pt, 2.2, Color32::from_rgb(220, 218, 204));
    // Draggable lid tip handle to swing lid directly 0..60 deg!
    p.circle_filled(tip_pt, 3.6, if lid_hl { AMBER } else { LID_COLOR });
    let lid_hit = ui.interact(
        Rect::from_center_size(tip_pt, vec2(24.0, 24.0)),
        ui.id().with("room-lid-drag"),
        Sense::drag(),
    );
    if lid_hit.hovered() {
        p.circle_stroke(tip_pt, 6.5, Stroke::new(1.2, AMBER));
    }
    if lid_hit.dragged() {
        let ddeg = -lid_hit.drag_delta().y as f64 * 0.85;
        o.lid_angle_deg = (o.lid_angle_deg + ddeg).clamp(0.0, 60.0);
        state.active_param = PARAMS.iter().position(|p| p.name == "Lid angle").unwrap_or(state.active_param);
        changed = true;
    }

    // Listener in side elevation (stand + head/ear dot, vertically draggable!)
    p.line_segment(
        [to_elev(o.listener_x, 0.0), listener_e],
        Stroke::new(1.0, Color32::LIGHT_BLUE.gamma_multiply(0.35)),
    );
    p.circle_filled(listener_e, 4.5, if ear_hl { AMBER } else { Color32::LIGHT_BLUE });
    p.text(
        listener_e + vec2(8.0, 0.0),
        Align2::LEFT_CENTER,
        format!("{:.2} m", o.ear_height_m),
        FontId::proportional(9.5),
        if ear_hl { AMBER } else { Color32::LIGHT_BLUE },
    );

    let elev_ear_hit = ui.interact(
        Rect::from_center_size(listener_e, vec2(32.0, 32.0)),
        ui.id().with("room-ear-height-drag"),
        Sense::drag(),
    );
    if elev_ear_hit.dragged() {
        let dz = -elev_ear_hit.drag_delta().y as f64 / elev_sz as f64;
        let ceiling_cap = (room.ceiling_z_at(o.listener_x) - 0.15).min(2.5);
        o.ear_height_m = (o.ear_height_m + dz).clamp(0.6, ceiling_cap.max(0.6));
        state.active_param = PARAMS.iter().position(|p| p.name == "Ear height").unwrap_or(state.active_param);
        changed = true;
    }

    if changed {
        state.apply_options(setter, &o);
        state.preset = "Custom settings".into();
    }

    // ─────────────────────────── FOOTER SUMMARY ───────────────────────────
    let slope_txt = if o.ceiling_slope_deg.abs() >= 0.5 {
        format!(" · Ceiling {:+.0}°", o.ceiling_slope_deg)
    } else {
        String::new()
    };
    let lid_txt = if lid.open {
        format!("Lid {:.0}°", o.lid_angle_deg)
    } else {
        "Lid closed".into()
    };
    p.text(
        pos2(r.left() + 14.0, r.bottom() - 6.0),
        Align2::LEFT_BOTTOM,
        format!(
            "{:.1} × {:.1} × {:.1} m · {}{} · {}",
            o.room_lx,
            o.room_ly,
            o.room_lz,
            room.shape.name(),
            slope_txt,
            lid_txt
        ),
        FontId::proportional(10.5),
        MUTED,
    );
    p.text(
        pos2(r.right() - 14.0, r.bottom() - 6.0),
        Align2::RIGHT_BOTTOM,
        "Drag piano, listener, corners, lid, or ceiling",
        FontId::proportional(10.0),
        Color32::from_gray(118),
    );
}
