//! Shared vector drawing helpers and live cutaway illustrations.
pub(super) mod action;
use super::*;
use nih_plug_egui::egui::epaint::CubicBezierShape;

pub(super) const INK: Color32 = Color32::from_rgb(182, 185, 173);
pub(super) const CASE: Color32 = Color32::from_rgb(58, 76, 82);
pub(super) const SIDE: Color32 = Color32::from_rgb(34, 53, 61);
pub(super) const WOOD: Color32 = Color32::from_rgb(170, 152, 118);
pub(super) const FRAME: Color32 = Color32::from_rgb(127, 139, 126);
pub(super) const KEY: Color32 = Color32::from_rgb(222, 219, 204);
pub(super) const EDGE: Color32 = Color32::from_rgb(214, 166, 92);

pub(super) fn cubic(points: &mut Vec<Pos2>, b: Pos2, c: Pos2, d: Pos2, tolerance: f32) {
    let a = *points.last().unwrap();
    let curve = CubicBezierShape::from_points_stroke(
        [a, b, c, d],
        false,
        Color32::TRANSPARENT,
        Stroke::NONE,
    );
    points.extend(curve.flatten(Some(tolerance)).into_iter().skip(1));
}
