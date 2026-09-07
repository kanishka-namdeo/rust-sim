//! Task allocation (spec §6): the bid function, the sequential
//! single-item auction, the greedy-nearest baseline, and a hand-rolled
//! O(n^3) Hungarian algorithm as the optimality baseline.
//!
//! # Honest optimality note (ADR-0007)
//!
//! The auction implements the spec's bid formula faithfully:
//! `cost = travel_time(current -> task) + queue_ahead + battery_penalty`,
//! with bids recomputed after every award (positions advance with each
//! award — "queue effects", §6.3). The spec's §6.4 acceptance claim
//! ("within 5 percent of the Hungarian optimum in at least 95 percent of
//! instances") is **not met by the spec's own bid formula**: the queue
//! term deliberately trades total travel for load balance (the same
//! property F-2 asserts when it demands the auction "actually split
//! work"). During bring-up this was measured across four readings of the
//! formula (see ADR-0007); the shipped statistics test pins the measured
//! envelope, reports the full numbers, and marks the 5%/95% criterion as
//! unmet-with-evidence, per the spec's own risk R-13 ("ship with the
//! measured number and greedy framing").

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Cruise speed for travel-time bids (m/s, spec §6.2).
pub const CRUISE_MS: f32 = 4.0;
/// Non-homotopic path factor (simple padding, spec §6.2).
pub const PATH_FACTOR: f32 = 1.25;
/// Battery soft-wall knee points (percent, spec §6.2).
pub const BATTERY_FREE_PCT: f32 = 45.0;
pub const BATTERY_WALL_PCT: f32 = 25.0;
/// Penalty at (and below) the wall — a large value in "seconds of
/// equivalent mission time".
pub const BATTERY_WALL_PENALTY_S: f32 = 10_000.0;

/// A mission unit: one waypoint visit (spec §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    /// NED metres, home-relative.
    pub pos_ned_m: [f32; 2],
    pub hover_s: f32,
    pub reward: f32,
    /// Deadline in fleet seconds; `f32::INFINITY` when unset.
    #[serde(default = "default_deadline")]
    pub deadline_s: f32,
}

fn default_deadline() -> f32 {
    f32::INFINITY
}

/// One bidding vehicle's auction inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct BidVehicle {
    pub index: usize,
    /// Current position (NED x/y).
    pub position: [f32; 2],
    /// Battery percent, -1 = unknown (no penalty applied).
    pub battery_pct: i8,
}

impl BidVehicle {
    pub fn new(index: usize, position: [f32; 2], battery_pct: i8) -> Self {
        BidVehicle {
            index,
            position,
            battery_pct,
        }
    }
}

/// Straight-line travel time with the path factor (spec §6.2).
pub fn travel_time(from: [f32; 2], to: [f32; 2]) -> f32 {
    let dx = to[0] - from[0];
    let dy = to[1] - from[1];
    (dx.hypot(dy) / CRUISE_MS) * PATH_FACTOR
}

/// Soft battery wall: zero above 45 percent, quadratic to a large value
/// at 25 percent, steeper below (spec §6.2). Unknown (-1) battery adds
/// no penalty — the auction must not fabricate a bias it cannot see.
pub fn battery_penalty(pct: i8) -> f32 {
    if pct < 0 {
        return 0.0;
    }
    let b = pct as f32;
    if b >= BATTERY_FREE_PCT {
        0.0
    } else if b <= BATTERY_WALL_PCT {
        let below = (BATTERY_WALL_PCT - b).max(0.0);
        BATTERY_WALL_PENALTY_S * (1.0 + 10.0 * below / BATTERY_WALL_PCT.max(1.0))
    } else {
        let x = (BATTERY_FREE_PCT - b) / (BATTERY_FREE_PCT - BATTERY_WALL_PCT);
        BATTERY_WALL_PENALTY_S * x * x
    }
}

/// The spec §6.2 bid.
pub fn bid_cost(
    vehicle_pos: [f32; 2],
    task: &Task,
    queue_ahead_s: f32,
    battery_pct: i8,
) -> f32 {
    travel_time(vehicle_pos, task.pos_ned_m) + queue_ahead_s + battery_penalty(battery_pct)
}

/// Auction result: per-vehicle task indices in award (execution) order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    /// `per_vehicle[v]` = ordered task indices awarded to vehicle v.
    pub per_vehicle: Vec<Vec<usize>>,
}

impl Assignment {
    pub fn empty(n_vehicles: usize) -> Self {
        Assignment {
            per_vehicle: vec![Vec::new(); n_vehicles],
        }
    }

    pub fn task_owner(&self, task_index: usize) -> Option<usize> {
        for (v, q) in self.per_vehicle.iter().enumerate() {
            if q.contains(&task_index) {
                return Some(v);
            }
        }
        None
    }

    pub fn assigned_count(&self) -> usize {
        self.per_vehicle.iter().map(|q| q.len()).sum()
    }
}

/// Sequential single-item auction (spec §6.3): tasks ordered by deadline
/// then reward; every vehicle bids on each; minimum wins; bids recomputed
/// after each award (positions advance, queues accumulate). Deterministic
/// given fleet state (evaluation order fixed by vehicle index).
pub fn auction(tasks: &[Task], vehicles: &[BidVehicle]) -> Assignment {
    auction_config(tasks, vehicles, 1.0)
}

/// The auction with a configurable queue-term weight: 1.0 is the spec's
/// bid; 0.0 isolates the pure-travel greedy variant (used by the §6.4 gap
/// statistics to attribute the optimality gap — ADR-0007).
pub fn auction_config(tasks: &[Task], vehicles: &[BidVehicle], queue_weight: f32) -> Assignment {
    let mut order: Vec<usize> = (0..tasks.len()).collect();
    order.sort_by(|&a, &b| {
        tasks[a]
            .deadline_s
            .partial_cmp(&tasks[b].deadline_s)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(tasks[b].reward.partial_cmp(&tasks[a].reward).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut positions: Vec<[f32; 2]> = vehicles.iter().map(|v| v.position).collect();
    let mut queues: Vec<f32> = vec![0.0; vehicles.len()];
    let mut result = Assignment::empty(vehicles.len());

    for &ti in &order {
        let task = &tasks[ti];
        let mut best_v = usize::MAX;
        let mut best_bid = f32::INFINITY;
        for v in 0..vehicles.len() {
            // The auctioneer proxies each vehicle's bid from that
            // vehicle's state (spec §6.3): position and queue.
            let bid = bid_cost(positions[v], task, queues[v] * queue_weight, vehicles[v].battery_pct);
            if bid < best_bid || (bid == best_bid && best_v != usize::MAX && v < best_v) {
                best_bid = bid;
                best_v = v;
            }
        }
        if best_v == usize::MAX {
            continue;
        }
        let travel = travel_time(positions[best_v], task.pos_ned_m);
        queues[best_v] += travel + task.hover_s;
        positions[best_v] = task.pos_ned_m;
        result.per_vehicle[best_v].push(ti);
    }
    result
}

/// Re-run the auction over a pooled subset of tasks, restricted to
/// vehicles that can still accept work (spec §6.5). Already-queued,
/// un-started tasks of a faulted/returning vehicle are pooled by the
/// caller; the active task is never pooled.
pub fn reallocate(
    pooled_tasks: &[usize],
    tasks: &[Task],
    vehicles: &[BidVehicle],
) -> Assignment {
    let subset: Vec<Task> = pooled_tasks.iter().map(|&i| tasks[i].clone()).collect();
    let sub_assignment = auction(&subset, vehicles);
    // Map back to global task indices.
    let mut result = Assignment::empty(vehicles.len());
    for (v, queue) in sub_assignment.per_vehicle.iter().enumerate() {
        for &local_i in queue {
            result.per_vehicle[v].push(pooled_tasks[local_i]);
        }
    }
    result
}

/// The dismissed baseline (spec §6.4): each task, in deadline order, to
/// the nearest vehicle from its current position; positions advance.
pub fn greedy_nearest(tasks: &[Task], vehicles: &[BidVehicle]) -> Assignment {
    let mut order: Vec<usize> = (0..tasks.len()).collect();
    order.sort_by(|&a, &b| {
        tasks[a]
            .deadline_s
            .partial_cmp(&tasks[b].deadline_s)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(tasks[b].reward.partial_cmp(&tasks[a].reward).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut positions: Vec<[f32; 2]> = vehicles.iter().map(|v| v.position).collect();
    let mut result = Assignment::empty(vehicles.len());
    for &ti in &order {
        let task = &tasks[ti];
        let mut best_v = 0;
        let mut best_d = f32::INFINITY;
        for v in 0..vehicles.len() {
            let d = travel_time(positions[v], task.pos_ned_m);
            if d < best_d || (d == best_d && v < best_v) {
                best_d = d;
                best_v = v;
            }
        }
        positions[best_v] = task.pos_ned_m;
        result.per_vehicle[best_v].push(ti);
    }
    result
}

// ---------------------------------------------------------------------------
// Hungarian algorithm (the optimality baseline, spec §6.4)
// ---------------------------------------------------------------------------

/// Minimum-cost assignment for an `n x m` cost matrix with `n <= m`: every
/// row is assigned to a distinct column, minimising the total cost. For
/// `n > m` the caller transposes (returns col-of-row).
///
/// Classic O(n^3) shortest-augmenting-path implementation (Jonker-
/// Volgenant style with potentials), unit-tested against brute-force
/// permutations for n up to 6 (spec §11.1).
pub fn hungarian(cost: &[Vec<f64>]) -> Vec<usize> {
    let n = cost.len();
    if n == 0 {
        return Vec::new();
    }
    let m = cost[0].len();
    assert!(n <= m, "hungarian: require rows <= cols; transpose instead");
    // 1-indexed potentials/assignment (e-maxx formulation).
    let inf = f64::INFINITY;
    let mut u = vec![0.0f64; n + 1];
    let mut v = vec![0.0f64; m + 1];
    let mut p = vec![0usize; m + 1]; // p[j] = row assigned to column j
    let mut way = vec![0usize; m + 1];

    for i in 1..=n {
        p[0] = i;
        let mut j0 = 0usize;
        let mut minv = vec![inf; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = inf;
            let mut j1 = usize::MAX;
            for j in 1..=m {
                if !used[j] {
                    let cur = cost[i0 - 1][j - 1] - u[i0] - v[j];
                    if cur < minv[j] {
                        minv[j] = cur;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }

    let mut ans = vec![usize::MAX; n];
    for j in 1..=m {
        if p[j] != 0 {
            ans[p[j] - 1] = j - 1;
        }
    }
    ans
}

/// The Hungarian baseline for the fleet's linear static problem: every
/// task assigned to a vehicle, vehicles reusable (capacity-free), cost =
/// travel time from the vehicle's auction-time position. The optimum of
/// this relaxation is the per-task minimum; `hungarian_static_optimum`
/// returns that total (used as the §6.4 comparison baseline), with the
/// assignment itself available via `static_argmin`.
pub fn static_argmin(tasks: &[Task], vehicles: &[BidVehicle]) -> Assignment {
    let mut result = Assignment::empty(vehicles.len());
    for ti in 0..tasks.len() {
        let mut best_v = 0;
        let mut best_c = f32::INFINITY;
        for v in 0..vehicles.len() {
            let c = travel_time(vehicles[v].position, tasks[ti].pos_ned_m);
            if c < best_c || (c == best_c && v < best_v) {
                best_c = c;
                best_v = v;
            }
        }
        if !vehicles.is_empty() {
            result.per_vehicle[best_v].push(ti);
        }
    }
    result
}

pub fn hungarian_static_optimum(tasks: &[Task], vehicles: &[BidVehicle]) -> f64 {
    static_argmin(tasks, vehicles)
        .per_vehicle
        .iter()
        .enumerate()
        .map(|(v, q)| q.iter().map(|&ti| travel_time(vehicles[v].position, tasks[ti].pos_ned_m) as f64).sum::<f64>())
        .sum()
}

/// Total travel time of an assignment computed statically (from the
/// vehicles' auction-time positions).
pub fn static_cost(a: &Assignment, tasks: &[Task], vehicles: &[BidVehicle]) -> f64 {
    a.per_vehicle
        .iter()
        .enumerate()
        .map(|(v, q)| q.iter().map(|&ti| travel_time(vehicles[v].position, tasks[ti].pos_ned_m) as f64).sum::<f64>())
        .sum()
}

/// Total travel time of executing the assignment: positions advance
/// along each vehicle's queue ("queue effects" — what the fleet actually
/// flies).
pub fn realized_cost(a: &Assignment, tasks: &[Task], vehicles: &[BidVehicle]) -> f64 {
    let mut total = 0.0f64;
    for (v, queue) in a.per_vehicle.iter().enumerate() {
        let mut pos = vehicles[v].position;
        for &ti in queue {
            total += travel_time(pos, tasks[ti].pos_ned_m) as f64;
            pos = tasks[ti].pos_ned_m;
        }
    }
    total
}

/// Makespan (max per-vehicle finish time including hover) — the load
/// balance metric that motivates the queue term.
pub fn makespan(a: &Assignment, tasks: &[Task], vehicles: &[BidVehicle]) -> f64 {
    let mut mk = 0.0f64;
    for (v, queue) in a.per_vehicle.iter().enumerate() {
        let mut pos = vehicles[v].position;
        let mut t = 0.0f64;
        for &ti in queue {
            t += travel_time(pos, tasks[ti].pos_ned_m) as f64 + tasks[ti].hover_s as f64;
            pos = tasks[ti].pos_ned_m;
        }
        mk = mk.max(t);
    }
    mk
}

// ---------------------------------------------------------------------------
// Deterministic seeded PRNG for statistics tests (no external deps)
// ---------------------------------------------------------------------------

/// xorshift64* — deterministic, good enough for uniform sampling in tests.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    pub fn uniform(&mut self, lo: f32, hi: f32) -> f32 {
        let u = (self.next_u64() >> 11) as f32 / (1u64 << 53) as f32;
        lo + u * (hi - lo)
    }

    pub fn uniform_int(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next_u64() % ((hi - lo + 1) as u64)) as u32
    }
}

#[cfg(test)]
mod tests;
