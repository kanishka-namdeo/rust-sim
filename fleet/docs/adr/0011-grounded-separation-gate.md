# ADR 0011: Separation AltitudeDiverge must not fire for grounded vehicles

**Status**: Accepted (2026-09-07, discovered during the live F-2 bring-up).
**Note (2026-09-10 cleanup)**: Policy 7 (separation, rank 7) and the
whole 8-policy safety ladder were removed end-to-end in the 2026-09-10
cleanup (Tasks 7a/7b/7c-finish). `fleet-safety` is now geofence-only
(passive `GEOFENCE_WARN` flag). The F-2 harness was removed with the
auction-flown mission. The transit altitude layering advisory
(`10 + 5k m` AGL) is kept in the docs as the deconfliction hint, but
there is no longer a runtime AltitudeDiverge override enforcing it.
This ADR is kept as the historical record of the bug + its fix; the
fix's regression test (`grounded_colocated_spawn_gets_no_altitude_override`)
was removed with the policy module. The NED dz convention
(climb = negative) documentation is still useful for the operator
reading FleetFrame telemetry.
**Context**: F-2 (mavfleet driving per-vehicle rustsitsim instances) initially produced a
fleet that armed, echoed OFFBOARD, and never left the ground: motor outputs pinned at
armed-idle, task goals never advanced horizontally, and PX4 auto-disarmed after 10 s
("Disarmed by auto preflight disarming"). The ULog for the failed run showed
`trajectory_setpoint` marching to z = +10 (NED **down**) for a -10 m target with x/y frozen
at the spawn anchor — while every Rust layer (runner goal, link pump, wire codec) was
verified NED-correct by a dedicated wire repro test
(`fleet-mavlink/tests/wire_goal_repro.rs`).

**Root cause**: both vehicles spawn co-located at NED (0,0,0) — PX4 derives its local
origin from the first GPS fix, which is each vehicle's own origin, so the manager's
per-vehicle local frames coincide numerically. Policy 7 (separation, rank 7) then fired on
EVERY 10 Hz tick for both ACTIVE vehicles, and its AltitudeDiverge override replaced the
runner's mission goal with `current[xy] frozen, current_z +/- 3 m` — a goal the pump then
chased at cruise speed. The z feedback loop (target = current + dz, recomputed every tick)
random-walked the commanded z to +10 while armed-but-grounded, permanently starving the
real mission goals. The manager-side `if snap.armed` gate (added for the interim sim) was
insufficient: armed != airborne.

**Decision**: policy 7 emits AltitudeDiverge only when BOTH vehicles are airborne
(`est z < AIRBORNE_Z_M = -1.0 m` NED). Grounded conflicts (spawn co-location, taxi, landed)
are logged as notes but never override the goal stream — a grounded vehicle cannot comply
with a vertical separation command anyway. The NED dz semantics are unchanged (climb =
negative; the lower-altitude vehicle climbs, the higher dives) and are now documented on
the constant; the stale "vehicle 0 is ABOVE" comment in the test was corrected (z=-10 is
BELOW z=-10.5 in NED).

**Consequences**:
- Co-located spawns fly their missions normally; transit altitude layering (10 + 5k m)
  separates the vehicles vertically within seconds of takeoff, and task geometry separates
  them horizontally.
- Real airborne conflicts still trigger the override (pinned by the updated
  `grounded_colocated_spawn_gets_no_altitude_override` regression test).
- Related F-2 fixes in the same bring-up: `LAND_TIMEOUT_MS` 30 s -> 120 s (the interim-sim
  value declared vehicles "LANDED" while ~20 m up: a real RTL cycle is climb-to-return-alt
  ~30 s + return + descend at ~0.7 m/s + touchdown + auto-disarm, measured 60-90 s), and
  the harness READY gate now accepts any post-READY state (the 2/2 READY snapshot window
  is <200 ms wide because both vehicles flip READY->ACTIVE together).
