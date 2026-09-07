//! Quaternion utilities (w-first, body-to-NED, SPEC §5.1).
//!
//! All functions are pure and allocation-free; `sqrt` comes from libm so
//! sitsim-core stays `no_std`-compatible (see ADR-001).

/// Hamilton product a ⊗ b (w-first).
pub fn quat_mul(a: &[f64; 4], b: &[f64; 4]) -> [f64; 4] {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

pub fn quat_norm(q: &[f64; 4]) -> f64 {
    libm::sqrt(q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3])
}

/// Renormalize to unit norm (returns the input if it is already unit within
/// 1e-15, to avoid gratuitous float churn).
pub fn quat_normalize(q: &[f64; 4]) -> [f64; 4] {
    let n = quat_norm(q);
    if (n - 1.0).abs() < 1e-15 {
        *q
    } else {
        [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
    }
}

/// Rotation matrix R(q) with q body->NED: `v_ned = R * v_body`.
/// Row-major.
pub fn quat_to_rot(q: &[f64; 4]) -> [[f64; 3]; 3] {
    let [w, x, y, z] = *q;
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// R^T * v (body-frame vector of a NED vector).
pub fn rot_t_mul(r: &[[f64; 3]; 3], v: &[f64; 3]) -> [f64; 3] {
    [
        r[0][0] * v[0] + r[1][0] * v[1] + r[2][0] * v[2],
        r[0][1] * v[0] + r[1][1] * v[1] + r[2][1] * v[2],
        r[0][2] * v[0] + r[1][2] * v[1] + r[2][2] * v[2],
    ]
}

/// R * v (NED vector of a body-frame vector).
pub fn rot_mul(r: &[[f64; 3]; 3], v: &[f64; 3]) -> [f64; 3] {
    [
        r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2],
        r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2],
        r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2],
    ]
}

/// ZYX Euler angles (roll, pitch, yaw) in radians from a body->NED
/// quaternion (PX4 FRD/NED convention).
pub fn quat_to_euler(q: &[f64; 4]) -> (f64, f64, f64) {
    let [w, x, y, z] = *q;
    let sinr_cosp = 2.0 * (w * x + y * z);
    let cosr_cosp = 1.0 - 2.0 * (x * x + y * y);
    let roll = libm::atan2(sinr_cosp, cosr_cosp);
    let sinp = 2.0 * (w * y - z * x);
    let pitch = if sinp.abs() >= 1.0 {
        core::f64::consts::FRAC_PI_2 * sinp.signum()
    } else {
        libm::asin(sinp)
    };
    let siny_cosp = 2.0 * (w * z + x * y);
    let cosy_cosp = 1.0 - 2.0 * (y * y + z * z);
    let yaw = libm::atan2(siny_cosp, cosy_cosp);
    (roll, pitch, yaw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_identity() {
        let q = [1.0, 0.0, 0.0, 0.0];
        let r = quat_to_rot(&q);
        let v = [1.0, -2.0, 3.0];
        assert_eq!(rot_mul(&r, &v), v);
        assert_eq!(rot_t_mul(&r, &v), v);
    }

    #[test]
    fn quaternion_rotation_matches_euler_for_90deg_yaw() {
        // yaw 90°: body x (front) should point East (+y NED).
        let half = core::f64::consts::FRAC_PI_4;
        let q = [half.cos(), 0.0, 0.0, half.sin()];
        let r = quat_to_rot(&q);
        let front = rot_mul(&r, &[1.0, 0.0, 0.0]);
        assert!((front[0] - 0.0).abs() < 1e-12);
        assert!((front[1] - 1.0).abs() < 1e-12);
        assert!((front[2] - 0.0).abs() < 1e-12);
        let (roll, pitch, yaw) = quat_to_euler(&q);
        assert!(roll.abs() < 1e-12);
        assert!(pitch.abs() < 1e-12);
        assert!((yaw - core::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn quat_mul_identity_and_norm() {
        let q = [0.9, 0.1, 0.2, 0.3];
        let id = [1.0, 0.0, 0.0, 0.0];
        let m = quat_mul(&q, &id);
        for i in 0..4 {
            assert!((m[i] - q[i]).abs() < 1e-15);
        }
        let n = quat_normalize(&q);
        assert!((quat_norm(&n) - 1.0).abs() < 1e-15);
    }
}
