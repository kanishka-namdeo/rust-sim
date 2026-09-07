//! Geofence model (spec §8.2): inclusion polygon in local NED plus
//! altitude ceiling/floor. Point-in-polygon by ray casting, cross-checked
//! against an independent winding-number implementation in tests;
//! self-intersecting and degenerate polygons rejected at parse.

#![forbid(unsafe_code)]

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum GeofenceError {
    TooFewPoints(usize),
    Degenerate,
    SelfIntersection,
    BadAltitudeBox,
}

impl fmt::Display for GeofenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GeofenceError::TooFewPoints(n) => write!(f, "geofence needs >= 3 points, got {n}"),
            GeofenceError::Degenerate => write!(f, "geofence polygon is degenerate (zero area)"),
            GeofenceError::SelfIntersection => write!(f, "geofence polygon self-intersects"),
            GeofenceError::BadAltitudeBox => write!(f, "geofence ceiling must be above floor"),
        }
    }
}
impl std::error::Error for GeofenceError {}

#[derive(Debug, Clone, PartialEq)]
pub struct Geofence {
    /// Polygon vertices, NED x/y (m), closed implicitly.
    pub points: Vec<[f32; 2]>,
    /// Maximum altitude above home, metres (positive up).
    pub ceiling_m: f32,
    /// Minimum altitude above home, metres.
    pub floor_m: f32,
}

/// Warn distance from a boundary (spec §5.2 GEOFENCE_WARN).
pub const BOUNDARY_WARN_M: f32 = 10.0;
/// Deep-breach distance: outside by more than this -> LAND (spec §8.1).
pub const DEEP_BREACH_M: f32 = 10.0;
/// Estimate/truth disagreement persistence before treating as breach
/// (spec §8.2, fail-safe over fail-certain).
pub const DISAGREE_MS: u64 = 3000;

impl Geofence {
    /// Parse and validate: >= 3 distinct points, non-zero area, no
    /// self-intersection, sane altitude box.
    pub fn parse(points: Vec<[f32; 2]>, ceiling_m: f32, floor_m: f32) -> Result<Self, GeofenceError> {
        if points.len() < 3 {
            return Err(GeofenceError::TooFewPoints(points.len()));
        }
        if ceiling_m <= floor_m {
            return Err(GeofenceError::BadAltitudeBox);
        }
        if Self::polygon_area(&points).abs() < 1.0 {
            return Err(GeofenceError::Degenerate);
        }
        if Self::self_intersects(&points) {
            return Err(GeofenceError::SelfIntersection);
        }
        Ok(Geofence {
            points,
            ceiling_m,
            floor_m,
        })
    }

    /// Spec §18 default: 100 m square, 60 m ceiling, 0 floor.
    pub fn default_square() -> Self {
        Geofence::parse(
            vec![[-100.0, -100.0], [100.0, -100.0], [100.0, 100.0], [-100.0, 100.0]],
            60.0,
            0.0,
        )
        .expect("default fence is valid")
    }

    /// Ray-casting point-in-polygon (spec §8.2).
    pub fn contains_xy(&self, p: [f32; 2]) -> bool {
        point_in_polygon_ray_cast(p, &self.points)
    }

    /// Full 3-D containment: inside the polygon AND within the altitude
    /// box. NED z is negative-up: altitude box maps to
    /// `-ceiling <= z <= -floor`.
    pub fn contains_ned(&self, pos: [f32; 3]) -> bool {
        self.contains_xy([pos[0], pos[1]]) && self.altitude_ok(pos[2])
    }

    pub fn altitude_ok(&self, z_ned: f32) -> bool {
        (-self.ceiling_m..=-self.floor_m).contains(&z_ned)
    }

    /// Vertical breach signed distance (positive = outside the box).
    pub fn altitude_breach(&self, z_ned: f32) -> f32 {
        if z_ned > -self.floor_m {
            z_ned + self.floor_m
        } else if z_ned < -self.ceiling_m {
            -self.ceiling_m - z_ned
        } else {
            0.0
        }
    }

    /// Horizontal breach distance: 0 inside, distance to the nearest edge
    /// when outside.
    pub fn horizontal_breach(&self, p: [f32; 2]) -> f32 {
        if self.contains_xy(p) {
            0.0
        } else {
            self.distance_to_polygon(p)
        }
    }

    /// Distance to the nearest boundary edge (used for GEOFENCE_WARN and
    /// the deep-breach test). Inside or out.
    pub fn distance_to_polygon(&self, p: [f32; 2]) -> f32 {
        let n = self.points.len();
        let mut best = f32::INFINITY;
        for i in 0..n {
            let a = self.points[i];
            let b = self.points[(i + 1) % n];
            best = best.min(point_segment_distance(p, a, b));
        }
        best
    }

    /// True when within `BOUNDARY_WARN_M` of a boundary edge or the
    /// altitude box limits (inside the fence).
    pub fn boundary_warning(&self, pos: [f32; 3]) -> bool {
        let d = self.distance_to_polygon([pos[0], pos[1]]);
        if d <= BOUNDARY_WARN_M {
            return true;
        }
        let alt_m = -pos[2];
        alt_m <= self.floor_m + BOUNDARY_WARN_M || alt_m >= self.ceiling_m - BOUNDARY_WARN_M
    }

    /// Clamp a setpoint into the fence shrunk by `margin` (spec §7.2: the
    /// generator clamps to the polygon shrunk by 2 m so the manager's own
    /// commands never violate the fence). Approximate but conservative:
    /// the point is pulled toward the polygon's centroid until it is
    /// inside the shrunk polygon.
    pub fn clamp_setpoint(&self, p: [f32; 2], margin: f32) -> [f32; 2] {
        let shrunk = self.shrink(margin);
        if shrunk.contains_xy(p) {
            return p;
        }
        // binary search along the segment centroid -> p.
        let c = self.centroid();
        let mut lo = 0.0f32;
        let mut hi = 1.0f32;
        let mut best = c;
        for _ in 0..24 {
            let mid = (lo + hi) * 0.5;
            let q = [c[0] + (p[0] - c[0]) * mid, c[1] + (p[1] - c[1]) * mid];
            if shrunk.contains_xy(q) {
                lo = mid;
                best = q;
            } else {
                hi = mid;
            }
        }
        best
    }

    /// Clamp altitude into the box shrunk by margin.
    pub fn clamp_altitude(&self, z_ned: f32, margin: f32) -> f32 {
        let z = z_ned.clamp(-self.ceiling_m + margin, -self.floor_m - margin);
        z
    }

    fn centroid(&self) -> [f32; 2] {
        let n = self.points.len() as f32;
        let x: f32 = self.points.iter().map(|p| p[0]).sum::<f32>() / n;
        let y: f32 = self.points.iter().map(|p| p[1]).sum::<f32>() / n;
        [x, y]
    }

    /// Shrink the polygon toward its centroid by `margin` (approximation
    /// of the offset polygon; conservative for convex shapes).
    fn shrink(&self, margin: f32) -> Geofence {
        let c = self.centroid();
        let points = self
            .points
            .iter()
            .map(|&p| {
                let d = [p[0] - c[0], p[1] - c[1]];
                let len = d[0].hypot(d[1]).max(1e-6);
                let scale = ((len - margin) / len).max(0.05);
                [c[0] + d[0] * scale, c[1] + d[1] * scale]
            })
            .collect();
        Geofence {
            points,
            ceiling_m: self.ceiling_m - margin,
            floor_m: self.floor_m + margin,
        }
    }

    fn polygon_area(points: &[[f32; 2]]) -> f32 {
        let n = points.len();
        let mut area = 0.0;
        for i in 0..n {
            let a = points[i];
            let b = points[(i + 1) % n];
            area += a[0] * b[1] - b[0] * a[1];
        }
        area / 2.0
    }

    fn self_intersects(points: &[[f32; 2]]) -> bool {
        let n = points.len();
        for i in 0..n {
            let a1 = points[i];
            let a2 = points[(i + 1) % n];
            for j in (i + 1)..n {
                let b1 = points[j];
                let b2 = points[(j + 1) % n];
                // Adjacent edges share a vertex — skip.
                if j == i {
                    continue;
                }
                let adjacent = (i + 1) % n == j || (j + 1) % n == i;
                if adjacent {
                    continue;
                }
                if segments_intersect(a1, a2, b1, b2) {
                    return true;
                }
            }
        }
        false
    }
}

fn point_in_polygon_ray_cast(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let pi = poly[i];
        let pj = poly[j];
        let crosses = (pi[1] > p[1]) != (pj[1] > p[1]);
        if crosses {
            let x_at = (pj[0] - pi[0]) * (p[1] - pi[1]) / (pj[1] - pi[1]) + pi[0];
            if p[0] < x_at {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Independent implementation for cross-checks (spec §11.1).
/// Winding-number containment (Dan Sunday's formulation): counts signed
/// crossings of the ray from p in +x direction; inside iff winding != 0.
pub fn point_in_polygon_winding(p: [f32; 2], poly: &[[f32; 2]]) -> bool {
    let mut winding: i32 = 0;
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        // side > 0: p is left of the directed edge a->b.
        let side = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        if a[1] <= p[1] {
            // upward crossing?
            if b[1] > p[1] && side > 0.0 {
                winding += 1;
            }
        } else if b[1] <= p[1] && side < 0.0 {
            // downward crossing
            winding -= 1;
        }
    }
    winding != 0
}

fn point_segment_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    if len2 <= f32::EPSILON {
        return p[0].hypot(p[1] - a[1]) + (p[0] - a[0]).abs() * 0.0; // a==b
    }
    let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0);
    let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    (p[0] - q[0]).hypot(p[1] - q[1])
}

fn segments_intersect(a1: [f32; 2], a2: [f32; 2], b1: [f32; 2], b2: [f32; 2]) -> bool {
    let d1 = cross(b1, b2, a1);
    let d2 = cross(b1, b2, a2);
    let d3 = cross(a1, a2, b1);
    let d4 = cross(a1, a2, b2);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    // collinear-on-segment cases are edge cases we count as intersection
    let on = |p: [f32; 2], q1: [f32; 2], q2: [f32; 2]| {
        let d = cross(q1, q2, p);
        d.abs() <= 1e-6
            && p[0] >= q1[0].min(q2[0]) - 1e-6
            && p[0] <= q1[0].max(q2[0]) + 1e-6
            && p[1] >= q1[1].min(q2[1]) - 1e-6
            && p[1] <= q1[1].max(q2[1]) + 1e-6
    };
    on(a1, b1, b2) || on(a2, b1, b2) || on(b1, a1, a2) || on(b2, a1, a2)
}

fn cross(o: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Geofence {
        Geofence::default_square()
    }

    /// Spec §11.1: ray-casting vs winding-number on 1,000 random points.
    #[test]
    fn point_in_polygon_cross_implementation_1000_points() {
        let mut rng = crate::geofence::tests_rng::Rng(0x5AFE_2026);
        let poly: Vec<[f32; 2]> = vec![
            [-80.0, -40.0],
            [10.0, -90.0],
            [90.0, -10.0],
            [40.0, 60.0],
            [-30.0, 80.0],
        ];
        let fence = Geofence::parse(poly.clone(), 60.0, 0.0).unwrap();
        let mut disagreements = 0;
        for _ in 0..1000 {
            let p = [
                rng.uniform(-150.0, 150.0),
                rng.uniform(-150.0, 150.0),
            ];
            let a = fence.contains_xy(p);
            let b = point_in_polygon_winding(p, &poly);
            if a != b {
                // Boundary-precision points: count but require none beyond
                // a tiny band.
                let d = fence.distance_to_polygon(p);
                assert!(d < 0.01, "real disagreement at {p:?}: ray={a} winding={b}");
                disagreements += 1;
            }
        }
        // The two algorithms must agree away from the boundary.
        assert!(disagreements < 20, "too many near-boundary mismatches: {disagreements}");
    }

    #[test]
    fn square_containment_basics() {
        let f = square();
        assert!(f.contains_xy([0.0, 0.0]));
        assert!(f.contains_xy([99.0, 99.0]));
        assert!(!f.contains_xy([101.0, 0.0]));
        assert!(!f.contains_xy([-100.5, 0.0]));
        // altitude box: NED z negative up
        assert!(f.contains_ned([0.0, 0.0, -30.0]));
        assert!(!f.contains_ned([0.0, 0.0, -61.0])); // above 60 m ceiling
        assert!(!f.contains_ned([0.0, 0.0, 5.0])); // below floor
        assert!(!f.contains_ned([101.0, 0.0, -30.0]));
    }

    #[test]
    fn breach_distances() {
        let f = square();
        assert_eq!(f.horizontal_breach([0.0, 0.0]), 0.0);
        assert!((f.horizontal_breach([110.0, 0.0]) - 10.0).abs() < 1e-4);
        assert!((f.altitude_breach(-70.0) - 10.0).abs() < 1e-4);
        assert_eq!(f.altitude_breach(-30.0), 0.0);
        assert!((f.altitude_breach(7.0) - 7.0).abs() < 1e-4);
    }

    #[test]
    fn boundary_warning_band() {
        let f = square();
        assert!(!f.boundary_warning([0.0, 0.0, -30.0]));
        assert!(f.boundary_warning([92.0, 0.0, -30.0])); // 8 m from edge
        assert!(f.boundary_warning([0.0, 0.0, -55.0])); // 5 m from ceiling
        assert!(!f.boundary_warning([0.0, 0.0, -40.0]));
    }

    #[test]
    fn parse_rejects_bad_fences() {
        assert!(matches!(
            Geofence::parse(vec![[0.0, 0.0], [1.0, 1.0]], 60.0, 0.0),
            Err(GeofenceError::TooFewPoints(2))
        ));
        // degenerate: collinear
        assert!(matches!(
            Geofence::parse(vec![[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]], 60.0, 0.0),
            Err(GeofenceError::Degenerate)
        ));
        // self-intersecting pinched quad (unequal lobes so the area check
        // does not reject it as degenerate first): edges (30,0)->(0,20) and
        // (20,20)->(0,0) cross at (12,12).
        assert!(matches!(
            Geofence::parse(
                vec![[0.0, 0.0], [30.0, 0.0], [0.0, 20.0], [20.0, 20.0]],
                60.0,
                0.0
            ),
            Err(GeofenceError::SelfIntersection)
        ));
        // symmetric bowtie: zero signed area -> degenerate
        assert!(matches!(
            Geofence::parse(
                vec![[-10.0, -10.0], [10.0, 10.0], [10.0, -10.0], [-10.0, 10.0]],
                60.0,
                0.0
            ),
            Err(GeofenceError::Degenerate)
        ));
        assert!(matches!(
            Geofence::parse(vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]], 0.0, 10.0),
            Err(GeofenceError::BadAltitudeBox)
        ));
    }

    #[test]
    fn concave_polygon_supported() {
        // L-shape (concave): vertices A(-50,-50) B(50,-50) C(50,50)
        // D(0,50) E(0,0) F(-50,0) — interior is the bottom strip plus the
        // RIGHT column (x>0, y>0); the notch is the top-left quadrant.
        let f = Geofence::parse(
            vec![
                [-50.0, -50.0],
                [50.0, -50.0],
                [50.0, 50.0],
                [0.0, 50.0],
                [0.0, 0.0],
                [-50.0, 0.0],
            ],
            60.0,
            0.0,
        )
        .unwrap();
        assert!(f.contains_xy([-25.0, -25.0]));
        assert!(f.contains_xy([25.0, -25.0]));
        assert!(f.contains_xy([25.0, 25.0]), "right column is interior");
        assert!(!f.contains_xy([-25.0, 25.0]), "top-left notch must be outside");
        // cross-check against the winding implementation
        assert_eq!(
            f.contains_xy([-25.0, 25.0]),
            point_in_polygon_winding([-25.0, 25.0], &f.points)
        );
        assert_eq!(
            f.contains_xy([25.0, 25.0]),
            point_in_polygon_winding([25.0, 25.0], &f.points)
        );
    }

    #[test]
    fn setpoint_clamp_stays_inside_shrunk_fence() {
        let f = square();
        let margin = 2.0;
        for p in [
            [150.0, 0.0],
            [-150.0, 0.0],
            [0.0, 150.0],
            [99.0, 99.0],
            [-99.0, 99.0],
            [5.0, 5.0],
        ] {
            let q = f.clamp_setpoint(p, margin);
            // clamped point must be inside the original fence and at least
            // ~margin from the boundary (approximation tolerance).
            assert!(f.contains_xy(q), "clamp moved {p:?} outside: {q:?}");
            assert!(
                f.distance_to_polygon(q) > margin * 0.7,
                "clamp for {p:?} -> {q:?} too close to boundary (d={})",
                f.distance_to_polygon(q)
            );
        }
        assert!((f.clamp_altitude(-70.0, 2.0) + 58.0).abs() < 1e-4);
        assert!((f.clamp_altitude(-1.0, 2.0) + 2.0).abs() < 1e-4);
    }
}

/// Tiny deterministic RNG for tests (xorshift64*), shared shape with
/// fleet-alloc's to avoid a cross-crate dev-dependency.
#[cfg(test)]
pub(crate) mod tests_rng {
    pub struct Rng(pub u64);

    impl Rng {
        pub fn uniform(&mut self, lo: f32, hi: f32) -> f32 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            let u = (x.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f32 / (1u64 << 53) as f32;
            lo + u * (hi - lo)
        }
    }
}
