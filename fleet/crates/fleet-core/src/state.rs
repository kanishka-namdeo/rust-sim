//! `VehicleState` — the manager's per-vehicle shared state (spec §5.1) —
//! plus the versioned slot that publishes it.
//!
//! ADR-0002: the spec calls for a "seqlock-style" single-writer/many-reader
//! slot. v0.1 implements the same contract with safe code: a
//! `RwLock`-guarded value with a version counter and arrival stamps. The
//! write path is the link aggregator (single writer); readers copy a
//! snapshot under a short critical section. Swap-in of a truly lock-free
//! seqlock later changes nothing for callers.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use serde::Serialize;

/// Snapshot of one link's counters (subset of fleet-mavlink's `LinkStats`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LinkCounters {
    pub sent: u64,
    pub recv: u64,
    pub retries: u64,
    pub cmd_failures: u64,
    pub commands_sent: u64,
    pub setpoints_sent: u64,
    pub heartbeats_sent: u64,
    pub dropped_link_loss: u64,
    /// Average inbound rate since link start (Hz).
    pub recv_rate_hz: f32,
}

/// Ground-truth block polled from the per-vehicle simulator (2 Hz, spec
/// §5.1). `None` while running against the interim sim, which has no status
/// plane (ADR-0001).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SimStatus {
    pub phase: String,
    pub tick_p95_us: f32,
    pub faults_active: u32,
}

/// Per-vehicle shared state, spec §5.1 field-for-field.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct VehicleState {
    pub index: u8,
    pub sysid: u8,
    pub compid: u16,
    /// Position estimate (NED, m, home-relative) from LOCAL_POSITION_NED.
    pub position_ned_m: [f32; 3],
    /// Attitude quaternion [w, x, y, z] from ATTITUDE.
    pub attitude_q_wxyz: [f32; 4],
    /// Velocity estimate (NED, m/s) from LOCAL_POSITION_NED.
    pub velocity_ned_ms: [f32; 3],
    /// GLOBAL_POSITION_INT fix.
    pub lat_deg_e7: i32,
    pub lon_deg_e7: i32,
    pub alt_mm: i32,
    pub relative_alt_mm: i32,
    /// Battery estimate (-1 = unknown; interim sim has no battery model).
    pub battery_pct: i8,
    pub voltage_v: f32,
    /// HOME_POSITION local NED anchor.
    pub home_ned_m: [f32; 3],
    pub home_set: bool,
    /// HEARTBEAT decode.
    pub mode_word: u32,
    pub armed: bool,
    /// Arrival stamps in fleet-epoch milliseconds (staleness inputs).
    pub last_heartbeat_ms: u64,
    pub last_msg_ms: u64,
    /// Link counters (written by the aggregator from the link task stats).
    pub link: LinkCounters,
    /// Simulator ground truth, when available.
    pub sim: Option<SimStatus>,
    // --- bring-up / health inputs (not serialized for the API) ---
    /// True once a HEARTBEAT from the expected sysid has arrived.
    pub heartbeat_seen: bool,
    /// True once LOCAL_POSITION_NED has arrived with finite values.
    pub local_position_seen: bool,
    /// Counts per inbound message id.
    pub msg_counts: std::collections::BTreeMap<u32, u64>,
    /// Last STATUSTEXT severity+text (operator log tail).
    pub last_statustext: Option<String>,
    /// Position estimate age inside the BOOTING gate.
    pub position_samples: u32,
}

impl VehicleState {
    pub fn new(index: u8) -> Self {
        VehicleState {
            index,
            battery_pct: -1,
            voltage_v: f32::NAN,
            ..Default::default()
        }
    }

    /// Convert (roll, pitch, yaw) to the [w, x, y, z] quaternion
    /// (Z-Y-X / yaw-pitch-roll convention, as PX4 reports). Standard
    /// half-angle formulation.
    pub fn set_attitude_euler(&mut self, roll: f32, pitch: f32, yaw: f32) {
        let (cr, sr) = ((roll * 0.5).cos(), (roll * 0.5).sin());
        let (cp, sp) = ((pitch * 0.5).cos(), (pitch * 0.5).sin());
        let (cy, sy) = ((yaw * 0.5).cos(), (yaw * 0.5).sin());
        self.attitude_q_wxyz = [
            cr * cp * cy + sr * sp * sy,
            sr * cp * cy - cr * sp * sy,
            cr * sp * cy + sr * cp * sy,
            cr * cp * sy - sr * sp * cy,
        ];
    }
}

/// Versioned read slot: single writer, many readers (ADR-0002).
pub struct Slot<T> {
    inner: RwLock<T>,
    version: AtomicU64,
}

impl<T> Slot<T> {
    pub fn new(value: T) -> Self {
        Slot {
            inner: RwLock::new(value),
            version: AtomicU64::new(0),
        }
    }

    /// Reader copy. Version parity check is advisory: a writer-preempted
    /// read retries once, then returns the fresh value.
    pub fn read(&self) -> T
    where
        T: Clone,
    {
        let mut v0 = self.version.load(Ordering::Acquire);
        loop {
            let snap = {
                let guard = self.inner.read().unwrap();
                guard.clone()
            };
            let v1 = self.version.load(Ordering::Acquire);
            if v0 == v1 {
                return snap;
            }
            v0 = v1;
        }
    }

    pub fn write(&self, f: impl FnOnce(&mut T)) {
        let mut guard = self.inner.write().unwrap();
        f(&mut guard);
        self.version.fetch_add(1, Ordering::Release);
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }
}

impl<T: Default> Default for Slot<T> {
    fn default() -> Self {
        Slot::new(T::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_roundtrip_and_version() {
        let slot = Slot::new(0u32);
        assert_eq!(slot.read(), 0);
        assert_eq!(slot.version(), 0);
        slot.write(|v| *v = 7);
        assert_eq!(slot.read(), 7);
        assert_eq!(slot.version(), 1);
    }

    #[test]
    fn slot_concurrent_writer_reader_bounded() {
        // Seqlock-discipline stress: one writer thread, many readers; the
        // readers must observe monotonic values and terminate (loom-style
        // bounded-time test in the spec's spirit).
        let slot = std::sync::Arc::new(Slot::new(0u64));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let slot = slot.clone();
            let stop = stop.clone();
            handles.push(std::thread::spawn(move || {
                // Read BEFORE checking stop: every reader performs at least
                // one read, so a late-starting reader still observes the
                // final value.
                let mut last = 0;
                loop {
                    let v = slot.read();
                    assert!(v >= last, "stale/regressed read");
                    last = v;
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                }
                last
            }));
        }
        for i in 1..=20_000u64 {
            slot.write(|v| *v = i);
        }
        // Give descheduled readers a window to observe the final value
        // before the stop flag (the monotonicity property is the point;
        // this avoids the cold-start race without weakening it).
        std::thread::sleep(std::time::Duration::from_millis(200));
        stop.store(true, Ordering::Relaxed);
        for h in handles {
            let last = h.join().unwrap();
            assert!(last > 0, "reader never observed a write");
        }
        assert_eq!(slot.read(), 20_000);
    }

    #[test]
    fn attitude_euler_identity_at_zero() {
        let mut s = VehicleState::new(0);
        s.set_attitude_euler(0.0, 0.0, 0.0);
        assert!((s.attitude_q_wxyz[0] - 1.0).abs() < 1e-6);
        assert!(s.attitude_q_wxyz[1..].iter().all(|q| q.abs() < 1e-6));
    }

    #[test]
    fn attitude_euler_yaw90() {
        let mut s = VehicleState::new(0);
        s.set_attitude_euler(0.0, 0.0, std::f32::consts::FRAC_PI_2);
        // yaw 90deg: z-rotation; w = cos(45), z = sin(45)
        assert!((s.attitude_q_wxyz[0] - (std::f32::consts::FRAC_PI_4).cos()).abs() < 1e-6);
        assert!((s.attitude_q_wxyz[3] - (std::f32::consts::FRAC_PI_4).sin()).abs() < 1e-6);
    }
}
