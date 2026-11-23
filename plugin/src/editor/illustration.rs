//! Authored fixed-view instrument illustration, not manufactured CAD or an engine snapshot.
//! The low-noise charcoal/ivory artwork has one rigid, parameter-driven lid hinge.
//! Curves are flattened with egui's existing Bezier implementation; filled surfaces use
//! verified star kernels or ruled strips, not an undefined concave convex_polygon fill.
pub(super) mod action;
use super::*;
use nih_plug_egui::egui::{
    emath::TSTransform,
    epaint::{CubicBezierShape, Mesh},
};

const INK: Color32 = Color32::from_rgb(182, 185, 173);
const CASE: Color32 = Color32::from_rgb(58, 76, 82);
const SIDE: Color32 = Color32::from_rgb(34, 53, 61);
const WOOD: Color32 = Color32::from_rgb(170, 169, 145);
const FRAME: Color32 = Color32::from_rgb(127, 139, 126);
const KEY: Color32 = Color32::from_rgb(216, 215, 200);
const BLACK: Color32 = Color32::from_rgb(26, 34, 38);
const LID: Color32 = Color32::from_rgb(101, 120, 121);
const EDGE: Color32 = Color32::from_rgb(186, 161, 120);
const HINGE: f32 = 0.965;
const KERNEL: Pos2 = Pos2::new(0.25, 0.5);
type P3 = [f32; 3];

fn project([x, y, z]: P3) -> Pos2 {
    pos2(
        145.0 + 225.0 * (0.95 * x + 0.78 * y),
        550.0 + 225.0 * (0.19 * x - 0.20 * y - 0.95 * z),
    )
}
fn source_rect() -> Rect {
    Rect::from_min_max(pos2(50.0, 60.0), pos2(850.0, 625.0))
}
fn lid_point(x: f32, y: f32, angle: f32, normal: f32) -> P3 {
    let (sin, cos) = angle.to_radians().sin_cos();
    [x * cos - normal * sin, y, HINGE + x * sin + normal * cos]
}
fn lid_screen(p: Pos2, angle: f32, normal: f32) -> Pos2 {
    project(lid_point(p.x * 0.99, 0.25 + p.y * 0.9, angle, normal))
}
fn map(points: &[Pos2], f: impl Fn(Pos2) -> Pos2) -> Vec<Pos2> {
    points.iter().copied().map(f).collect()
}
fn mean(points: &[Pos2]) -> Pos2 {
    (points.iter().fold(Vec2::ZERO, |s, p| s + p.to_vec2()) / points.len() as f32).to_pos2()
}
const WORLD_CURVE_TOLERANCE: f32 = 0.002;

fn cubic(points: &mut Vec<Pos2>, b: Pos2, c: Pos2, d: Pos2, tolerance: f32) {
    let a = *points.last().unwrap();
    let curve = CubicBezierShape::from_points_stroke(
        [a, b, c, d],
        false,
        Color32::TRANSPARENT,
        Stroke::NONE,
    );
    points.extend(curve.flatten(Some(tolerance)).into_iter().skip(1));
}
fn rim() -> (Vec<Pos2>, Vec<Pos2>) {
    let mut p = vec![pos2(0.0, 0.0), pos2(1.5, 0.0), pos2(1.5, 0.32)];
    cubic(&mut p, pos2(1.49, 0.54), pos2(1.30, 0.70), pos2(1.08, 0.87), WORLD_CURVE_TOLERANCE);
    cubic(&mut p, pos2(1.10, 1.04), pos2(1.00, 1.23), pos2(0.98, 1.48), WORLD_CURVE_TOLERANCE);
    cubic(&mut p, pos2(0.97, 1.82), pos2(0.86, 2.24), pos2(0.64, 2.48), WORLD_CURVE_TOLERANCE);
    cubic(&mut p, pos2(0.43, 2.70), pos2(0.20, 2.63), pos2(0.09, 2.36), WORLD_CURVE_TOLERANCE);
    let visible = p.clone();
    cubic(&mut p, pos2(0.01, 2.20), pos2(0.0, 2.05), pos2(0.0, 1.90), WORLD_CURVE_TOLERANCE);
    (p, visible)
}

fn signed_area(points: &[Pos2]) -> f64 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(a, b)| a.x as f64 * b.y as f64 - a.y as f64 * b.x as f64)
        .sum::<f64>()
        * 0.5
}
#[derive(Default)]
struct Drawing {
    shapes: Vec<Shape>,
}
impl Drawing {
    fn line(&mut self, points: Vec<Pos2>, color: Color32, width: f32) {
        self.shapes
            .push(Shape::line(points, Stroke::new(width, color)));
    }
    fn surface(&mut self, points: Vec<Pos2>, kernel: Pos2, fill: Color32, stroke: Stroke) {
        let n = points.len();
        let mut mesh = Mesh::default();
        mesh.colored_vertex(kernel, fill);
        for &p in &points {
            mesh.colored_vertex(p, fill);
        }
        for i in 0..n {
            mesh.add_triangle(0, (i + 1) as u32, ((i + 1) % n + 1) as u32);
        }
        // A lid exactly edge-on is a contour, not an ill-conditioned filled fan.
        if signed_area(&points).abs() > 0.1 {
            self.shapes.push(Shape::mesh(mesh));
        }
        if stroke.width > 0.0 {
            self.shapes.push(Shape::closed_line(points, stroke));
        }
    }
    fn polygon(&mut self, p: &[P3], fill: Color32, stroke: Stroke) {
        let points: Vec<_> = p.iter().copied().map(project).collect();
        let kernel = mean(&points);
        self.surface(points, kernel, fill, stroke);
    }
    fn ellipse(&mut self, p: P3, rx: f32, ry: f32, fill: Color32) {
        let c = project(p);
        let points: Vec<_> = (0..24)
            .map(|i| {
                let a = i as f32 / 24.0 * std::f32::consts::TAU;
                c + vec2(rx * a.cos(), ry * a.sin())
            })
            .collect();
        self.surface(points, c, fill, Stroke::NONE);
    }
    fn leg(&mut self, x: f32, y: f32) {
        let mut p = vec![pos2(x - 0.065, 0.70)];
        cubic(
            &mut p,
            pos2(x - 0.070, 0.51),
            pos2(x - 0.040, 0.26),
            pos2(x - 0.026, 0.075),
            WORLD_CURVE_TOLERANCE,
        );
        p.push(pos2(x + 0.028, 0.075));
        cubic(
            &mut p,
            pos2(x + 0.040, 0.25),
            pos2(x + 0.068, 0.51),
            pos2(x + 0.065, 0.70),
            WORLD_CURVE_TOLERANCE,
        );
        self.surface(
            map(&p, |p| project([p.x, y, p.y])),
            project([x, y, 0.38]),
            SIDE,
            Stroke::new(0.9, INK),
        );
        self.ellipse([x, y, 0.035], 6.0, 8.0, FRAME);
        self.ellipse(
            [x, y, 0.0],
            8.0,
            2.0,
            Color32::from_rgba_unmultiplied(182, 185, 173, 50),
        );
    }
    fn ribbon(&mut self, p: &[Pos2]) {
        let mut mesh = Mesh::default();
        for v in p {
            mesh.colored_vertex(project([v.x, v.y, 0.94]), SIDE);
            mesh.colored_vertex(project([v.x, v.y, 0.68]), SIDE);
        }
        for i in 0..p.len() - 1 {
            let a = (i * 2) as u32;
            mesh.add_triangle(a, a + 1, a + 2);
            mesh.add_triangle(a + 2, a + 1, a + 3);
        }
        self.shapes.push(Shape::mesh(mesh));
        let mut outline = map(p, |p| project([p.x, p.y, 0.94]));
        outline.extend(p.iter().rev().map(|p| project([p.x, p.y, 0.68])));
        self.shapes
            .push(Shape::closed_line(outline, Stroke::new(0.9, INK)));
    }
}

pub(super) struct GrandIllustration {
    back: Vec<Shape>,
    front: Vec<Shape>,
    lid: Vec<Shape>,
    rim: Vec<Pos2>,
    cached: Option<(u32, bool)>,
}
impl Default for GrandIllustration {
    fn default() -> Self {
        let (rim, visible) = rim();
        let mut back = Drawing::default();
        for (x, y) in [(0.43, 2.22), (0.12, 0.08), (1.38, 0.10)] {
            back.ellipse(
                [x, y, 0.0],
                18.0,
                4.0,
                Color32::from_rgba_unmultiplied(182, 185, 173, 20),
            );
        }
        back.leg(0.43, 2.22);
        // Lyre and pedals are recognition cues; there is no action/physics model here.
        for x in [0.62, 0.80] {
            let mut p = vec![pos2(x - 0.021, 0.65)];
            cubic(
                &mut p,
                pos2(x - 0.024, 0.43),
                pos2(x - 0.016, 0.32),
                pos2(x - 0.027, 0.14),
                WORLD_CURVE_TOLERANCE,
            );
            p.push(pos2(x + 0.027, 0.14));
            cubic(
                &mut p,
                pos2(x + 0.016, 0.34),
                pos2(x + 0.024, 0.46),
                pos2(x + 0.021, 0.65),
                WORLD_CURVE_TOLERANCE,
            );
            back.surface(
                map(&p, |p| project([p.x, 0.09, p.y])),
                project([x, 0.09, 0.38]),
                CASE,
                Stroke::new(0.9, INK),
            );
        }
        back.polygon(
            &[
                [0.57, 0.04, 0.13],
                [0.85, 0.04, 0.13],
                [0.85, 0.13, 0.13],
                [0.57, 0.13, 0.13],
            ],
            SIDE,
            Stroke::new(0.9, INK),
        );
        for x in [0.62, 0.71, 0.80] {
            back.polygon(
                &[
                    [x - 0.017, -0.05, 0.055],
                    [x + 0.017, -0.05, 0.055],
                    [x + 0.02, 0.11, 0.13],
                    [x - 0.02, 0.11, 0.13],
                ],
                FRAME,
                Stroke::new(0.5, INK),
            );
        }
        back.leg(0.12, 0.08);
        back.leg(1.38, 0.10);
        back.ribbon(&visible);
        back.surface(
            map(&rim, |p| project([p.x, p.y, 0.94])),
            project([KERNEL.x, KERNEL.y, 0.94]),
            CASE,
            Stroke::new(0.9, INK),
        );
        let inset = |p: Pos2| project([0.045 + p.x * 0.91, 0.075 + p.y * 0.94, 0.946]);
        back.surface(map(&rim, inset), inset(KERNEL), WOOD, Stroke::new(0.9, INK));
        // Sparse broad frame members, not repeated high-contrast string marks.
        for (a, b) in [
            ((0.18, 0.40), (0.28, 2.44)),
            ((0.64, 0.32), (0.70, 2.28)),
            ((1.14, 0.30), (0.94, 1.15)),
        ] {
            back.line(
                vec![project([a.0, a.1, 0.96]), project([b.0, b.1, 0.96])],
                FRAME,
                6.0,
            );
        }
        back.line(
            map(&visible, |p| project([p.x, p.y, 0.94])),
            Color32::from_rgba_unmultiplied(216, 215, 200, 165),
            2.0,
        );
        back.polygon(
            &[
                [-0.03, -0.32, 0.75],
                [1.53, -0.32, 0.75],
                [1.53, -0.32, 0.81],
                [-0.03, -0.32, 0.81],
            ],
            SIDE,
            Stroke::new(0.9, INK),
        );
        back.polygon(
            &[
                [-0.03, -0.32, 0.81],
                [1.53, -0.32, 0.81],
                [1.53, 0.0, 0.81],
                [-0.03, 0.0, 0.81],
            ],
            BLACK,
            Stroke::new(0.9, INK),
        );
        back.polygon(
            &[
                [0.0, -0.305, 0.835],
                [1.5, -0.305, 0.835],
                [1.5, -0.008, 0.835],
                [0.0, -0.008, 0.835],
            ],
            KEY,
            Stroke::NONE,
        );
        let width = 1.5 / 52.0;
        let mut white = 0;
        for midi in 21..=108 {
            if black(midi) {
                let x = white as f32 * width;
                back.polygon(
                    &[
                        [x - width * 0.30, -0.145, 0.862],
                        [x + width * 0.30, -0.145, 0.862],
                        [x + width * 0.30, -0.005, 0.862],
                        [x - width * 0.30, -0.005, 0.862],
                    ],
                    BLACK,
                    Stroke::NONE,
                );
            } else {
                white += 1;
            }
        }
        let mut front = Drawing::default();
        let mut desk = vec![pos2(0.29, 0.975), pos2(1.22, 0.975), pos2(1.18, 1.23)];
        cubic(
            &mut desk,
            pos2(1.14, 1.26),
            pos2(1.08, 1.265),
            pos2(1.02, 1.26),
            WORLD_CURVE_TOLERANCE,
        );
        desk.push(pos2(0.38, 1.26));
        cubic(
            &mut desk,
            pos2(0.33, 1.26),
            pos2(0.31, 1.245),
            pos2(0.30, 1.23),
            WORLD_CURVE_TOLERANCE,
        );
        let desk_screen = |p: Pos2| project([p.x, 0.055 + (p.y - 0.975) * 0.25, p.y]);
        front.surface(
            map(&desk, desk_screen),
            desk_screen(pos2(0.75, 1.10)),
            CASE,
            Stroke::new(0.9, INK),
        );
        front.line(
            vec![project([0.26, 0.04, 0.97]), project([1.25, 0.04, 0.97])],
            SIDE,
            5.0,
        );
        Self {
            back: back.shapes,
            front: front.shapes,
            lid: Vec::new(),
            rim,
            cached: None,
        }
    }
}
impl GrandIllustration {
    fn set_lid(&mut self, angle: f32, selected: bool) {
        let key = (angle.to_bits(), selected);
        if self.cached == Some(key) {
            return;
        }
        let mut drawing = Drawing::default();
        let (sin, cos) = angle.to_radians().sin_cos();
        let length = if angle < 20.0 { 0.24 } else { 0.80 };
        let base = 0.90;
        let gap = HINGE - 0.94;
        let b = -base * cos + gap * sin;
        let c = base * base + gap * gap - length * length;
        let disc = b * b - c;
        if angle > 5.0 && disc >= 0.0 {
            let x = -b + disc.sqrt();
            if x > 0.0 && x < 1.48 {
                drawing.line(
                    vec![
                        project([base, 0.72, 0.94]),
                        project(lid_point(x, 0.72, angle, 0.0)),
                    ],
                    BLACK,
                    4.0,
                );
            }
        }
        let upper = map(&self.rim, |p| lid_screen(p, angle, 0.025));
        drawing.surface(
            upper,
            lid_screen(KERNEL, angle, 0.025),
            Color32::from_rgb(51, 74, 83),
            Stroke::NONE,
        );
        let facing = -sin * 0.741 + cos * 0.3382;
        let fill = if facing < 0.0 {
            LID
        } else {
            Color32::from_rgb(51, 74, 83)
        };
        drawing.surface(
            map(&self.rim, |p| lid_screen(p, angle, 0.0)),
            lid_screen(KERNEL, angle, 0.0),
            fill,
            Stroke::new(
                1.0,
                if selected {
                    EDGE
                } else {
                    Color32::from_rgb(153, 153, 137)
                },
            ),
        );
        self.lid = drawing.shapes;
        self.cached = Some(key);
    }
    pub(super) fn paint(
        &mut self,
        ui: &mut Ui,
        height: f32,
        angle: f32,
        selected: bool,
    ) -> Response {
        self.set_lid(angle, selected);
        let (r, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        // Fixed source framing across lid states; never autofit to moving geometry.
        let source = source_rect();
        let scale =
            ((r.width() - 12.0) / source.width()).min((r.height() - 12.0) / source.height());
        let transform = TSTransform::new(
            r.center().to_vec2() - source.center().to_vec2() * scale,
            scale,
        );
        let painter = ui.painter_at(r);
        for shape in self.back.iter().chain(&self.lid).chain(&self.front) {
            let mut shape = shape.clone();
            shape.transform(transform);
            painter.add(shape);
        }
        response.on_hover_text(format!("Instrument illustration · lid {angle:.0}°. Authored geometry, not an engine snapshot or manufactured CAD. Room placement is edited in the floor plan."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn angles() -> Vec<f32> {
        let meta = PARAMS.iter().find(|p| p.name == "Lid angle").unwrap();
        let mut values: Vec<_> = (0..=120)
            .map(|i| (meta.lo + (meta.hi - meta.lo) * i as f64 / 120.0) as f32)
            .collect();
        let edge_on = (0.3382f32 / 0.741).atan().to_degrees();
        values.extend([edge_on - 0.01, edge_on, edge_on + 0.01]);
        values
    }
    fn cross(a: Vec2, b: Vec2) -> f32 {
        a.x * b.y - a.y * b.x
    }
    fn contains_or_edge(p: Pos2, contour: &[Pos2]) -> bool {
        let mut inside = false;
        for (&a, &b) in contour
            .iter()
            .zip(contour.iter().cycle().skip(1))
            .take(contour.len())
        {
            let ab = b - a;
            if cross(p - a, ab).abs() <= 0.05 * ab.length().max(1e-5)
                && p.x >= a.x.min(b.x) - 0.05
                && p.x <= a.x.max(b.x) + 0.05
                && p.y >= a.y.min(b.y) - 0.05
                && p.y <= a.y.max(b.y) + 0.05
            {
                return true;
            }
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                inside = !inside;
            }
        }
        inside
    }
    #[test]
    fn lid_is_a_rigid_rotation_with_an_invariant_hinge() {
        for angle in angles() {
            for y in [0.25, 1.0, 2.5] {
                assert_eq!(lid_point(0.0, y, angle, 0.0), [0.0, y, HINGE]);
            }
            for x in [0.25, 1.0, 1.4] {
                let p = lid_point(x, 0.7, angle, 0.0);
                assert!((p[0].hypot(p[2] - HINGE) - x).abs() < 1e-5);
            }
        }
    }
    #[test]
    fn full_parameter_range_and_edge_on_geometry_is_finite_framed_and_contained() {
        let mut art = GrandIllustration::default();
        for angle in angles() {
            art.set_lid(angle, true);
            for shape in art.back.iter().chain(&art.lid).chain(&art.front) {
                let bounds = shape.visual_bounding_rect();
                assert!(bounds.min.is_finite() && bounds.max.is_finite());
                assert!(
                    source_rect().expand(2.0).contains_rect(bounds),
                    "unframed artwork at {angle}: {bounds:?}"
                );
                if let Shape::Mesh(mesh) = shape {
                    assert!(mesh.vertices.iter().all(|v| v.pos.is_finite()));
                    assert!(mesh
                        .indices
                        .iter()
                        .all(|&i| (i as usize) < mesh.vertices.len()));
                    if !mesh.indices.chunks_exact(3).all(|t| t[0] == 0) {
                        // Ruled side ribbon: every adjacent pair is an exact
                        // parallelogram split into two triangles, not a global fan.
                        for pair in mesh.indices.chunks_exact(6) {
                            let quad = [
                                mesh.vertices[pair[0] as usize].pos,
                                mesh.vertices[pair[1] as usize].pos,
                                mesh.vertices[pair[5] as usize].pos,
                                mesh.vertices[pair[2] as usize].pos,
                            ];
                            for triangle in pair.chunks_exact(3) {
                                let a = mesh.vertices[triangle[0] as usize].pos;
                                let b = mesh.vertices[triangle[1] as usize].pos;
                                let c = mesh.vertices[triangle[2] as usize].pos;
                                assert!(
                                    contains_or_edge(a + ((b - a) + (c - a)) / 3.0, &quad),
                                    "ribbon triangle outside its quad"
                                );
                            }
                        }
                        continue;
                    }
                    let p: Vec<_> = mesh.vertices[1..].iter().map(|v| v.pos).collect();
                    let signed = signed_area(&p);
                    if signed.abs() > 1.0 {
                        let kernel = mesh.vertices[0].pos;
                        for (&a, &b) in p.iter().zip(p.iter().cycle().skip(1)).take(p.len()) {
                            assert!(
                                cross(b - a, kernel - a) * signed.signum() as f32 >= -0.1,
                                "invalid visibility kernel at {angle}"
                            );
                        }
                    }
                    let area = signed.abs();
                    let triangles = mesh
                        .indices
                        .chunks_exact(3)
                        .map(|t| {
                            let a = mesh.vertices[t[0] as usize].pos;
                            let b = mesh.vertices[t[1] as usize].pos;
                            let c = mesh.vertices[t[2] as usize].pos;
                            let ab = b - a;
                            let ac = c - a;
                            let triangle =
                                (ab.x as f64 * ac.y as f64 - ab.y as f64 * ac.x as f64).abs() * 0.5;
                            // Edge-on lid polygons legitimately collapse to a line. At all
                            // nondegenerate orientations, each triangle must remain in the contour.
                            if area > 1.0 && triangle > 0.001 {
                                assert!(
                                    contains_or_edge(a + ((b - a) + (c - a)) / 3.0, &p),
                                    "triangle outside contour at {angle}"
                                );
                            }
                            triangle
                        })
                        .sum::<f64>();
                    assert!(
                        (area - triangles).abs() < area * 0.001 + 0.1,
                        "fan overfill at {angle}: {area} vs {triangles}"
                    );
                }
            }
        }
    }
}
