# ADR 0011: HIL_ACTUATOR_CONTROLS carries PWM values on PX4 v1.16 (V-3 resolved)

**Status**: Accepted (v0.1)
**Context**: SPEC §3.9 assumed HIL_ACTUATOR_CONTROLS `controls[i]` arrive in
[-1, +1] (jMAVSim-era convention) and mapped `u = (c + 1) / 2`. With that
mapping, arming produced full throttle on all motors and the vehicle
diverged/flipped: the values PX4 v1.16 actually sends are **raw
`actuator_outputs` PWM microseconds** — ~[1000, 2000] when armed, 0 when
disarmed (`SimulatorMavlink::actuator_controls_from_outputs`:
`msg->controls[i] = _actuator_outputs.output[i]`, zeroed when disarmed).

**Decision**: range-disambiguate per channel (see
`sitsim-sdk/src/engine.rs`):

- |c| <= 1.5 → the legacy [-1, +1] convention, u = (c + 1) / 2
- c >= 900 → PWM microseconds, u = (c − 1000) / 1000
- otherwise → 0 (disarmed/invalid)

Disarm is handled by the mode-flag armed bit (rotor stop), not the control
values, so the ambiguous 0 case is covered either way.

**Verification**: with the fix, armed motor outputs map to sane normalized
commands; the vehicle spins motors, enters OFFBOARD, and begins a physical
climb (sim ground truth z reaches −0.25 m before the takeoff transient;
see ADR-0012 for the remaining dynamics issue).

**Protocol doc impact**: `docs/PROTOCOL.md` §9 now documents the dual
convention with the v1.16 source citation.
