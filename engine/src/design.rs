//! Physical description of the instrument. Everything here is in SI units and describes
//! geometry and materials, not numerics. See docs/MODEL.md for sources and caveats.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug)]
pub struct StringDesign {
    /// Target fundamental of this string, Hz (unison detune included).
    pub f0: f64,
    /// Speaking length, m.
    pub length: f64,
    /// Linear mass density, kg/m.
    pub rho_a: f64,
    /// Bending stiffness, N m^2.
    pub ei: f64,
    /// Axial stiffness of the load-bearing wire, N.
    pub ea: f64,
    /// Nominal tension, N (the solver retunes tension to hit f0).
    pub tension: f64,
    /// Frequency independent loss, 1/s.
    pub sigma0: f64,
    /// Frequency dependent loss, m^2/s.
    pub sigma1: f64,
    /// Where the string meets the bridge, piano coordinates.
    pub bridge: Vec2,
    /// false: rigid termination (used by validation tests).
    pub attached: bool,
}

#[derive(Clone, Debug)]
pub struct NoteDesign {
    pub midi: i32,
    pub strings: Vec<StringDesign>,
    /// Lengths of the same wires behind the bridge that are left free to ring (rear duplex).
    /// Never struck, never damped; the bridge alone sets them going.
    pub duplex: Vec<StringDesign>,
    /// Hammer position / speaking length.
    pub strike_ratio: f64,
    /// kg, whole hammer.
    pub hammer_mass: f64,
    /// Felt stiffness per string contact, N/m^p.
    pub hammer_k: f64,
    pub hammer_p: f64,
    /// Hunt-Crossley felt damping, s/m.
    pub hammer_mu: f64,
    /// The hammer head rides on a springy wooden shank. `shank_share` of the hammer's mass
    /// sits behind that spring; head and shank together swing at `shank_hz` when free.
    pub shank_share: f64,
    pub shank_hz: f64,
    pub shank_loss: f64,
    pub has_damper: bool,
    /// Strike direction away from the soundboard normal, rad.
    pub hammer_tilt: f64,
    pub two_polarisations: bool,
    /// 0 keeps the key's and the hammer's landings from reaching the board at all.
    pub key_thump: f64,
    pub key_thump_at: Vec2,
}

#[derive(Clone, Debug)]
pub struct BoardDesign {
    /// Outline in piano coordinates: X along the keyboard (bass at 0), Y toward the tail.
    pub outline: Vec<Vec2>,
    /// Direction of the grain in piano coordinates, rad.
    pub grain_angle: f64,
    pub thickness: f64,
    pub rho: f64,
    pub ex: f64,
    pub ey: f64,
    pub gxy: f64,
    pub nuxy: f64,
    /// Structural loss factor.
    pub loss_factor: f64,
    /// The board is glued up as a shallow dome (the crown). Pushing a dome flat stretches it,
    /// which resists normal motion like a spring bed of stiffness sqrt(Ex Ey) H / R^2 per unit
    /// area. That is the long-wavelength limit of shallow shell theory with the rim holding the
    /// edge; it stiffens the lowest modes and fades out above them. 0 means a flat board.
    pub crown_radius: f64,
    /// How firmly the rim holds the board's edge slope: 0 simply supported, 1 clamped.
    pub rim_clamp: f64,
    /// Broadband loss, 1/s.
    pub sigma0: f64,
    pub rib_spacing: f64,
    pub rib_width: f64,
    pub rib_height: f64,
    pub rib_e: f64,
    pub rib_rho: f64,
    pub ribs: bool,
    /// Bridges are beams glued along polylines (piano coordinates).
    pub bridges: Vec<Vec<Vec2>>,
    pub bridge_width: f64,
    pub bridge_height: f64,
    pub bridge_e: f64,
    pub bridge_rho: f64,
}

impl Default for BoardDesign {
    fn default() -> Self {
        BoardDesign {
            outline: Vec::new(),
            grain_angle: 0.0,
            thickness: 0.008,
            rho: 420.0,
            ex: 11.5e9,
            ey: 0.75e9,
            gxy: 0.70e9,
            nuxy: 0.37,
            loss_factor: 0.02,
            crown_radius: 18.0,
            rim_clamp: 1.0,
            sigma0: 2.0,
            rib_spacing: 0.125,
            rib_width: 0.024,
            rib_height: 0.022,
            rib_e: 11.0e9,
            rib_rho: 420.0,
            ribs: true,
            bridges: Vec::new(),
            bridge_width: 0.032,
            bridge_height: 0.034,
            bridge_e: 12.0e9,
            bridge_rho: 680.0,
        }
    }
}

/// Ear position in piano coordinates, z measured upward from the soundboard.
#[derive(Clone, Copy, Debug)]
pub struct Listener {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// Floor-plan shape of the room. Rectangular uses the exact shoebox image-source path;
/// non-rectangular shapes and sloped ceilings use generalized planar image sources.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum RoomShape {
    #[default]
    Rectangular = 0,
    /// Symmetric fan-shaped concert hall widening from stage toward listener.
    FanHall = 1,
    /// Acoustic studio with splayed non-parallel walls to break flutter echoes.
    AngledStudio = 2,
    /// Living / recital space with an L-shaped alcove.
    LAlcove = 3,
}

impl RoomShape {
    pub const ALL: [RoomShape; 4] = [
        RoomShape::Rectangular,
        RoomShape::FanHall,
        RoomShape::AngledStudio,
        RoomShape::LAlcove,
    ];

    pub fn from_value(v: f64) -> Self {
        match v.round() as i32 {
            1 => RoomShape::FanHall,
            2 => RoomShape::AngledStudio,
            3 => RoomShape::LAlcove,
            _ => RoomShape::Rectangular,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RoomShape::Rectangular => "Rectangular",
            RoomShape::FanHall => "Concert Hall (Fan)",
            RoomShape::AngledStudio => "Angled Studio",
            RoomShape::LAlcove => "L-Shaped Alcove",
        }
    }

    /// Counter-clockwise 2D floor polygon in room metres inside [0, lx] x [0, ly].
    pub fn floor_polygon(self, lx: f64, ly: f64) -> Vec<Vec2> {
        let pt = |x, y| Vec2 { x, y };
        match self {
            RoomShape::Rectangular => vec![pt(0.0, 0.0), pt(lx, 0.0), pt(lx, ly), pt(0.0, ly)],
            RoomShape::FanHall => vec![
                pt(0.0, 0.12 * ly),
                pt(lx, 0.0),
                pt(lx, ly),
                pt(0.0, 0.88 * ly),
            ],
            RoomShape::AngledStudio => vec![
                pt(0.0, 0.0),
                pt(lx, 0.07 * ly),
                pt(0.93 * lx, ly),
                pt(0.05 * lx, 0.94 * ly),
            ],
            RoomShape::LAlcove => vec![
                pt(0.0, 0.0),
                pt(lx, 0.0),
                pt(lx, 0.62 * ly),
                pt(0.60 * lx, 0.62 * ly),
                pt(0.60 * lx, ly),
                pt(0.0, ly),
            ],
        }
    }
}

/// Room geometry. The piano's coordinate origin (bass end of the keyboard side of the
/// board) sits at (piano_x, piano_y) on the floor plan, the board `board_height` above the floor.
#[derive(Clone, Copy, Debug)]
pub struct RoomDesign {
    pub enabled: bool,
    pub lx: f64,
    pub ly: f64,
    pub lz: f64,
    pub shape: RoomShape,
    /// Ceiling tilt along the room length (x), degrees. Positive rises toward +x (listener).
    pub ceiling_slope_deg: f64,
    /// Wall taper (-0.30..0.30): widens +x end vs -x end for custom fan/trapezoid sculpting.
    pub wall_splay: f64,
    /// Wall skew (-0.25..0.25): shears upper wall horizontally to break parallel-wall flutter.
    pub wall_skew: f64,
    /// Woodworth spherical head-shadow low-pass cue on contralateral ears (ideal for headphones).
    pub binaural_head: bool,
    pub piano_x: f64,
    pub piano_y: f64,
    pub board_height: f64,
    /// Mean absorption coefficient of all surfaces.
    pub absorption: f64,
}

impl RoomDesign {
    #[inline]
    pub fn is_plain_shoebox(&self) -> bool {
        self.shape == RoomShape::Rectangular
            && self.ceiling_slope_deg.abs() < 1e-9
            && self.wall_splay.abs() < 1e-9
            && self.wall_skew.abs() < 1e-9
    }

    /// Ceiling height at horizontal room coordinate `x`, clamped safely above floor.
    pub fn ceiling_z_at(&self, x: f64) -> f64 {
        let slope = self.ceiling_slope_deg.clamp(-15.0, 15.0).to_radians().tan();
        // Clamp slope so neither end of the room drops below 1.6 m.
        let max_slope = ((self.lz - 1.6).max(0.0)) / (0.5 * self.lx.max(0.1));
        let s = slope.clamp(-max_slope, max_slope);
        self.lz + (x - 0.5 * self.lx) * s
    }

    /// Effective clamped ceiling slope dz/dx.
    pub fn ceiling_slope_tan(&self) -> f64 {
        let slope = self.ceiling_slope_deg.clamp(-15.0, 15.0).to_radians().tan();
        let max_slope = ((self.lz - 1.6).max(0.0)) / (0.5 * self.lx.max(0.1));
        slope.clamp(-max_slope, max_slope)
    }

    pub fn floor_polygon(&self) -> Vec<Vec2> {
        let mut poly = self.shape.floor_polygon(self.lx, self.ly);
        let splay = self.wall_splay.clamp(-0.30, 0.30);
        let skew = self.wall_skew.clamp(-0.25, 0.25);
        if splay.abs() < 1e-9 && skew.abs() < 1e-9 {
            return poly;
        }
        let lx = self.lx.max(0.5);
        let ly = self.ly.max(0.5);
        let cy = 0.5 * ly;
        for v in &mut poly {
            let u = (v.x / lx).clamp(0.0, 1.0);
            let t = (v.y / ly).clamp(0.0, 1.0);
            // Splay scales y distance from center symmetrically along x (positive = wider at +x).
            let scale_y = (1.0 + splay * (2.0 * u - 1.0)).clamp(0.55, 1.30);
            v.y = (cy + (v.y - cy) * scale_y).clamp(0.0, ly);
            // Skew shifts upper vertices gently along x without crossing edges.
            v.x = (v.x + skew * lx * (t - 0.5) * 0.65).clamp(0.0, lx);
        }
        poly
    }
}

/// Point-in-polygon test (ray casting) for simple 2D polygons.
pub fn point_in_polygon(poly: &[Vec2], p: Vec2) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let a = poly[i];
        let b = poly[j];
        if ((a.y > p.y) != (b.y > p.y))
            && (p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Project `p` back inside `poly` with a margin if it falls outside or too close to a wall.
pub fn clamp_point_to_polygon(poly: &[Vec2], p: Vec2, margin: f64) -> Vec2 {
    if poly.len() < 3 {
        return p;
    }
    // Find nearest point on polygon boundary.
    let mut best_dist2 = f64::INFINITY;
    let mut best_pt = p;
    let mut best_inward = Vec2 { x: 0.0, y: 0.0 };
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let abx = b.x - a.x;
        let aby = b.y - a.y;
        let len2 = abx * abx + aby * aby;
        if len2 < 1e-12 {
            continue;
        }
        let t = (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0);
        let q = Vec2 { x: a.x + t * abx, y: a.y + t * aby };
        let d2 = (p.x - q.x).powi(2) + (p.y - q.y).powi(2);
        if d2 < best_dist2 {
            best_dist2 = d2;
            best_pt = q;
            let len = len2.sqrt();
            // Counter-clockwise polygon: inward normal is (-aby, abx) / len.
            best_inward = Vec2 { x: -aby / len, y: abx / len };
        }
    }
    let inside = point_in_polygon(poly, p);
    if inside && best_dist2 >= margin * margin {
        return p;
    }
    let candidate = Vec2 {
        x: best_pt.x + best_inward.x * margin,
        y: best_pt.y + best_inward.y * margin,
    };
    if point_in_polygon(poly, candidate) {
        return candidate;
    }
    // Fallback toward polygon centroid (always safe for all four room shapes).
    let cx = poly.iter().map(|v| v.x).sum::<f64>() / n as f64;
    let cy = poly.iter().map(|v| v.y).sum::<f64>() / n as f64;
    let mut lo = 0.0;
    let mut hi = 1.0;
    for _ in 0..12 {
        let mid = 0.5 * (lo + hi);
        let q = Vec2 { x: p.x + (cx - p.x) * mid, y: p.y + (cy - p.y) * mid };
        if point_in_polygon(poly, q) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let f = (hi + 0.04).min(1.0);
    Vec2 { x: p.x + (cx - p.x) * f, y: p.y + (cy - p.y) * f }
}

/// The grand's lid, hinged along the bass side.
#[derive(Clone, Copy, Debug)]
pub struct LidDesign {
    pub open: bool,
    pub angle_deg: f64,
    pub hinge_height: f64,
    pub width: f64,
    pub length: f64,
    pub reflection: f64,
}

/// The action, from the moment the hammer leaves the jack (let-off). Before that the key,
/// wippen and jack only accelerate the hammer, and their result, the hammer's speed, is what
/// a note-on carries.
#[derive(Clone, Copy, Debug)]
pub struct ActionDesign {
    /// Hammer to string at let-off, m.
    pub let_off: f64,
    /// Where the backcheck catches the returning hammer, below the string.
    pub check_distance: f64,
    /// Hammer at rest, below the string.
    pub rest_distance: f64,
    /// The hammer flies and falls under gravity.
    pub gravity: bool,
    /// Key front travel, m.
    pub key_travel: f64,
    /// Key position when the jack lets go.
    pub key_at_let_off: f64,
    /// Key position at which the damper meets the string.
    pub key_at_damper: f64,
    /// Key and wippen as felt at the key front, kg.
    pub key_mass: f64,
    /// Hammer speed over key speed.
    pub ratio: f64,
    /// Force returning the key, N.
    pub up_weight: f64,
    /// Action felts: stiffening spring K z^p plus compression-engaged drag C(z) dz/dt.
    /// C retains N s/m units at the assumed reference compression in action_contact.rs;
    /// damping vanishes at first touch instead of jumping to C*velocity. Key rail felts:
    pub felt_k: f64,
    pub felt_p: f64,
    pub felt_c: f64,
    /// Backcheck and hammer rest.
    pub stop_k: f64,
    pub stop_p: f64,
    pub stop_c: f64,
    /// Share of these stop forces that reaches the soundboard.
    pub case_share: f64,
    /// Felt hardness under the shifted (una corda) hammer, x normal.
    pub soft_pedal_hardness: f64,
}

impl Default for ActionDesign {
    fn default() -> Self {
        ActionDesign {
            let_off: 0.002,
            check_distance: 0.015,
            rest_distance: 0.047,
            gravity: true,
            key_travel: 0.010,
            key_at_let_off: 0.0092,
            key_at_damper: 0.0045,
            key_mass: 0.10,
            ratio: 5.5,
            up_weight: 0.25,
            felt_k: 4.5e5,
            felt_p: 1.5,
            felt_c: 50.0,
            stop_k: 6.3e4,
            stop_p: 1.5,
            stop_c: 3.0,
            case_share: 0.15,
            soft_pedal_hardness: 0.6,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PianoDesign {
    pub board: BoardDesign,
    /// Longitudinal string motion: stretching driven by the transverse wave (the source of the
    /// 'phantom' partials and the bass strings' ring). The tension change it makes at the bridge
    /// presses on the board through the downbearing angles, front plus back.
    pub longitudinal: bool,
    pub downbearing_deg: f64,
    /// The air pushes back on the moving board: it adds mass at low frequency and carries
    /// energy away as sound.
    pub air_loading: bool,
    /// Keep simulating strings that sit under their dampers, so that they answer faintly to the
    /// rest of the instrument as real damped strings do. Otherwise a settled damped string is
    /// replaced by a dashpot at the bridge, which is cheaper.
    pub damped_strings_ring: bool,
    /// Each damper rests on its string with this force (weight plus spring, per string). The
    /// string sits slightly deflected under it; lifting the damper lets it go, which is the
    /// faint stir of the strings when the pedal goes down.
    pub damper_press: f64,
    /// Knocks and friction of the pedal work, as forces on the case that reach the board.
    /// 1 is the reference guess; 0 turns them off.
    pub mechanism_noise: f64,
    pub action: ActionDesign,
    pub damper_strength: f64,
    pub notes: Vec<NoteDesign>,
    pub ears: [Listener; 2],
    pub room: RoomDesign,
    pub lid: LidDesign,
    pub air_rho: f64,
    pub air_c: f64,
}

/// Everything adjustable about the reference grand, in physical units where the quantity has
/// one, and as a multiple of the reference where it varies from note to note.
///
/// The layout is shared with C (include/piano_engine.h); keep the two in step.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrandOptions {
    /// Piano key numbers, A0 = 1, C4 = 40.
    pub first_key: i32,
    pub last_key: i32,

    // Strings and tuning.
    pub a4_hz: f64,
    /// Spread of the strings inside one note.
    pub unison_detune_cents: f64,
    /// x reference: the string's own damping.
    pub string_loss: f64,
    /// x reference: bending stiffness, sets the inharmonicity.
    pub string_stiffness: f64,
    /// Angle the strings make over the bridge, each side.
    pub downbearing_deg: f64,
    /// How far off the upright the hammer strikes; assumed.
    pub hammer_tilt_deg: f64,

    // Hammers and action.
    pub hammer_hardness: f64,
    pub hammer_mass: f64,
    pub felt_damping: f64,
    pub strike_position: f64,
    /// Scales keybed/return-rail and hammer backcheck/rest knocks into the case.
    pub key_thump: f64,
    pub shank_hz: f64,
    pub let_off_mm: f64,
    pub damper_strength: f64,
    pub damper_press_n: f64,
    /// Sustain-pedal endpoint knocks and moving friction, not returning-key sounds.
    pub mechanism_noise: f64,

    // Soundboard.
    pub board_thickness_mm: f64,
    pub board_loss_percent: f64,
    pub rib_height_mm: f64,
    pub rib_spacing_mm: f64,
    /// 0: flat board.
    pub crown_radius_m: f64,
    /// 0 simply supported, 1 clamped.
    pub rim_clamp: f64,
    pub bridge_height_mm: f64,

    // Space. Room coordinates in metres from one corner; the piano is placed by the bass end
    // of its keyboard side, keyboard along x, tail toward +y.
    pub room_lx: f64,
    pub room_ly: f64,
    pub room_lz: f64,
    pub room_absorption: f64,
    pub piano_x: f64,
    pub piano_y: f64,
    /// Centre of the head.
    pub listener_x: f64,
    pub listener_y: f64,
    /// Above the floor.
    pub ear_height_m: f64,
    /// A spaced pair; the head turns to face the piano.
    pub ear_spacing_m: f64,
    /// 0 closes the lid.
    pub lid_angle_deg: f64,
    /// Room floor-plan shape: 0 rectangular, 1 fan hall, 2 angled studio, 3 L-alcove.
    pub room_shape: f64,
    /// Ceiling tilt along room length, degrees (-15..15).
    pub ceiling_slope_deg: f64,
    /// Wall taper (-0.30..0.30) for sculpting arbitrary non-rectangular rooms.
    pub wall_splay: f64,
    /// Wall shear (-0.25..0.25) for angling opposing walls.
    pub wall_skew: f64,
    /// Woodworth spherical head-shadow low-pass cue on contralateral ear (headphones).
    pub binaural_head: bool,

    // Switches.
    /// Octaves tuned by matching partials, as on a real piano.
    pub stretch_tuning: bool,
    /// Stretching of the strings along their length.
    pub longitudinal: bool,
    /// Reaction of the air on the soundboard.
    pub air_loading: bool,
    /// Free string lengths behind the bridge in the treble.
    pub duplex: bool,
    /// Damped strings stay in the simulation.
    pub damped_strings_ring: bool,
    /// Sideways string motion of struck strings.
    pub two_polarisations: bool,
    pub room_enabled: bool,
}

impl Default for GrandOptions {
    fn default() -> Self {
        GrandOptions {
            first_key: 1,
            last_key: 88,
            a4_hz: 440.0,
            unison_detune_cents: 0.6,
            string_loss: 1.0,
            string_stiffness: 1.0,
            downbearing_deg: 1.5,
            hammer_tilt_deg: 4.0,
            hammer_hardness: 1.0,
            hammer_mass: 1.0,
            felt_damping: 1.0,
            strike_position: 1.0,
            key_thump: 1.0,
            shank_hz: 300.0,
            let_off_mm: 2.0,
            damper_strength: 1.0,
            damper_press_n: 0.12,
            mechanism_noise: 1.0,
            board_thickness_mm: 8.0,
            board_loss_percent: 2.0,
            rib_height_mm: 22.0,
            rib_spacing_mm: 125.0,
            crown_radius_m: 18.0,
            rim_clamp: 1.0,
            bridge_height_mm: 34.0,
            room_lx: 9.0,
            room_ly: 7.0,
            room_lz: 4.0,
            room_absorption: 0.18,
            piano_x: 3.0,
            piano_y: 2.2,
            listener_x: 6.4,
            listener_y: 2.9,
            ear_height_m: 1.2,
            ear_spacing_m: 0.5,
            lid_angle_deg: 38.0,
            room_shape: 0.0,
            ceiling_slope_deg: 0.0,
            wall_splay: 0.0,
            wall_skew: 0.0,
            binaural_head: false,
            stretch_tuning: true,
            longitudinal: true,
            air_loading: true,
            duplex: true,
            damped_strings_ring: false,
            two_polarisations: true,
            room_enabled: true,
        }
    }
}

pub fn key_to_midi(key: i32) -> i32 {
    key + 20
}

/// Positive MIDI NoteOn velocity to physical hammer launch speed, m/s.
/// MIDI velocity zero is NoteOff and must be decoded before calling this mapping.
pub fn midi_velocity_to_hammer_speed(velocity: i32) -> f64 {
    let midi = velocity.clamp(1, 127);
    if midi < 16 { return quiet_touch_launch(midi as f64); }
    // Preserve the original mapping and floating-point operation order for MIDI16..127.
    let v = midi as f64 / 127.0;
    0.25 + 5.5 * v.powf(1.7)
}

/// EXPERIMENT: access the grazing-launch regime, not a post-synthesis gain curve.
/// A cubic joins 0.200m/s with zero slope at MIDI1 to the old value AND slope at16.
/// The lower launch is a hypothesis, not a universal contact/audibility threshold.
fn quiet_touch_launch(velocity: f64) -> f64 {
    let upper = 0.25 + 5.5 * (16.0_f64 / 127.0).powf(1.7);
    let upper_slope = 5.5 * 1.7 / 127.0 * (16.0_f64 / 127.0).powf(0.7);
    let delta = upper - 0.200;
    let u = (velocity - 1.0) / 15.0;
    0.200 + u * u * (3.0 * delta - 15.0 * upper_slope
        + u * (15.0 * upper_slope - 2.0 * delta))
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// Piecewise linear through three anchor keys (C2, C4, C7), linear extrapolation outside.
fn anchors(key: i32, c2: f64, c4: f64, c7: f64) -> f64 {
    if key <= 40 {
        lerp(c2, c4, (key - 16) as f64 / 24.0)
    } else {
        lerp(c4, c7, (key - 40) as f64 / 36.0)
    }
}

/// Synthetic stable tuning texture, not measurements of a tuned instrument.
/// Cents sum to zero so detuning does not move the geometric tuning center.
fn unison_cents(key: i32, count: usize, center_hz: f64, amount: f64) -> [f64; 3] {
    if count == 1 || amount == 0.0 { return [0.0; 3]; }
    let texture = |salt: u32| {
        let mut x = key as u32 ^ salt;
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846ca68b);
        x ^= x >> 16;
        2.0 * x as f64 / u32::MAX as f64 - 1.0
    };
    let width = 1.0 + 0.15 * texture(0xa511e9b3);
    let skew = 0.12 * texture(0x63d83595);
    // Fixed 440 Hz reference, NOT the user's potentially retuned A4 frequency.
    let register = (440.0 / center_hz).powf(0.25).min(1.0);
    let a = amount * width * register;
    match count {
        2 => [-0.5 * a, 0.5 * a, 0.0],
        3 => {
            let low = a * (-1.0 + skew);
            let middle = a * (-2.0 * skew);
            [low, middle, -(low + middle)]
        }
        _ => unreachable!("grand scale uses one, two or three strings"),
    }
}

/// A generic 2 m class grand. Not a model of any specific instrument.
pub fn make_grand(o: &GrandOptions) -> PianoDesign {
    let mut board = BoardDesign {
        outline: [(0.0, 0.0), (1.40, 0.0), (1.40, 0.42), (1.33, 0.62), (1.20, 0.82), (1.02, 1.02), (0.82, 1.22), (0.62, 1.38), (0.45, 1.50), (0.0, 1.50)]
            .iter()
            .map(|&(x, y)| Vec2 { x, y })
            .collect(),
        grain_angle: 130.0 * PI / 180.0, // roughly parallel to the long bridge
        thickness: o.board_thickness_mm * 1e-3,
        loss_factor: o.board_loss_percent * 1e-2,
        rib_height: o.rib_height_mm * 1e-3,
        rib_spacing: o.rib_spacing_mm * 1e-3,
        crown_radius: o.crown_radius_m,
        rim_clamp: o.rim_clamp,
        bridge_height: o.bridge_height_mm * 1e-3,
        ..BoardDesign::default()
    };
    let room = RoomDesign {
        enabled: o.room_enabled,
        lx: o.room_lx,
        ly: o.room_ly,
        lz: o.room_lz,
        shape: RoomShape::from_value(o.room_shape),
        ceiling_slope_deg: o.ceiling_slope_deg,
        wall_splay: o.wall_splay,
        wall_skew: o.wall_skew,
        binaural_head: o.binaural_head,
        piano_x: o.piano_x,
        piano_y: o.piano_y,
        board_height: 0.75,
        absorption: o.room_absorption,
    };
    let lid = LidDesign { open: o.lid_angle_deg > 1.0, angle_deg: o.lid_angle_deg, hinge_height: 0.28, width: 1.45, length: 1.55, reflection: 0.9 };
    let action = ActionDesign { let_off: o.let_off_mm * 1e-3, case_share: 0.15 * o.key_thump, ..ActionDesign::default() };

    let steel_e = 2.0e11;
    let steel_rho = 7850.0;
    let agraffe_y = 0.03;
    let key_x = |key: i32| 0.09 + (key - 1) as f64 / 87.0 * 1.22;
    let plain_length = |key: i32| {
        let l = 0.052 * 1.858f64.powf((88 - key) as f64 / 12.0);
        let cap: f64 = 1.20; // case length limits the low tenor
        (l.powf(-6.0) + cap.powf(-6.0)).powf(-1.0 / 6.0)
    };
    let bridge_point = |key: i32| -> Vec2 {
        if key <= 26 {
            // overstrung bass, separate bridge near the tail
            let t = (key - 1) as f64 / 25.0;
            Vec2 { x: lerp(0.22, 0.50, t), y: lerp(1.40, 1.30, t) }
        } else {
            Vec2 { x: key_x(key), y: (agraffe_y + plain_length(key)).max(0.075) }
        }
    };
    // Bridges follow the string terminations of the full compass, whatever subset is built.
    board.bridges = vec![(1..=26).map(bridge_point).collect(), (27..=88).map(bridge_point).collect()];

    // The wire of one key, before tuning: length, mass and stiffness follow from the scale
    // design at the nominal pitch.
    let equal = |key: i32| o.a4_hz * 2.0f64.powf((key - 49) as f64 / 12.0);
    let wire = |key: i32| -> StringDesign {
        let f0 = equal(key);
        let (length, tension, rho_a, ei, ea);
        if key <= 26 {
            // copper wound on a steel core; only the core carries bending
            let t = (26 - key) as f64 / 25.0;
            length = lerp(1.12, 1.42, t);
            tension = lerp(750.0, 1000.0, t);
            rho_a = tension / (2.0 * length * f0).powi(2);
            let core = lerp(1.0e-3, 1.5e-3, t);
            ei = steel_e * PI * core.powi(4) / 64.0 * o.string_stiffness;
            ea = steel_e * PI * core * core / 4.0;
        } else {
            length = plain_length(key);
            tension = 720.0;
            rho_a = tension / (2.0 * length * f0).powi(2);
            let dia = (4.0 * rho_a / (PI * steel_rho)).sqrt();
            ei = steel_e * PI * dia.powi(4) / 64.0 * o.string_stiffness;
            ea = steel_e * PI * dia * dia / 4.0;
        }
        StringDesign { f0, length, rho_a, ei, ea, tension, sigma0: 0.0, sigma1: 0.0, bridge: Vec2::default(), attached: true }
    };

    // Tuning. Stiff strings have sharp partials, so octaves that sound in tune are wider than
    // 2:1. Tune as a tuner does: an equal tempered octave around A4, then octaves outward by
    // matching partials (4:2 in the middle, 2:1 in the high treble, 6:3 in the bass).
    let mut tuned = vec![0.0f64; 89];
    {
        let mut inharm = vec![0.0f64; 89];
        for key in 1..=88 {
            let s = wire(key);
            inharm[key as usize] = PI * PI * s.ei / (s.tension * s.length * s.length);
            tuned[key as usize] = equal(key);
        }
        if o.stretch_tuning {
            // Partial k of a string whose first partial is f1.
            let partial = |key: usize, k: i32, f1: f64| {
                let b = inharm[key];
                k as f64 * f1 * ((1.0 + b * (k * k) as f64) / (1.0 + b)).sqrt()
            };
            // First partial of `key` such that its partial a equals partial b of `reference`.
            let matched = |tuned: &[f64], key: usize, a: i32, reference: usize, b: i32| partial(reference, b, tuned[reference]) / partial(key, a, 1.0);
            tuned[37] = matched(&tuned, 37, 4, 49, 2);
            for key in 38..49 {
                tuned[key] = tuned[37] * (tuned[49] / tuned[37]).powf((key - 37) as f64 / 12.0);
            }
            for key in 50..=88usize {
                tuned[key] = if key <= 64 { matched(&tuned, key, 2, key - 12, 4) } else { matched(&tuned, key, 1, key - 12, 2) };
            }
            for key in (1..=36usize).rev() {
                tuned[key] = if key >= 28 { matched(&tuned, key, 4, key + 12, 2) } else { matched(&tuned, key, 6, key + 12, 3) };
            }
        }
    }

    let mut notes = Vec::new();
    for key in o.first_key.max(1)..=o.last_key.min(88) {
        let f0 = tuned[key as usize];
        let count = if key <= 10 { 1 } else if key <= 26 { 2 } else { 3 };
        let mut s = wire(key);
        s.tension *= (f0 / equal(key)).powi(2); // the tension that gives the tuned pitch
        // Loss law of Chaigne & Askenfelt (1994): rate = b1 + b3 * omega^2.
        let b3 = if key <= 40 { 6.25e-9 } else { 6.25e-9 * (1.0e-9f64 / 6.25e-9).powf((key - 40) as f64 / 48.0) };
        // Their constant term (0.5 /s) was fitted with a rigid bridge, so it contains the energy
        // the soundboard takes. Here the board takes that energy itself, and only the string's
        // own loss (air and internal friction) belongs in the constant. 0.12 /s is an estimate.
        s.sigma0 = 0.12 * o.string_loss;
        s.sigma1 = b3 * s.tension / s.rho_a * o.string_loss;

        let bp = bridge_point(key);
        let mut strings = Vec::new();
        let mut duplex = Vec::new();
        let cents = unison_cents(key, count, f0, o.unison_detune_cents);
        for i in 0..count {
            let mut si = s.clone();
            let offset = if count == 1 { 0.0 } else { i as f64 - (count - 1) as f64 / 2.0 };
            si.f0 = f0 * 2.0f64.powf(cents[i] / 1200.0);
            si.bridge = Vec2 { x: bp.x + 0.006 * offset, y: bp.y };
            if o.duplex && key >= 52 {
                // The same wire at the same tension, a third (or, in the top octaves, half) as
                // long: it sounds a twelfth or an octave above the speaking length.
                let mut du = si.clone();
                let ratio: f64 = if key < 65 { 3.0 } else { 2.0 };
                let bs = PI * PI * si.ei / (si.tension * si.length * si.length);
                du.length = si.length / ratio;
                du.f0 = si.f0 * ratio * ((1.0 + bs * ratio * ratio) / (1.0 + bs)).sqrt();
                duplex.push(du);
            }
            strings.push(si);
        }
        // Chaigne & Askenfelt give per-string equivalent hammers at C2, C4, C7.
        let per_string_mass = anchors(key, 4.9e-3, 2.97e-3, 2.2e-3).clamp(2.0e-3, 5.5e-3);
        notes.push(NoteDesign {
            midi: key_to_midi(key),
            strings,
            duplex,
            strike_ratio: (if key <= 50 { 0.12 } else { lerp(0.12, 0.075, (key - 50) as f64 / 38.0) }) * o.strike_position,
            hammer_mass: per_string_mass * count as f64 * o.hammer_mass,
            hammer_k: anchors(key, 4.0e8f64.ln(), 4.5e9f64.ln(), 1.0e11f64.ln()).exp() * o.hammer_hardness,
            hammer_p: anchors(key, 2.3, 2.5, 3.0).clamp(2.2, 3.1),
            hammer_mu: 0.1 * o.felt_damping,
            shank_share: 0.2,
            shank_hz: o.shank_hz,
            shank_loss: 0.05,
            has_damper: key <= 69,
            hammer_tilt: o.hammer_tilt_deg * PI / 180.0,
            two_polarisations: o.two_polarisations,
            key_thump: o.key_thump,
            key_thump_at: Vec2 { x: key_x(key), y: 0.06 },
        });
    }

    // Ears: a spaced pair centred on the listener, turned to face the middle of the board.
    let ears = {
        let hx = o.listener_x - o.piano_x;
        let hy = o.listener_y - o.piano_y; // head, piano coordinates
        let (mut fx, mut fy) = (0.70 - hx, 0.60 - hy);
        let mut len = fx.hypot(fy);
        if len < 1e-6 {
            fx = 0.0;
            fy = 1.0;
            len = 1.0;
        }
        fx /= len;
        fy /= len;
        let z = o.ear_height_m - room.board_height;
        let half = 0.5 * o.ear_spacing_m;
        // The left ear is to the left of the facing direction.
        [Listener { x: hx - fy * half, y: hy + fx * half, z }, Listener { x: hx + fy * half, y: hy - fx * half, z }]
    };

    PianoDesign {
        board,
        longitudinal: o.longitudinal,
        downbearing_deg: o.downbearing_deg,
        air_loading: o.air_loading,
        damped_strings_ring: o.damped_strings_ring,
        damper_press: o.damper_press_n,
        mechanism_noise: o.mechanism_noise,
        action,
        damper_strength: o.damper_strength,
        notes,
        ears,
        room,
        lid,
        air_rho: 1.2,
        air_c: 343.0,
    }
}


#[cfg(test)]
mod quiet_touch_tests {
    use super::*;
    fn legacy(velocity: f64) -> f64 { 0.25 + 5.5 * (velocity / 127.0).powf(1.7) }

    #[test]
    fn quiet_touch_preserves_every_upper_velocity_bit_and_stays_monotone() {
        for velocity in 16..=127 {
            assert_eq!(midi_velocity_to_hammer_speed(velocity).to_bits(), legacy(velocity as f64).to_bits());
        }
        assert_eq!(midi_velocity_to_hammer_speed(1).to_bits(), 0.200_f64.to_bits());
        assert_eq!(midi_velocity_to_hammer_speed(127).to_bits(), 5.75_f64.to_bits());
        for velocity in 1..127 {
            let a = midi_velocity_to_hammer_speed(velocity);
            let b = midi_velocity_to_hammer_speed(velocity + 1);
            assert!(a.is_finite() && a > 0.0 && b > a);
        }
    }

    #[test]
    fn quiet_touch_cubic_joins_old_value_and_slope_at_sixteen() {
        assert!((quiet_touch_launch(16.0) - legacy(16.0)).abs() < 1e-14);
        let epsilon = 1e-3;
        let quiet_slope = (quiet_touch_launch(16.0 + epsilon) - quiet_touch_launch(16.0 - epsilon)) / (2.0 * epsilon);
        let old_slope = (legacy(16.0 + epsilon) - legacy(16.0 - epsilon)) / (2.0 * epsilon);
        assert!((quiet_slope - old_slope).abs() < 1e-9);
        let low_slope = (quiet_touch_launch(1.0 + epsilon) - quiet_touch_launch(1.0 - epsilon)) / (2.0 * epsilon);
        assert!(low_slope.abs() < 1e-9);
    }

    #[test]
    fn midi_velocity_zero_is_decoded_as_noteoff_not_a_quiet_launch() {
        let bytes = b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk\x00\x00\x00\x0c\x00\x90\x3c\x01\x01\x90\x3c\x00\x00\xff\x2f\x00";
        let events = crate::midi::parse_midi(bytes);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, crate::midi::MidiKind::NoteOn);
        assert_eq!(events[0].b, 1);
        assert_eq!(events[1].kind, crate::midi::MidiKind::NoteOff);
        assert_eq!(events[1].b, 0);
    }
}
