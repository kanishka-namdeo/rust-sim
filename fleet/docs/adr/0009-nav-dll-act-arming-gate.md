# ADR 0009: Manager writes NAV_DLL_ACT=0 at vehicle bring-up

**Status**: Accepted (v0.1). Note: the "8-policy ladder" referenced
below was removed in the 2026-09-10 cleanup; `fleet-safety` is now
geofence-only (passive `GEOFENCE_WARN` flag, no RTL/LAND escalation).
The NAV_DLL_ACT=0 write at bring-up is still current — PX4's arming
gate check is unchanged and the manager is still the datalink
supervisor.
**Context**: PX4 v1.16's arming gate (`HealthAndArmingChecks/checks/rcAndDataLinkCheck.cpp:81`)
denies arming when `gcs_connection_lost && NAV_DLL_ACT > 0` — "No connection to the ground
control station". Our fleet topology intentionally has no GCS on the 18570+i link: the
manager *is* the datalink supervisor, and its §8 policy ladder (heartbeat-loss RTL/LAND
with a 3 s / 13 s escalation, exercised by the scenario `link_loss` event — **removed
2026-09-10**) implemented the datalink failsafe at the fleet level. The manager still
supervises the link (the heartbeat health aggregation in `fleet-core::health` is kept)
but no longer escalates to RTL/LAND on link loss — the operator is the failsafe now.

**Decision**: when a vehicle reaches READY, the manager writes `NAV_DLL_ACT = 0` via
MAVLink PARAM_SET (REAL32), confirmed by the PARAM_VALUE echo, and logs the confirmation
or the honest failure as an event. The write happens once per vehicle per run. This
disables only PX4's *datalink-loss* failsafe trigger; every other PX4 failsafe (geofence
via PX4's own nav... EKF, battery, attitude) stays armed, and the manager's ladder covers
link loss.

**Consequences**:
- Arming works without a GCS; the arming-gate check becomes info-level in PX4's event
  stream, which is the correct configuration for supervised autonomy stacks.
- A vehicle used *without* a manager (standalone rustsitsim testing) keeps PX4 defaults —
  the param write is a manager action, not a sim-side scenario default.
- fleet-mavlink grows a minimal param plane: PARAM_SET encode + PARAM_VALUE decode with a
  600 ms × 3 retry ladder, mirroring the §3.2 command semantics.
