//! Live rotational hammer cutaway + down-the-rail perspective bank of neighboring hammers,
//! driven by real measured engine `ActionTrace` and 30 Hz `Snapshot` telemetry (per-key hammer
//! and damper lifts, felt compression, shank flex, key depression, and live unison wire waves).
use super::*;
use crate::editor::strings_view::WIRE_COLORS;
use piano_engine::piano::{ActionPhase, ActionPose, ActionPoseRecord, ActionStroke, ActionTrace};
use piano_engine::{GrandOptions, Snapshot};

const CROWN: Pos2 = Pos2::new(408.0, 100.0);
const PIVOT: Pos2 = Pos2::new(660.0, 222.0);
const CURVE_TOLERANCE: f32 = 0.25;
const LOOKBEHIND_S: f64 = 0.060;
const STALE_S: f64 = 0.100;
const CONTACT_CUE_S: f64 = 0.090;
const VIEW_GAP_MIN_M: f64 = -0.008;
const VIEW_GAP_MAX_M: f64 = 0.064;
const NEIGHBOR_DEPTH: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq)]
struct MotionPose {
    head_m: f64,
    string_m: [f64; 3],
    phase: ActionPhase,
}
impl MotionPose {
    fn gap_m(&self) -> f64 {
        self.string_m[0] - self.head_m
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Provenance {
    lower_step: u64,
    upper_step: u64,
    alpha: f64,
    cursor_s: f64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct SampledPose {
    draw: MotionPose,
    source: Provenance,
    meta: ActionStroke,
}
#[derive(Clone, Copy)]
struct ContactCue {
    pose: MotionPose,
    pose_step: u64,
    solve_step: u64,
    _wire: usize,
    meta: ActionStroke,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum RenderState {
    Neutral,
    Buffering,
    AwaitingBracket,
    Unavailable(Option<SampledPose>),
    Live(SampledPose),
    Paused(SampledPose),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    None,
    Felt,
    Hammer,
    Shank,
    String,
    Gap,
    Damper,
    Key,
}
fn selected_part(name: &str) -> Part {
    match name {
        "Felt hardness" | "Felt damping" => Part::Felt,
        "Hammer mass" => Part::Hammer,
        "Shank springiness" => Part::Shank,
        "Strike point" | "Strike angle" => Part::String,
        "Let-off distance" => Part::Gap,
        "Damper strength" | "Damper weight" | "Pedal noise" => Part::Damper,
        "Action knocks" => Part::Key,
        _ => Part::None,
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[derive(Clone)]
pub(in crate::editor) struct PaintTrace {
    pub panel: Rect,
    pub clip: Rect,
    pub status: &'static str,
    pub identity: Option<(u64, i16, u64)>,
    pub bracket: Option<(u64, u64, f64, f64)>,
    pub cue_steps: Option<(u64, u64, u64)>,
    pub felt_color: Color32,
    pub reference_color: Color32,
    pub cue_color: Color32,
}

/// Rotate point `p` around `PIVOT` by `angle` radians (screen space, +Y down).
#[inline]
fn rotate_around_pivot(p: Pos2, angle: f32) -> Pos2 {
    let (s, c) = angle.sin_cos();
    let d = p - PIVOT;
    pos2(PIVOT.x + d.x * c - d.y * s, PIVOT.y + d.x * s + d.y * c)
}

/// Solve exact rigid-body pivot rotation `Δθ` around `PIVOT` so that `CROWN` shifts
/// vertically by `dy_down` (positive = downward away from string, in cutaway units).
#[inline]
fn pivot_angle_for_drop(dy_down: f32) -> f32 {
    let r0 = CROWN - PIVOT; // (-252, -122)
    let radius = r0.length();
    let theta0 = r0.y.atan2(r0.x); // ~ -2.69 rad (upper-left quadrant)
    let target_y = (r0.y + dy_down).clamp(-radius * 0.90, radius * 0.90);
    // Keep hammer on the left side of the pivot (cos < 0): theta = PI - asin(y/R) wrapped to (-PI, PI]
    let sin_val = (target_y / radius).clamp(-0.98, 0.98);
    let mut theta = std::f32::consts::PI - sin_val.asin();
    if theta > std::f32::consts::PI {
        theta -= std::f32::consts::TAU;
    }
    let mut delta = theta - theta0;
    while delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    }
    while delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    delta
}

pub(in crate::editor) struct ActionIllustration {
    felt: Vec<Pos2>,
    molding: Vec<Pos2>,
    seam: Vec<Pos2>,
    shank: Vec<Pos2>,
    generation: Option<u64>,
    last_step: u64,
    last_sequence: u64,
    anchor_engine_s: f64,
    anchor_ui_s: f64,
    progress_ui_s: f64,
    suspended: bool,
    initial_fill: bool,
    initial_step: u64,
    owner_unavailable: bool,
    frozen: Option<SampledPose>,
    wire_scale: f32,
}

impl Default for ActionIllustration {
    fn default() -> Self {
        let mut felt = vec![pos2(390.0, 100.0), pos2(424.0, 100.0)];
        cubic(&mut felt, pos2(440.0, 100.0), pos2(450.0, 109.0), pos2(451.0, 125.0), CURVE_TOLERANCE);
        cubic(&mut felt, pos2(449.0, 146.0), pos2(445.0, 175.0), pos2(435.0, 198.0), CURVE_TOLERANCE);
        cubic(&mut felt, pos2(424.0, 200.0), pos2(408.0, 199.0), pos2(400.0, 185.0), CURVE_TOLERANCE);
        cubic(&mut felt, pos2(388.0, 165.0), pos2(380.0, 143.0), pos2(380.0, 120.0), CURVE_TOLERANCE);
        cubic(&mut felt, pos2(380.0, 108.0), pos2(382.0, 100.0), pos2(390.0, 100.0), CURVE_TOLERANCE);
        felt.pop();

        let mut molding = vec![pos2(414.0, 124.0)];
        cubic(&mut molding, pos2(428.0, 123.0), pos2(440.0, 135.0), pos2(440.0, 151.0), CURVE_TOLERANCE);
        cubic(&mut molding, pos2(440.0, 166.0), pos2(437.0, 184.0), pos2(430.0, 190.0), CURVE_TOLERANCE);
        cubic(&mut molding, pos2(417.0, 190.0), pos2(407.0, 178.0), pos2(403.0, 165.0), CURVE_TOLERANCE);
        cubic(&mut molding, pos2(398.0, 150.0), pos2(401.0, 130.0), pos2(414.0, 124.0), CURVE_TOLERANCE);
        molding.pop();

        let mut seam = vec![pos2(385.0, 121.0)];
        cubic(&mut seam, pos2(385.0, 151.0), pos2(398.0, 179.0), pos2(411.0, 191.0), CURVE_TOLERANCE);

        let shank = vec![
            pos2(438.0, 164.0),
            pos2(660.0, 217.0),
            pos2(658.0, 227.0),
            pos2(440.0, 178.0),
        ];

        Self {
            felt,
            molding,
            seam,
            shank,
            generation: None,
            last_step: 0,
            last_sequence: 0,
            anchor_engine_s: 0.0,
            anchor_ui_s: 0.0,
            progress_ui_s: 0.0,
            suspended: true,
            initial_fill: false,
            initial_step: 0,
            owner_unavailable: false,
            frozen: None,
            wire_scale: 1e-4,
        }
    }
}

impl ActionIllustration {
    fn invalidate(&mut self) {
        self.generation = None;
        self.frozen = None;
        self.suspended = true;
        self.initial_fill = false;
        self.owner_unavailable = false;
    }

    pub(in crate::editor) fn receive(&mut self, generation: u64, trace: &ActionTrace, now: f64) {
        let first = trace.poses().next();
        if !trace.valid
            || !now.is_finite()
            || !trace.internal_dt_s.is_finite()
            || trace.internal_dt_s <= 0.0
            || trace.sample_stride_steps == 0
            || first.is_none()
            || trace.current_stroke().is_none()
        {
            self.invalidate();
            return;
        }
        let changed = self.generation != Some(generation);
        if changed {
            self.invalidate();
            self.generation = Some(generation);
            self.anchor_engine_s = trace.latest_step as f64 * trace.internal_dt_s - LOOKBEHIND_S;
            self.anchor_ui_s = now;
            self.progress_ui_s = now;
            self.initial_step = first.unwrap().pose.pose_step;
            self.initial_fill = true;
            self.suspended = false;
        } else if trace.latest_step < self.last_step || now < self.progress_ui_s {
            self.suspended = true;
            return;
        }
        let distance = first.unwrap().sequence.wrapping_sub(self.last_sequence);
        let gap = !changed && distance > 1 && distance < (1u64 << 63);
        let fresh = changed || trace.latest_step > self.last_step;
        if !changed && now - self.progress_ui_s > STALE_S {
            self.suspended = true;
        }
        if gap {
            self.suspended = true;
        } else if fresh && self.suspended {
            let candidate = trace.latest_step as f64 * trace.internal_dt_s - LOOKBEHIND_S;
            let forward = self.frozen.is_none_or(|shown| candidate >= shown.source.cursor_s);
            match Self::sample(trace, candidate) {
                Ok(Some(_)) if forward => {
                    self.anchor_engine_s = candidate;
                    self.anchor_ui_s = now;
                    self.suspended = false;
                    self.initial_fill = false;
                    self.owner_unavailable = false;
                }
                Err(_) => self.owner_unavailable = true,
                _ => {}
            }
        }
        if fresh {
            self.progress_ui_s = now;
        }
        self.last_step = trace.latest_step;
        self.last_sequence = trace.pose_sequence;
    }

    fn measured(meta: &ActionStroke, pose: &ActionPose) -> Option<MotionPose> {
        if !(0..128).contains(&meta.midi)
            || !(1..=3).contains(&meta.string_count)
            || !meta.hammer_mass_kg.is_finite()
            || meta.hammer_mass_kg <= 0.0
            || !meta.let_off_requested_m.is_finite()
            || !meta.check_distance_m.is_finite()
            || !meta.rest_distance_m.is_finite()
        {
            return None;
        }
        let head = match pose.phase {
            ActionPhase::Flight => pose.eta_m,
            ActionPhase::Returning | ActionPhase::Checked | ActionPhase::Rest => {
                let origin = &meta.return_origin;
                if !origin.valid
                    || !origin.reference_observed
                    || pose.pose_step < origin.transition_pose_step
                {
                    return None;
                }
                origin.origin_m - pose.hy_m
            }
            ActionPhase::Invalid => return None,
        };
        (head.is_finite()
            && pose
                .string_along_m
                .iter()
                .take(meta.string_count as usize)
                .all(|x| x.is_finite()))
        .then_some(MotionPose {
            head_m: head,
            string_m: pose.string_along_m,
            phase: pose.phase,
        })
    }

    fn exact(trace: &ActionTrace, record: &ActionPoseRecord, cursor: f64) -> Result<Option<SampledPose>, u64> {
        let meta = trace.stroke(record.stroke_sequence).ok_or(record.stroke_sequence)?;
        Ok(Self::measured(meta, &record.pose).map(|draw| SampledPose {
            draw,
            meta: *meta,
            source: Provenance {
                lower_step: record.pose.pose_step,
                upper_step: record.pose.pose_step,
                alpha: 0.0,
                cursor_s: cursor,
            },
        }))
    }

    fn sample(trace: &ActionTrace, cursor: f64) -> Result<Option<SampledPose>, u64> {
        let mut records = trace.poses().peekable();
        let mut before: Option<&ActionPoseRecord> = None;
        while let Some(first) = records.next() {
            let step = first.pose.pose_step;
            let time = step as f64 * trace.internal_dt_s;
            if !time.is_finite() {
                return Ok(None);
            }
            let mut last = first;
            let mut upper = before
                .filter(|old| old.stroke_sequence == first.stroke_sequence)
                .map(|_| first);
            while records.peek().is_some_and(|record| record.pose.pose_step == step) {
                let record = records.next().unwrap();
                if before.is_some_and(|old| old.stroke_sequence == record.stroke_sequence) {
                    upper = Some(record);
                }
                last = record;
            }
            if time == cursor {
                return Self::exact(trace, last, cursor);
            }
            if time < cursor {
                before = Some(last);
                continue;
            }
            let Some(lower) = before else {
                return Ok(None);
            };
            let lower_time = lower.pose.pose_step as f64 * trace.internal_dt_s;
            if lower_time == cursor {
                return Self::exact(trace, lower, cursor);
            }
            let meta = trace.stroke(lower.stroke_sequence).ok_or(lower.stroke_sequence)?;
            let Some(upper) = upper else {
                return Ok(None);
            };
            let a = Self::measured(meta, &lower.pose);
            let b = Self::measured(meta, &upper.pose);
            let (Some(a), Some(b)) = (a, b) else {
                return Ok(None);
            };
            let span = time - lower_time;
            if span <= 0.0 || span > 2.0 * trace.sample_stride_steps as f64 * trace.internal_dt_s + 1e-12 {
                return Ok(None);
            }
            let alpha = if a.phase == b.phase {
                (cursor - lower_time) / span
            } else {
                0.0
            };
            let draw = MotionPose {
                head_m: a.head_m + (b.head_m - a.head_m) * alpha,
                string_m: std::array::from_fn(|i| a.string_m[i] + (b.string_m[i] - a.string_m[i]) * alpha),
                phase: a.phase,
            };
            return Ok(Some(SampledPose {
                draw,
                meta: *meta,
                source: Provenance {
                    lower_step: lower.pose.pose_step,
                    upper_step: if alpha == 0.0 {
                        lower.pose.pose_step
                    } else {
                        upper.pose.pose_step
                    },
                    alpha,
                    cursor_s: cursor,
                },
            }));
        }
        Ok(None)
    }

    fn render_state(&mut self, trace: &ActionTrace, now: f64) -> RenderState {
        if self.generation.is_none() || !trace.valid {
            return RenderState::Neutral;
        }
        if !now.is_finite() || now < self.anchor_ui_s || now - self.progress_ui_s > STALE_S {
            self.suspended = true;
        }
        if !self.suspended {
            let cursor = self.anchor_engine_s + (now - self.anchor_ui_s);
            let first = trace.poses().next().unwrap().pose.pose_step;
            if self.initial_fill && cursor < self.initial_step as f64 * trace.internal_dt_s && first == self.initial_step {
                return RenderState::Buffering;
            }
            if self.frozen.is_some_and(|shown| cursor < shown.source.cursor_s) {
                self.suspended = true;
            } else {
                match Self::sample(trace, cursor) {
                    Ok(Some(sample)) => {
                        self.initial_fill = false;
                        self.owner_unavailable = false;
                        self.frozen = Some(sample);
                        return RenderState::Live(sample);
                    }
                    Err(_) => {
                        self.owner_unavailable = true;
                        self.suspended = true;
                    }
                    Ok(None) => {
                        self.owner_unavailable = false;
                        self.suspended = true;
                    }
                }
            }
        }
        if self.owner_unavailable {
            RenderState::Unavailable(self.frozen)
        } else {
            self.frozen.map_or(RenderState::AwaitingBracket, RenderState::Paused)
        }
    }

    fn recent_contact(trace: &ActionTrace, cursor: f64) -> Option<ContactCue> {
        let mut chosen: Option<ContactCue> = None;
        for contact in trace.contacts() {
            let Some(meta) = trace.stroke(contact.stroke_sequence) else {
                continue;
            };
            for (pose, solve_step) in [
                (&contact.entry_pose, contact.entry_solve_step),
                (&contact.peak_pose, contact.peak_solve_step),
            ] {
                let time = pose.pose_step as f64 * trace.internal_dt_s;
                if !pose.force_observed()
                    || pose.pose_step != solve_step.saturating_add(1)
                    || time > cursor
                    || cursor - time > CONTACT_CUE_S
                {
                    continue;
                }
                let Some(wire) = pose
                    .force_n
                    .iter()
                    .take(meta.string_count as usize)
                    .position(|f| f.is_finite() && *f > 0.0)
                else {
                    continue;
                };
                let Some(measured) = Self::measured(meta, pose) else {
                    continue;
                };
                if chosen.is_none_or(|old| pose.pose_step > old.pose_step) {
                    chosen = Some(ContactCue {
                        pose: measured,
                        pose_step: pose.pose_step,
                        solve_step,
                        _wire: wire,
                        meta: *meta,
                    });
                }
            }
        }
        chosen
    }

    /// Paint the unified rotational hammer cutaway + down-the-rail perspective bank.
    pub(in crate::editor) fn paint(
        &mut self,
        ui: &mut Ui,
        snap: &Snapshot,
        options: &GrandOptions,
        soft_pedal: bool,
        active: &str,
        size: Vec2,
    ) -> Response {
        let trace = &snap.action;
        let now = ui.ctx().input(|input| input.time);
        let state = self.render_state(trace, now);
        let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 8.0, PANEL);

        let part = selected_part(active);
        let moving = matches!(state, RenderState::Live(_));
        let shown = match state {
            RenderState::Live(s) | RenderState::Paused(s) => Some(s),
            RenderState::Unavailable(s) => s,
            RenderState::Neutral | RenderState::Buffering | RenderState::AwaitingBracket => None,
        };
        let pose = shown.map(|s| s.draw);

        // Local cutaway coordinates [160..760] x [24..360] mapped cleanly inside `rect`
        let scene_w = 600.0;
        let scene_h = 330.0;
        let scale = ((rect.width() - 32.0) / scene_w)
            .min((rect.height() - 44.0) / scene_h)
            .clamp(0.45, 2.2);
        let origin = pos2(
            rect.center().x - 460.0 * scale,
            rect.top() + 26.0 + (rect.height() - 44.0 - scene_h * scale) * 0.48,
        );
        // Una corda (soft pedal) physically shifts the hammer rail sideways so felt strikes 2 of 3 wires!
        let una_shift = if soft_pedal { vec2(7.0 * scale, 2.5 * scale) } else { Vec2::ZERO };
        let to_screen = |pt: Pos2| -> Pos2 { pos2(origin.x + pt.x * scale, origin.y + pt.y * scale) };
        let to_hammer_screen = |pt: Pos2| -> Pos2 { to_screen(pt) + una_shift };

        // Vertical displacement mapping: 0.047 m rest distance maps to ~96 cutaway units (~22 deg arc!)
        let cutaway_units_per_m = 104.0 / 0.050;
        let hero_midi: i32 = shown
            .map(|s| s.meta.midi as i32)
            .or_else(|| (snap.focus_midi >= 21).then_some(snap.focus_midi))
            .unwrap_or(60);

        // ─── 1. RECEDING PERSPECTIVE HAMMER & DAMPER BANK (back-to-front!) ───
        // Inspired by down-the-rail grand action perspective: recedes gently up-left into depth.
        // Driven live by REAL `snap.key_hammer_lift` and `snap.key_damper_lift` across neighbors!
        let dir: i32 = if hero_midi + NEIGHBOR_DEPTH as i32 <= 108 { 1 } else { -1 };
        let vanish = pos2(195.0, 82.0); // subtle perspective vanishing point along the action rail

        // Draw receding action rail beam connecting the back flanges to the hero flange
        let back_t = 0.935f32.powi(NEIGHBOR_DEPTH as i32);
        let persp_pt = |pt: Pos2, depth: usize| -> Pos2 {
            let f = 0.932f32.powi(depth as i32);
            let shifted = pos2(pt.x - depth as f32 * 5.2, pt.y - depth as f32 * 2.2);
            pos2(
                vanish.x + (shifted.x - vanish.x) * f,
                vanish.y + (shifted.y - vanish.y) * f,
            )
        };

        // Subtle wooden rail receding into the distance
        let rail_back_top = to_screen(persp_pt(pos2(648.0, 224.0), NEIGHBOR_DEPTH));
        let rail_back_bot = to_screen(persp_pt(pos2(648.0, 268.0), NEIGHBOR_DEPTH));
        let rail_front_top = to_screen(pos2(648.0, 224.0));
        let rail_front_bot = to_screen(pos2(648.0, 268.0));
        p.add(Shape::convex_polygon(
            vec![rail_back_top, rail_front_top, rail_front_bot, rail_back_bot],
            Color32::from_rgb(42, 50, 54).gamma_multiply(0.85),
            Stroke::new(0.8, Color32::from_rgb(76, 86, 88)),
        ));
        let _ = back_t;

        // Draw recessed neighbors k = NEIGHBOR_DEPTH down to 1
        for k in (1..=NEIGHBOR_DEPTH).rev() {
            let n_midi = (hero_midi + dir * k as i32).clamp(21, 108);
            let n_idx = (n_midi - 21) as usize;
            let h_lift = snap.key_hammer_lift[n_idx].clamp(0.0, 1.05);
            let d_lift = snap.key_damper_lift[n_idx].clamp(0.0, 1.0);
            // 0 lift = rest (94 units below string), 1 lift = at string (0 units)
            let drop_units = (1.0 - h_lift) * 94.0;
            let ang = pivot_angle_for_drop(drop_units);

            let fade = 0.84f32.powi(k as i32);
            let felt_col = Color32::from_rgb(
                (54.0 + 135.0 * fade) as u8,
                (60.0 + 130.0 * fade) as u8,
                (62.0 + 116.0 * fade) as u8,
            );
            let shank_col = Color32::from_rgb(
                (48.0 + 96.0 * fade) as u8,
                (52.0 + 82.0 * fade) as u8,
                (52.0 + 58.0 * fade) as u8,
            );
            let stroke_col = Color32::from_rgb(76, 84, 86).gamma_multiply(0.35 + 0.5 * fade);

            let xf = |pt: Pos2| -> Pos2 { to_hammer_screen(persp_pt(rotate_around_pivot(pt, ang), k)) };

            // Recessed shank
            let s0 = xf(pos2(440.0, 171.0));
            let s1 = xf(PIVOT);
            p.line_segment([s0, s1], Stroke::new((3.4 * scale * fade.sqrt()).max(1.0), shank_col));

            // Recessed felt silhouette (clean closed contour)
            let felt_pts: Vec<Pos2> = self.felt.iter().step_by(2).map(|&pt| xf(pt)).collect();
            if felt_pts.len() >= 3 {
                let mut mesh = epaint::Mesh::default();
                let center = xf(pos2(416.0, 148.0));
                mesh.colored_vertex(center, felt_col);
                for &pt in &felt_pts {
                    mesh.colored_vertex(pt, felt_col);
                }
                let n = felt_pts.len();
                for i in 0..n {
                    mesh.add_triangle(0, (i + 1) as u32, ((i + 1) % n + 1) as u32);
                }
                p.add(Shape::mesh(mesh));
                p.add(Shape::closed_line(felt_pts, Stroke::new(0.7, stroke_col)));
            }

            // Recessed damper pad for damped notes (A0..F#6 -> key 1..69 -> midi 21..89)
            if n_midi <= 89 {
                let dy_damp = -d_lift * 16.0;
                let d0 = to_screen(persp_pt(pos2(304.0, 72.0 + dy_damp), k));
                let d1 = to_screen(persp_pt(pos2(342.0, 96.0 + dy_damp), k));
                let dr = Rect::from_two_pos(d0, d1);
                p.rect_filled(dr, 2.0, Color32::from_rgb(66, 74, 78).gamma_multiply(0.45 + 0.45 * fade));
            }
        }

        // ─── 2. OVERHEAD REAL UNISON WIRES (Strings-tab visual language!) ───
        let wire_count = shown
            .map(|s| (s.meta.string_count as usize).clamp(1, 3))
            .unwrap_or(3);
        let peak_wire = snap
            .shape
            .iter()
            .zip(snap.shape_nodes)
            .flat_map(|(s, n)| s[..n.min(s.len())].iter())
            .fold(1e-7f32, |m, x| m.max(x.abs()));
        self.wire_scale = peak_wire.max(self.wire_scale * 0.97).max(2e-5);

        let wire_x0 = rect.left() + 18.0;
        let wire_x1 = rect.right() - 18.0;
        let strike_screen_x = to_screen(CROWN).x;
        let total_force: f32 = snap.hammer_force.iter().sum();

        for i in 0..wire_count {
            let base_y = to_screen(pos2(CROWN.x, CROWN.y - 3.0 - i as f32 * 5.0)).y;
            let skipped = soft_pedal && wire_count == 3 && i == 2;
            let col = if part == Part::String {
                EDGE
            } else if skipped {
                WIRE_COLORS[i.min(2)].gamma_multiply(0.32)
            } else {
                WIRE_COLORS[i.min(2)]
            };
            let n = snap.shape_nodes[i];
            if snap.focus_midi >= 21 && n >= 2 {
                // Render real simulated string wave spanning the cutaway!
                let amp = (8.5 * scale).min(14.0);
                let pts: Vec<Pos2> = (0..n)
                    .map(|j| {
                        let frac = j as f32 / (n - 1) as f32;
                        let disp = (snap.shape[i][j] / self.wire_scale).clamp(-1.0, 1.0) * amp;
                        pos2(wire_x0 + (wire_x1 - wire_x0) * frac, base_y - disp)
                    })
                    .collect();
                p.add(Shape::line(pts.clone(), Stroke::new(2.8, col.gamma_multiply(0.18))));
                p.add(Shape::line(pts, Stroke::new(1.4, col)));
            } else {
                p.line_segment([pos2(wire_x0, base_y), pos2(wire_x1, base_y)], Stroke::new(1.3, col.gamma_multiply(0.8)));
            }
        }

        // Subtle contact glow on wires during real hammer impact
        if total_force > 0.1 {
            let glow_r = (6.0 + (total_force / 4.0).min(18.0)) * scale;
            let c_pt = pos2(strike_screen_x, to_screen(CROWN).y - 4.0 * scale);
            p.circle_filled(c_pt, glow_r, AMBER.gamma_multiply(0.20));
            p.circle_filled(c_pt, glow_r * 0.42, AMBER.gamma_multiply(0.65));
        }

        // ─── 3. REAL DAMPER PAD (lifts smoothly with snap.damper_lift / half-pedaling!) ───
        let has_damper = if snap.focus_midi >= 21 {
            snap.damper_span[1] > snap.damper_span[0]
        } else {
            hero_midi <= 89
        };
        if has_damper {
            let d_lift = if snap.focus_midi >= 21 { snap.damper_lift } else { 0.0 };
            let lift_px = d_lift * 22.0;
            let damp_y_bot = CROWN.y - 3.0 - lift_px;
            let damp_y_top = damp_y_bot - 26.0;
            let damp_x0 = 286.0;
            let damp_x1 = 344.0;
            // Slim damper guide wire descending behind
            p.line_segment(
                [to_screen(pos2(315.0, damp_y_bot)), to_screen(pos2(315.0, 255.0))],
                Stroke::new(1.2, FRAME.gamma_multiply(0.65)),
            );
            // Wooden damper head + white felt underside wedge
            let wood_r = Rect::from_two_pos(
                to_screen(pos2(damp_x0, damp_y_top)),
                to_screen(pos2(damp_x1, damp_y_top + 11.0)),
            );
            let felt_r = Rect::from_two_pos(
                to_screen(pos2(damp_x0 + 3.0, damp_y_top + 11.0)),
                to_screen(pos2(damp_x1 - 3.0, damp_y_bot)),
            );
            let d_stroke = Stroke::new(
                if part == Part::Damper { 1.6 } else { 0.9 },
                if part == Part::Damper { EDGE } else { INK },
            );
            p.rect_filled(wood_r, 2.0, Color32::from_rgb(78, 62, 54));
            p.rect_stroke(wood_r, 2.0, d_stroke, StrokeKind::Inside);
            p.rect_filled(felt_r, 1.5, KEY.gamma_multiply(0.92));
            p.rect_stroke(felt_r, 1.5, d_stroke, StrokeKind::Inside);
            p.text(
                pos2(wood_r.center().x, wood_r.top() - 4.0),
                Align2::CENTER_BOTTOM,
                if d_lift > 0.75 { "Damper (up)" } else { "Damper" },
                FontId::proportional(10.0),
                if part == Part::Damper { EDGE } else { MUTED },
            );
        }

        // ─── 4. CLEAN KEYBED / ACTION RAIL BASE + LIVE KEY LEVER ───
        // Stationary flange bracket & vertical action rail on right
        let rail_r = Rect::from_two_pos(to_screen(pos2(646.0, 212.0)), to_screen(pos2(686.0, 286.0)));
        p.rect_filled(rail_r, 3.0, CASE);
        p.rect_stroke(rail_r, 3.0, Stroke::new(1.0, INK.gamma_multiply(0.7)), StrokeKind::Inside);
        // Key lever bar along bottom reacting to real `snap.key_depression`
        let key_dip = snap.key_depression * 10.0;
        let key_left = to_screen(pos2(236.0, 304.0 + key_dip));
        let key_fulcrum = to_screen(pos2(475.0, 304.0));
        let key_back = to_screen(pos2(666.0, 304.0 - key_dip * 0.75));
        p.add(Shape::line(
            vec![key_left, key_fulcrum, key_back],
            Stroke::new(
                (7.0 * scale).clamp(4.0, 9.0),
                if part == Part::Key { EDGE } else { Color32::from_rgb(132, 126, 112).gamma_multiply(0.65) },
            ),
        ));
        p.circle_filled(key_fulcrum, 3.2 * scale, FRAME);

        // ─── 5. HERO PIVOTING HAMMER, SHANK & REAL FELT COMPRESSION / SHANK FLEX ───
        // Compute vertical drop below string (0 at string, ~0.047 m at rest)
        let head_m = pose.map_or(-0.022, |pr| pr.head_m.clamp(-VIEW_GAP_MAX_M, -VIEW_GAP_MIN_M));
        let drop_units = (-head_m * cutaway_units_per_m) as f32;
        let angle = pivot_angle_for_drop(drop_units);

        // Subtle parameter-driven felt plumpness (mass & softness) + real Hertzian compression!
        let mass_scale = (options.hammer_mass as f32).sqrt().clamp(0.84, 1.20);
        let comp_squash = (snap.felt_compression_m * 1400.0).clamp(0.0, 0.22); // real indentation!
        let felt_pt = |pt: Pos2| -> Pos2 {
            // Scale around molding center for mass, and gently flatten top crown under real contact
            let c = pos2(416.0, 152.0);
            let mut d = (pt - c) * mass_scale;
            if d.y < 0.0 && comp_squash > 0.001 {
                d.y *= 1.0 - comp_squash;
                d.x *= 1.0 + comp_squash * 0.45;
            }
            to_hammer_screen(rotate_around_pivot(c + d, angle))
        };

        // Tapered wooden shank with real elastic flex from `snap.shank_flex_m`!
        let flex_px = (snap.shank_flex_m * 3200.0).clamp(-10.0, 10.0);
        let shank_start = rotate_around_pivot(pos2(440.0, 171.0), angle);
        let shank_mid = rotate_around_pivot(pos2(550.0, 196.0 + flex_px), angle);
        let shank_end = PIVOT;
        let shank_pts = vec![
            to_hammer_screen(rotate_around_pivot(self.shank[0], angle)),
            to_hammer_screen(rotate_around_pivot(pos2(548.0, 190.0 + flex_px), angle)),
            to_hammer_screen(rotate_around_pivot(self.shank[1], angle)),
            to_hammer_screen(rotate_around_pivot(self.shank[2], angle)),
            to_hammer_screen(rotate_around_pivot(pos2(548.0, 202.0 + flex_px), angle)),
            to_hammer_screen(rotate_around_pivot(self.shank[3], angle)),
        ];
        {
            let mut mesh = epaint::Mesh::default();
            mesh.colored_vertex(to_hammer_screen(shank_mid), WOOD);
            for &pt in &shank_pts {
                mesh.colored_vertex(pt, WOOD);
            }
            let n = shank_pts.len();
            for i in 0..n {
                mesh.add_triangle(0, (i + 1) as u32, ((i + 1) % n + 1) as u32);
            }
            p.add(Shape::mesh(mesh));
            p.add(Shape::closed_line(
                shank_pts,
                Stroke::new(if part == Part::Shank { 1.6 } else { 0.9 }, if part == Part::Shank { EDGE } else { INK }),
            ));
        }
        let _ = (shank_start, shank_end);

        // Hero Felt head
        let felt_screen: Vec<Pos2> = self.felt.iter().map(|&pt| felt_pt(pt)).collect();
        let felt_fill = if options.hammer_hardness > 1.5 {
            Color32::from_rgb(232, 230, 220)
        } else if options.hammer_hardness < 0.7 {
            Color32::from_rgb(218, 210, 192)
        } else {
            KEY
        };
        {
            let mut mesh = epaint::Mesh::default();
            mesh.colored_vertex(felt_pt(pos2(416.0, 150.0)), felt_fill);
            for &pt in &felt_screen {
                mesh.colored_vertex(pt, felt_fill);
            }
            let n = felt_screen.len();
            for i in 0..n {
                mesh.add_triangle(0, (i + 1) as u32, ((i + 1) % n + 1) as u32);
            }
            p.add(Shape::mesh(mesh));
            p.add(Shape::closed_line(
                felt_screen,
                Stroke::new(if part == Part::Felt { 1.8 } else { 1.1 }, if part == Part::Felt { EDGE } else { INK }),
            ));
        }

        // Wood molding core inside felt
        let mold_screen: Vec<Pos2> = self.molding.iter().map(|&pt| felt_pt(pt)).collect();
        {
            let mut mesh = epaint::Mesh::default();
            mesh.colored_vertex(felt_pt(pos2(420.0, 158.0)), WOOD);
            for &pt in &mold_screen {
                mesh.colored_vertex(pt, WOOD);
            }
            let n = mold_screen.len();
            for i in 0..n {
                mesh.add_triangle(0, (i + 1) as u32, ((i + 1) % n + 1) as u32);
            }
            p.add(Shape::mesh(mesh));
            p.add(Shape::closed_line(
                mold_screen,
                Stroke::new(if part == Part::Hammer { 1.6 } else { 1.1 }, if part == Part::Hammer { EDGE } else { FRAME }),
            ));
        }

        // Felt under-wrap seam & flange pivot pin
        p.add(Shape::line(self.seam.iter().map(|&pt| felt_pt(pt)).collect(), Stroke::new(0.9, FRAME)));
        let pivot_scr = to_hammer_screen(PIVOT);
        p.circle_filled(pivot_scr, (4.8 * scale).max(3.2), EDGE);
        p.circle_stroke(pivot_scr, (4.8 * scale).max(3.2), Stroke::new(1.0, SIDE));
        if soft_pedal {
            p.text(
                pos2(rect.right() - 14.0, rect.top() + 12.0),
                Align2::RIGHT_TOP,
                "Una corda · shifted",
                FontId::proportional(10.5),
                AMBER,
            );
        }

        // Subtle historical contact ghost outline during brief impact window
        let cue = if moving {
            Self::recent_contact(trace, shown.unwrap().source.cursor_s)
        } else {
            None
        };
        if let Some(cue) = cue.filter(|c| c.pose_step == c.solve_step.saturating_add(1)) {
            let cue_drop = (-cue.pose.head_m.clamp(-VIEW_GAP_MAX_M, -VIEW_GAP_MIN_M) * cutaway_units_per_m) as f32;
            let cue_ang = pivot_angle_for_drop(cue_drop);
            let ghost: Vec<Pos2> = self
                .felt
                .iter()
                .map(|&pt| to_screen(rotate_around_pivot(pt, cue_ang)))
                .collect();
            p.add(Shape::closed_line(ghost, Stroke::new(1.0, EDGE.gamma_multiply(0.55))));
        }

        // ─── 6. CALM, CLEAN INSTRUMENT HUD (no debug jargon clutter) ───
        let header_txt = if let Some(s) = shown {
            format!(
                "{} · {:.1} g hammer · let-off {:.1} mm",
                note_name(s.meta.midi as i32),
                s.meta.hammer_mass_kg * 1000.0,
                s.meta.let_off_effective_m() * 1000.0,
            )
        } else {
            "Action cutaway · play a note or chord to see hammers & dampers move".into()
        };
        p.text(
            pos2(rect.left() + 14.0, rect.bottom() - 8.0),
            Align2::LEFT_BOTTOM,
            header_txt,
            FontId::proportional(11.0),
            if part == Part::Gap { EDGE } else { MUTED },
        );

        if let Some(pr) = pose {
            let right_txt = if total_force > 0.1 {
                format!("Contact {:.1} N", total_force)
            } else {
                let phase_lbl = match pr.phase {
                    ActionPhase::Flight => "In flight",
                    ActionPhase::Returning => "Returning",
                    ActionPhase::Checked => "Backcheck",
                    ActionPhase::Rest => "At rest",
                    ActionPhase::Invalid => "",
                };
                format!("{phase_lbl} · Gap {:+.1} mm", pr.gap_m() * 1000.0)
            };
            p.text(
                pos2(rect.right() - 14.0, rect.bottom() - 8.0),
                Align2::RIGHT_BOTTOM,
                right_txt,
                FontId::monospace(10.5),
                if total_force > 0.1 { AMBER } else { INK },
            );
        }

        #[cfg(test)]
        {
            let status = match state {
                RenderState::Neutral => "Neutral",
                RenderState::Buffering => "Buffering",
                RenderState::AwaitingBracket => "AwaitingBracket",
                RenderState::Unavailable(_) => "Unavailable",
                RenderState::Paused(_) => "Paused",
                RenderState::Live(_) => "Live",
            };
            let snapshot = PaintTrace {
                panel: rect,
                clip: p.clip_rect(),
                status,
                identity: shown.map(|s| (self.generation.unwrap(), s.meta.midi, s.meta.stroke_sequence)),
                bracket: shown.map(|s| (s.source.lower_step, s.source.upper_step, s.source.alpha, s.source.cursor_s)),
                cue_steps: cue.map(|c| (c.meta.stroke_sequence, c.pose_step, c.solve_step)),
                felt_color: felt_fill,
                reference_color: if part == Part::String { EDGE } else { WIRE_COLORS[0] },
                cue_color: EDGE,
            };
            ui.ctx().data_mut(|data| data.insert_temp(Id::new("action:paint"), snapshot));
        }

        response.on_hover_text(
            "Rotational hammer cutaway and receding hammer bank driven live by the simulation: pivot arc, felt compression, shank flex, key depression, damper lift, and unison wires.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pivot_rotation_preserves_distance_to_flange_and_sweeps_monotonically() {
        let r0 = (CROWN - PIVOT).length();
        let mut prev_y = -1e9f32;
        for drop in [0.0f32, 20.0, 45.0, 70.0, 95.0] {
            let a = pivot_angle_for_drop(drop);
            let moved = rotate_around_pivot(CROWN, a);
            assert!(((moved - PIVOT).length() - r0).abs() < 1e-3);
            assert!((moved.y - (CROWN.y + drop)).abs() < 1e-2);
            assert!(moved.y > prev_y);
            prev_y = moved.y;
        }
    }
}
