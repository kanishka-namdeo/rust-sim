# ADR 0010: DO_SET_MODE parameter layout (V-11 empirical resolution)

**Status**: Accepted (v0.1)
**Context**: `LinkHandle::set_mode` sent PX4 custom modes as the packed 32-bit
word (`(sub_mode << 24) | (main_mode << 16)`) in COMMAND_LONG param2 — the same
layout PX4 uses in HEARTBEAT `custom_mode`, and the layout `fleet-modes`
implements. Symptom: every DO_SET_MODE was ACKed `MAV_RESULT_ACCEPTED` while
the vehicle's `nav_state_user_intention` never changed — a silent no-op.

**Root cause** (PX4 v1.16.2 source, `Commander.cpp:787-790`):

```cpp
case vehicle_command_s::VEHICLE_CMD_DO_SET_MODE: {
        uint8_t base_mode = (uint8_t)cmd.param1;
        uint8_t custom_main_mode = (uint8_t)cmd.param2;   // <- uint8 cast
        uint8_t custom_sub_mode = (uint8_t)cmd.param3;    // <- uint8 cast
```

PX4 decodes DO_SET_MODE as **separate small integers** (param1 = base mode
flags, param2 = main mode, param3 = sub mode). The packed word (0x00060000)
truncates to 0 in the `uint8_t` cast, matches no mode branch, and the command
still ACKs.

**Decision**: `set_mode(mode_word)` decomposes the word —
`main = (word >> 16) & 0xFF`, `sub = (word >> 24) & 0xFF` — and sends
`[CUSTOM_ENABLED, main, sub, 0, 0, 0, 0]`. Callers and the heartbeat-echo
comparisons keep using the packed word (HEARTBEAT `custom_mode` **does** use
the packed layout).

**Verification**: after the fix, the manager observed the mode echo
`OFFBOARD (custom_mode=0x00060000)` on real PX4 (V-11 empirical confirmation,
event log). A cautionary note for the spec: mode changes previously attributed
to the manager's AUTO.RTL command were in fact PX4-internal failsafe RTLs
(datalink loss) — the pre-fix command was a no-op.

**Note on the engage ladder**: `UserModeIntention::change` gates mode changes
while ARMED on `health_and_arming_checks.canRun(target)`, which for OFFBOARD
requires the setpoint stream fresh (`offboard_control_mode` within
COM_OF_LOSS_T) and the local position estimate valid. Entry attempts before
EKF2 alignment are converted to a LOITER fallback; the engage sequence
(stream → arm → set_mode, §3.3) with retries is therefore load-bearing, not
ceremonial.
