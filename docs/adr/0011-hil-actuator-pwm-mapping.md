# ADR 0011 (revised): HIL_ACTUATOR_CONTROLS carries per-motor NORMALIZED thrust [0, 1] on PX4 v1.16

**Status**: Supersedes ADR 0011 v1 (rejected — see "What v1 got wrong")
**Context**: SPEC §3.9 originally assumed the jMAVSim-era `[-1, +1]`
convention (`u = (c + 1) / 2`). v1 of this ADR "resolved" that to raw PWM
microseconds on the wire, based on reading
`SimulatorMavlink::actuator_controls_from_outputs`, which does
`msg->controls[i] = _actuator_outputs.output[i]` (armed) — but that
subscription is **`actuator_outputs_sim`**, not the PWM `actuator_outputs`
topic (`orb_subscribe_multi(ORB_ID(actuator_outputs_sim), 0)`).

`PWMSim::updateOutputs` publishes `actuator_outputs_sim` with its own
scale (`src/modules/simulation/pwm_out_sim/PWMSim.cpp`):

```cpp
// non-reversible Motor1..MotorMax function outputs:
actuator_outputs.output[i] = (output - PWM_SIM_PWM_MIN_MAGIC /*1000*/)
                           / (PWM_SIM_PWM_MAX_MAGIC /*2000*/ - 1000);   // -> [0, 1]
// everything else: (output - 1500) / 500                                // -> [-1, 1]
// channels at the disarmed magic (900) are SKIPPED and stay 0
```

**Live capture (PX4 v1.16.2, `scripts/wire_sniff.py`, armed + OFFBOARD
climb against real PX4)**:

- `mode = 0x0081` when armed — bit `0x80` is the armed flag, bit `0x01`
  the custom/lockstep flag; disarmed frames arrive as `mode = 0x0001`
  with **all controls = 0**.
- Motors are on `controls[0..3]` (CA_ROTOR0..3); the other 12 stay 0.
- Armed idle: `[0.0000, 0.0020, 0.0000, 0.0020]` (PWM 1000-ish → 0.002).
- Offboard climb: smooth ramp `0.013 → 0.28 → 0.5 → 0.98 → 1.0`
  (saturating against a static vehicle that never climbs).
- Disarm: zeros, armed bit clear.

**Decision** (`sitsim-sdk/src/engine.rs`):

- `u_i = clamp(controls[i], 0, 1)` for the [0, 1] v1.16 motor scale.
- `c >= 900` → PWM fallback, `u = (c − 1000) / 1000` (robustness for
  stacks that send raw PWM).
- **The armed bit (`mode & 0x80`) gates the rotors**: disarmed means
  `motor_stopped` — the rotors wind down through the first-order lag —
  not an idle spin.
- The `(c + 1) / 2` branch is REMOVED: no v1.16 code path emits it, and
  it is actively harmful (see below).

**What v1 of this ADR got wrong, and what it cost (I-2)**: the (c+1)/2
fallback fired on every v1.16 frame (all wire values are ≤ 1), turning
armed idle `0.002` into `u = 0.5` — a phantom 2/3-of-hover thrust on
"stopped" motors. The vehicle sat pinned on the ground with 475 rad/s
rotors, PX4's takeoff differential then pushed against the contacts until
the closed loop diverged (rates growing ~e^7/s from t ≈ 15.6 s, NaN,
frozen truth; the F-2 `vehicle_0.replay` signature was
`motors = [128, 128, 128, 199]` bytes = wire `[0, 0, 0, 0.56]` mapped to
`u = [0.5, 0.5, 0.5, 0.78]`). The wire scale was also cross-verified
against the ULog: replay wire values regress to
`0.876 × (pwm − 1000)/1000 − 0.006` ≈ identity.

**Verification**: engine regression
`engine::actuator_mapping_tests::v1_16_wire_convention_maps_directly`
(armed idle 0.002 → 0.002, PWM branch, disarm wind-down); live re-run of
I-2 after this change (see `tests/run_i2_flight.sh`).

**Protocol doc impact**: `docs/PROTOCOL.md` §3.9/§9 document the [0, 1]
scale, the armed bit, and the PWM fallback.
