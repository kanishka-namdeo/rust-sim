//! ADR-0029: PX4 version policy enforcement — hard reject on mismatch.
//!
//! Queries the fleet manager (:8400) for the vehicle's reported PX4
//! version and rejects upload if it does not exactly match v1.16.2.
//! Returns HTTP 426 + PX4_VERSION_MISMATCH on mismatch.

#![forbid(unsafe_code)]

use std::time::Duration;

/// The pinned PX4 version. Exact match required (rejects -rc1, +dirty).
pub const REQUIRED_PX4_VERSION: &str = "v1.16.2";

/// Escape hatch for core-team capture sessions (ADR-0029 §escape hatch).
/// When set in the server's environment, the version check downgrades
/// from a hard reject to a warning.
pub fn allow_unpinned() -> bool {
    std::env::var("RSIM_ALLOW_UNPINNED_PX4")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Parse a PX4 version string like "v1.16.2" or "1.16.2" into (1, 16, 2).
/// Returns None if the string does not match the expected pattern.
/// Suffixes like "-rc1" or "+dirty" cause None (exact match required).
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim().trim_start_matches('v');
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    // Each part must be a pure integer (no suffixes)
    let major = parts[0].parse::<u32>().ok()?;
    let minor = parts[1].parse::<u32>().ok()?;
    let patch = parts[2].parse::<u32>().ok()?;
    Some((major, minor, patch))
}

/// Check if a reported version matches the required v1.16.2.
pub fn version_matches(reported: &str) -> bool {
    match parse_version(reported) {
        Some((1, 16, 2)) => true,
        _ => false,
    }
}

/// The result of a version check.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status")]
pub enum VersionCheckResult {
    /// Version matches v1.16.2; upload can proceed.
    Ok {
        reported_version: String,
        required_version: String,
    },
    /// Version does not match; upload is rejected (HTTP 426) unless
    /// `RSIM_ALLOW_UNPINNED_PX4=1` is set, in which case it's a warning.
    Mismatch {
        reported_version: String,
        required_version: String,
        warning_only: bool,
    },
    /// Version could not be determined within the timeout; upload is
    /// rejected (HTTP 503).
    Unavailable {
        timeout_s: u64,
    },
}

impl VersionCheckResult {
    pub fn is_blocking(&self) -> bool {
        match self {
            VersionCheckResult::Ok { .. } => false,
            VersionCheckResult::Mismatch { warning_only: true, .. } => false,
            VersionCheckResult::Mismatch { warning_only: false, .. } => true,
            VersionCheckResult::Unavailable { .. } => true,
        }
    }

    pub fn http_status(&self) -> u16 {
        match self {
            VersionCheckResult::Ok { .. } => 200,
            VersionCheckResult::Mismatch { warning_only: false, .. } => 426,
            VersionCheckResult::Mismatch { warning_only: true, .. } => 200,
            VersionCheckResult::Unavailable { .. } => 503,
        }
    }
}

/// Query the fleet manager (:8400) for the vehicle's reported PX4 version
/// and check it against the required v1.16.2.
///
/// `fleet_base_url` is e.g. "http://127.0.0.1:8400".
/// `vehicle_id` is the vehicle index (0, 1, ...).
pub async fn check_vehicle_version(
    fleet_base_url: &str,
    vehicle_id: u8,
) -> VersionCheckResult {
    let url = format!("{fleet_base_url}/api/vehicles/{vehicle_id}");
    let timeout = Duration::from_secs(2);

    let client = match tokio::time::timeout(
        timeout,
        reqwest::get(&url),
    ).await {
        Ok(Ok(resp)) => resp,
        Ok(Err(_)) => return VersionCheckResult::Unavailable { timeout_s: 2 },
        Err(_) => return VersionCheckResult::Unavailable { timeout_s: 2 },
    };

    let body: serde_json::Value = match client.json().await {
        Ok(v) => v,
        Err(_) => return VersionCheckResult::Unavailable { timeout_s: 2 },
    };

    // The fleet manager returns { ok, data: { ... vehicle state ... } }
    // Try several paths where px4_version might live.
    let reported = body
        .get("data")
        .and_then(|d| d.get("px4_version"))
        .or_else(|| body.get("px4_version"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if reported.is_empty() {
        return VersionCheckResult::Unavailable { timeout_s: 2 };
    }

    if version_matches(reported) {
        VersionCheckResult::Ok {
            reported_version: reported.to_string(),
            required_version: REQUIRED_PX4_VERSION.into(),
        }
    } else {
        VersionCheckResult::Mismatch {
            reported_version: reported.to_string(),
            required_version: REQUIRED_PX4_VERSION.into(),
            warning_only: allow_unpinned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exact_version() {
        assert_eq!(parse_version("v1.16.2"), Some((1, 16, 2)));
        assert_eq!(parse_version("1.16.2"), Some((1, 16, 2)));
    }

    #[test]
    fn reject_suffixed_versions() {
        assert_eq!(parse_version("v1.16.2-rc1"), None);
        assert_eq!(parse_version("v1.16.2+dirty"), None);
        assert_eq!(parse_version("v1.16.2-beta"), None);
    }

    #[test]
    fn reject_wrong_versions() {
        assert_eq!(parse_version("v1.18.0"), Some((1, 18, 0)));
        assert!(!version_matches("v1.18.0"));
        assert!(!version_matches("v1.16.1"));
        assert!(!version_matches("v2.0.0"));
    }

    #[test]
    fn accept_correct_version() {
        assert!(version_matches("v1.16.2"));
        assert!(version_matches("1.16.2"));
    }

    #[test]
    fn check_result_blocking_logic() {
        let ok = VersionCheckResult::Ok {
            reported_version: "v1.16.2".into(),
            required_version: REQUIRED_PX4_VERSION.into(),
        };
        assert!(!ok.is_blocking());
        assert_eq!(ok.http_status(), 200);

        let mismatch_hard = VersionCheckResult::Mismatch {
            reported_version: "v1.18.0".into(),
            required_version: REQUIRED_PX4_VERSION.into(),
            warning_only: false,
        };
        assert!(mismatch_hard.is_blocking());
        assert_eq!(mismatch_hard.http_status(), 426);

        let mismatch_warn = VersionCheckResult::Mismatch {
            reported_version: "v1.18.0".into(),
            required_version: REQUIRED_PX4_VERSION.into(),
            warning_only: true,
        };
        assert!(!mismatch_warn.is_blocking());
        assert_eq!(mismatch_warn.http_status(), 200);

        let unavail = VersionCheckResult::Unavailable { timeout_s: 2 };
        assert!(unavail.is_blocking());
        assert_eq!(unavail.http_status(), 503);
    }
}
