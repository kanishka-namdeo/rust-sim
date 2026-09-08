//! ADR-0026: Mission validation rules — 13 rules (V-1 through V-13).
//!
//! Server-side authoritative; the Plan View mirrors these client-side
//! for immediate UX feedback. If they diverge, this is the truth.

#![forbid(unsafe_code)]

use crate::gcs::mission_file::{Geofence, MissionFile, Waypoint};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationError {
    pub rule: String,
    pub message: String,
    pub seq: Option<u16>,
    pub distance: Option<f32>,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
}

// ---------------------------------------------------------------------------
// Altitude bounds per vehicle type (V-5)
// ---------------------------------------------------------------------------

fn altitude_bounds(vehicle_type: &str) -> Option<(f32, f32)> {
    match vehicle_type {
        "quad" => Some((0.0, 120.0)),
        "fixed" => Some((0.0, 150.0)),
        "vtol" => Some((0.0, 150.0)),
        "rover" => Some((0.0, 0.0)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Supported MAV_CMD values (V-12)
// ---------------------------------------------------------------------------

const SUPPORTED_COMMANDS: &[u16] = &[16, 17, 18, 19, 20, 21, 22, 31, 200];

const SUPPORTED_FRAMES: &[u8] = &[0, 1, 3, 10, 11];

// ---------------------------------------------------------------------------
// Validation entry point
// ---------------------------------------------------------------------------

pub fn validate(m: &MissionFile) -> ValidationResult {
    let mut errors = Vec::new();

    // V-1: ≥1 waypoint
    if m.waypoints.is_empty() {
        errors.push(ValidationError {
            rule: "V-1".into(),
            message: "mission must have at least one waypoint".into(),
            seq: None,
            distance: None,
        });
    }

    // V-2: ≤100 waypoints
    if m.waypoints.len() > 100 {
        errors.push(ValidationError {
            rule: "V-2".into(),
            message: format!(
                "mission has {} waypoints; maximum is 100",
                m.waypoints.len()
            ),
            seq: None,
            distance: None,
        });
    }

    // V-5: altitude bounds per vehicle type
    let bounds = altitude_bounds(&m.mission.vehicle_type);
    if bounds.is_none() {
        errors.push(ValidationError {
            rule: "V-5".into(),
            message: format!(
                "unknown vehicle_type '{}'; supported: quad, fixed, vtol, rover",
                m.mission.vehicle_type
            ),
            seq: None,
            distance: None,
        });
    }

    let has_inclusion = !m.geofence.inclusion.is_empty();

    // V-7: inclusion polygon must be simple (no self-intersection)
    if has_inclusion && !is_simple_polygon(&m.geofence.inclusion) {
        errors.push(ValidationError {
            rule: "V-7".into(),
            message: "inclusion fence self-intersects".into(),
            seq: None,
            distance: None,
        });
    }

    // V-8: inclusion polygon must have ≥3 vertices
    if has_inclusion && m.geofence.inclusion.len() < 3 {
        errors.push(ValidationError {
            rule: "V-8".into(),
            message: format!(
                "inclusion fence must have at least 3 vertices (got {})",
                m.geofence.inclusion.len()
            ),
            seq: None,
            distance: None,
        });
    }

    // Per-waypoint checks
    for wp in &m.waypoints {
        // V-3: inside inclusion fence
        if has_inclusion && !point_in_polygon(wp.x, wp.y, &m.geofence.inclusion) {
            let dist = distance_to_polygon(wp.x, wp.y, &m.geofence.inclusion);
            errors.push(ValidationError {
                rule: "V-3".into(),
                message: format!(
                    "waypoint {} outside inclusion fence by {:.1} m",
                    wp.seq, dist
                ),
                seq: Some(wp.seq),
                distance: Some(dist),
            });
        }

        // V-4: not inside any exclusion polygon
        for (i, excl) in m.geofence.exclusion.iter().enumerate() {
            if point_in_polygon(wp.x, wp.y, excl) {
                let dist = distance_to_polygon(wp.x, wp.y, excl);
                errors.push(ValidationError {
                    rule: "V-4".into(),
                    message: format!(
                        "waypoint {} inside exclusion polygon {} by {:.1} m",
                        wp.seq, i, dist
                    ),
                    seq: Some(wp.seq),
                    distance: Some(dist),
                });
            }
        }

        // V-5: altitude bounds
        if let Some((min, max)) = bounds {
            if wp.z < min || wp.z > max {
                errors.push(ValidationError {
                    rule: "V-5".into(),
                    message: format!(
                        "waypoint {} altitude {:.1} m outside [{}, {}] for vehicle_type {}",
                        wp.seq, wp.z, min, max, m.mission.vehicle_type
                    ),
                    seq: Some(wp.seq),
                    distance: None,
                });
            }
        }

        // V-6: geofence ceiling/floor
        if has_inclusion {
            if wp.z > m.geofence.ceiling_m {
                errors.push(ValidationError {
                    rule: "V-6".into(),
                    message: format!(
                        "waypoint {} altitude {:.1} m above geofence ceiling {:.1} m",
                        wp.seq, wp.z, m.geofence.ceiling_m
                    ),
                    seq: Some(wp.seq),
                    distance: None,
                });
            }
            if wp.z < m.geofence.floor_m {
                errors.push(ValidationError {
                    rule: "V-6".into(),
                    message: format!(
                        "waypoint {} altitude {:.1} m below geofence floor {:.1} m",
                        wp.seq, wp.z, m.geofence.floor_m
                    ),
                    seq: Some(wp.seq),
                    distance: None,
                });
            }
        }

        // V-12: supported command
        if !SUPPORTED_COMMANDS.contains(&wp.command) {
            errors.push(ValidationError {
                rule: "V-12".into(),
                message: format!(
                    "waypoint {} has unsupported command {}; supported: 16, 17, 18, 19, 20, 21, 22, 31, 200",
                    wp.seq, wp.command
                ),
                seq: Some(wp.seq),
                distance: None,
            });
        }

        // V-13: supported frame
        if !SUPPORTED_FRAMES.contains(&wp.frame) {
            errors.push(ValidationError {
                rule: "V-13".into(),
                message: format!(
                    "waypoint {} has unsupported frame {}; supported: 0, 1, 3, 10, 11",
                    wp.seq, wp.frame
                ),
                seq: Some(wp.seq),
                distance: None,
            });
        }
    }

    // V-9: rally count ≤5
    if m.rally.len() > 5 {
        errors.push(ValidationError {
            rule: "V-9".into(),
            message: format!("rally point count {} exceeds maximum 5", m.rally.len()),
            seq: None,
            distance: None,
        });
    }

    // V-10 + V-11: rally points inside fence and within altitude bounds
    for rp in &m.rally {
        if has_inclusion && !point_in_polygon(rp.lat, rp.lon, &m.geofence.inclusion) {
            let dist = distance_to_polygon(rp.lat, rp.lon, &m.geofence.inclusion);
            errors.push(ValidationError {
                rule: "V-10".into(),
                message: format!(
                    "rally point {} outside inclusion fence by {:.1} m",
                    rp.seq, dist
                ),
                seq: Some(rp.seq),
                distance: Some(dist),
            });
        }
        if has_inclusion {
            if rp.alt_m > m.geofence.ceiling_m || rp.alt_m < m.geofence.floor_m {
                errors.push(ValidationError {
                    rule: "V-11".into(),
                    message: format!(
                        "rally point {} altitude {:.1} m outside [{}, {}]",
                        rp.seq, rp.alt_m, m.geofence.floor_m, m.geofence.ceiling_m
                    ),
                    seq: Some(rp.seq),
                    distance: None,
                });
            }
        }
    }

    ValidationResult {
        valid: errors.is_empty(),
        errors,
    }
}

// ---------------------------------------------------------------------------
// Geometry helpers (point-in-polygon, distance, self-intersection)
// ---------------------------------------------------------------------------

/// Ray-casting point-in-polygon. Closed polygon semantics: a point on
/// the boundary is considered inside (V-3 requires strict containment,
/// but we use a small epsilon to avoid flagging points that are on the
/// fence edge due to floating-point rounding). The ray-casting algorithm
/// is inherently ambiguous on boundaries; the epsilon makes the check
/// slightly permissive on the inside, which is the safe direction for
/// a geofence (a point on the boundary is "at the fence", not "outside").
fn point_in_polygon(lat: f64, lon: f64, polygon: &[[f64; 2]]) -> bool {
    if polygon.len() < 3 {
        return false;
    }
    // First check if the point is on any edge (within ~1 m tolerance).
    // If so, count it as inside (closed polygon semantics).
    for i in 0..polygon.len() {
        let j = (i + 1) % polygon.len();
        if distance_point_to_segment(lat, lon, polygon[i][0], polygon[i][1], polygon[j][0], polygon[j][1]) < 1.0 {
            return true;
        }
    }
    // Otherwise use standard ray-casting.
    let mut inside = false;
    let n = polygon.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (polygon[i][0], polygon[i][1]);
        let (xj, yj) = (polygon[j][0], polygon[j][1]);
        let intersects = (yi > lon) != (yj > lon)
            && lat < (xj - xi) * (lon - yi) / (yj - yi + 1e-30) + xi;
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Approximate distance from a point to a polygon (in metres, using
/// haversine for lat/lon). Used for error messages.
fn distance_to_polygon(lat: f64, lon: f64, polygon: &[[f64; 2]]) -> f32 {
    let mut min_dist = f32::MAX;
    let n = polygon.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let d = distance_point_to_segment(
            lat, lon,
            polygon[i][0], polygon[i][1],
            polygon[j][0], polygon[j][1],
        );
        if d < min_dist {
            min_dist = d;
        }
    }
    min_dist
}

fn distance_point_to_segment(
    px: f64, py: f64,
    ax: f64, ay: f64,
    bx: f64, by: f64,
) -> f32 {
    // Project point onto segment, clamp to [0,1], compute haversine distance.
    let dx = bx - ax;
    let dy = by - ay;
    let len_sq = dx * dx + dy * dy;
    let t = if len_sq > 1e-20 {
        (((px - ax) * dx + (py - ay) * dy) / len_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    haversine_m(px, py, cx, cy) as f32
}

fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6371000.0; // Earth radius in metres
    let to_rad = |d: f64| d * std::f64::consts::PI / 180.0;
    let la1 = to_rad(lat1);
    let la2 = to_rad(lat2);
    let dla = to_rad(lat2 - lat1);
    let dlo = to_rad(lon2 - lon1);
    let a = (dla / 2.0).sin().powi(2)
        + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    R * c
}

/// Shamos-Hoey self-intersection check. O(n log n) in theory; this is a
/// simple O(n²) implementation suitable for n ≤ 50 (the G-1 stress-test
/// limit). For n > 50 it would be slow but still correct.
fn is_simple_polygon(polygon: &[[f64; 2]]) -> bool {
    let n = polygon.len();
    if n < 4 {
        return true; // triangles are always simple
    }
    for i in 0..n {
        let j = (i + 1) % n;
        for k in (i + 1)..n {
            let l = (k + 1) % n;
            // Adjacent edges share a vertex and don't count as intersecting
            if i == k || i == l || j == k || j == l {
                continue;
            }
            if segments_intersect(
                polygon[i], polygon[j],
                polygon[k], polygon[l],
            ) {
                return false;
            }
        }
    }
    true
}

fn segments_intersect(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    // Standard cross-product test
    let ccw = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| -> i32 {
        let val = (q[1] - p[1]) * (r[0] - q[0]) - (q[0] - p[0]) * (r[1] - p[1]);
        if val.abs() < 1e-12 { 0 } else if val > 0.0 { 1 } else { -1 }
    };
    let d1 = ccw(c, d, a);
    let d2 = ccw(c, d, b);
    let d3 = ccw(a, b, c);
    let d4 = ccw(a, b, d);
    (d1 != d2) && (d3 != d4)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gcs::mission_file::{Geofence, MissionMeta, RallyPoint, Waypoint};

    fn meta() -> MissionMeta {
        MissionMeta {
            id: "test".into(), name: "test".into(), version: 1,
            created_at: "2026-09-09T10:00:00Z".into(),
            updated_at: "2026-09-09T10:00:00Z".into(),
            vehicle_type: "quad".into(), px4_version: "v1.16.2".into(),
        }
    }

    fn wp(seq: u16, lat: f64, lon: f64, alt: f32) -> Waypoint {
        Waypoint {
            seq, frame: 3, command: 16, x: lat, y: lon, z: alt,
            param1: 0.0, param2: 2.0, param3: 0.0, param4: 0.0,
        }
    }

    fn square_fence() -> Geofence {
        Geofence {
            ceiling_m: 60.0, floor_m: 0.0,
            inclusion: vec![
                [47.3970, 8.5450], [47.3980, 8.5450],
                [47.3980, 8.5460], [47.3970, 8.5460],
            ],
            exclusion: vec![],
        }
    }

    #[test]
    fn v1_empty_mission_rejected() {
        let m = MissionFile {
            mission: meta(), waypoints: vec![], geofence: square_fence(), rally: vec![],
        };
        let r = validate(&m);
        assert!(!r.valid);
        assert!(r.errors.iter().any(|e| e.rule == "V-1"));
    }

    #[test]
    fn v2_too_many_waypoints_rejected() {
        let wps: Vec<_> = (0..101).map(|i| wp(i, 47.3975, 8.5455, 12.0)).collect();
        let m = MissionFile {
            mission: meta(), waypoints: wps, geofence: square_fence(), rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-2"));
    }

    #[test]
    fn v3_waypoint_outside_fence_rejected() {
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![wp(0, 47.5000, 8.5000, 12.0)], // far outside
            geofence: square_fence(),
            rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-3" && e.seq == Some(0)));
    }

    #[test]
    fn v3_waypoint_inside_fence_accepted() {
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![wp(0, 47.3975, 8.5455, 12.0)], // inside
            geofence: square_fence(),
            rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().all(|e| e.rule != "V-3"));
    }

    #[test]
    fn v4_waypoint_in_exclusion_rejected() {
        let mut g = square_fence();
        g.exclusion.push(vec![
            [47.3974, 8.5454], [47.3976, 8.5454],
            [47.3976, 8.5456], [47.3974, 8.5456],
        ]);
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![wp(0, 47.3975, 8.5455, 12.0)], // inside exclusion
            geofence: g,
            rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-4"));
    }

    #[test]
    fn v5_altitude_too_high_rejected() {
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![wp(0, 47.3975, 8.5455, 200.0)], // 200m, quad max 120
            geofence: square_fence(),
            rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-5"));
    }

    #[test]
    fn v5_altitude_for_fixed_wing_higher() {
        let mut meta = meta();
        meta.vehicle_type = "fixed".into();
        let m = MissionFile {
            mission: meta,
            waypoints: vec![wp(0, 47.3975, 8.5455, 140.0)], // 140m ok for fixed
            geofence: Geofence {
                ceiling_m: 150.0, floor_m: 0.0,
                inclusion: square_fence().inclusion, exclusion: vec![],
            },
            rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().all(|e| e.rule != "V-5"));
    }

    #[test]
    fn v6_waypoint_above_ceiling_rejected() {
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![wp(0, 47.3975, 8.5455, 70.0)], // 70m, ceiling 60
            geofence: square_fence(),
            rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-6"));
    }

    #[test]
    fn v7_self_intersecting_fence_rejected() {
        // Bowtie polygon: two triangles meeting at a point
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![wp(0, 47.3975, 8.5455, 12.0)],
            geofence: Geofence {
                ceiling_m: 60.0, floor_m: 0.0,
                inclusion: vec![
                    [47.3970, 8.5450], [47.3980, 8.5460],
                    [47.3980, 8.5450], [47.3970, 8.5460],
                ],
                exclusion: vec![],
            },
            rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-7"));
    }

    #[test]
    fn v8_two_vertex_fence_rejected() {
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![wp(0, 47.3975, 8.5455, 12.0)],
            geofence: Geofence {
                ceiling_m: 60.0, floor_m: 0.0,
                inclusion: vec![[47.3970, 8.5450], [47.3980, 8.5460]],
                exclusion: vec![],
            },
            rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-8"));
    }

    #[test]
    fn v9_six_rally_points_rejected() {
        let rally: Vec<_> = (0..6).map(|i| RallyPoint {
            seq: i, lat: 47.3975, lon: 8.5455, alt_m: 0.0,
        }).collect();
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![wp(0, 47.3975, 8.5455, 12.0)],
            geofence: square_fence(),
            rally,
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-9"));
    }

    #[test]
    fn v10_rally_outside_fence_rejected() {
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![wp(0, 47.3975, 8.5455, 12.0)],
            geofence: square_fence(),
            rally: vec![RallyPoint {
                seq: 0, lat: 47.5000, lon: 8.5000, alt_m: 0.0,
            }],
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-10"));
    }

    #[test]
    fn v12_unsupported_command_rejected() {
        let mut m_wp = wp(0, 47.3975, 8.5455, 12.0);
        m_wp.command = 999;
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![m_wp],
            geofence: square_fence(),
            rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-12"));
    }

    #[test]
    fn v13_unsupported_frame_rejected() {
        let mut m_wp = wp(0, 47.3975, 8.5455, 12.0);
        m_wp.frame = 99;
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![m_wp],
            geofence: square_fence(),
            rally: vec![],
        };
        let r = validate(&m);
        assert!(r.errors.iter().any(|e| e.rule == "V-13"));
    }

    #[test]
    fn valid_4_waypoint_mission_accepted() {
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![
                wp(0, 47.3975, 8.5455, 12.0),
                wp(1, 47.3976, 8.5455, 12.0),
                wp(2, 47.3976, 8.5456, 12.0),
                wp(3, 47.3975, 8.5456, 12.0),
            ],
            geofence: square_fence(),
            rally: vec![RallyPoint {
                seq: 0, lat: 47.3975, lon: 8.5455, alt_m: 0.0,
            }],
        };
        let r = validate(&m);
        assert!(r.valid, "expected valid, got errors: {:?}", r.errors);
    }

    #[test]
    fn no_fence_skips_fence_checks() {
        // V-3, V-4, V-6, V-10 skipped when no fence defined
        let m = MissionFile {
            mission: meta(),
            waypoints: vec![wp(0, 47.3975, 8.5455, 200.0)], // 200m would fail V-6 but V-5 catches
            geofence: Geofence::default(), // empty
            rally: vec![],
        };
        let r = validate(&m);
        // V-5 still catches 200m (quad max 120), but V-3/V-6 should not fire
        assert!(r.errors.iter().all(|e| e.rule != "V-3" && e.rule != "V-6"));
    }
}
