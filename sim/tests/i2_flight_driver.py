#!/usr/bin/env python3
"""I-2 flight driver: GCS-side MAVLink driver for the real-PX4 flight.

Flow (all against PX4's onboard UDP link 14540):
  1. 1 Hz GCS heartbeat pump (required for arming — ADR-0011r note)
  2. wait for EKF convergence (~18 s)
  3. ARM (retry ladder, up to 5 attempts)
  4. stream SET_POSITION_TARGET_LOCAL_NED (pos-only) at >= 10 Hz
  5. DO_SET_MODE OFFBOARD (COMMAND_LONG 176: param1=1, param2=6 — the
     decomposed PX4 layout, ADR-0010)
  6. climb to z=-2.5 m, hold ~12 s, descend to z=-0.3, hold, then land
     (AUTO_LAND = param2=4 (AUTO) + param3=6 (AUTO_LAND) — sub-mode 6,
     px4_custom_mode.h), disarm
  7. poll the sim's ground truth from the replay record file (the sim
     flushes it every 200 ticks = 1 s; /api/status carries no position —
     position lives only on the WS telemetry plane)
  8. write a JSON result consumed by run_i2_flight.sh

v1.16 facts baked in (verified against PX4-Autopilot v1.16.2 source):
  - heartbeat custom_mode = packed px4_custom_mode union: main_mode in
    bits 16..23, sub_mode in bits 24..31 (HEARTBEAT.hpp:104) — so the
    OFFBOARD echo is main==6, NOT nav_state 14.
  - COM_OF_LOSS_T default = 1.0 s: offboard setpoints must beat 1 Hz or
    the commander failsafes out of OFFBOARD (the original plan loop
    blocked ~2 s per iteration in a message drain and starved the stream).
  - DO_SET_MODE: param2=main, param3=sub (Commander.cpp:788-790).
"""
import json
import math
import os
import struct
import sys
import threading
import time

from pymavlink import mavutil

GCS_PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 14540
API_PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 8200
RESULT = sys.argv[3] if len(sys.argv) > 3 else "/home/z/my-project/rustsitsim/tests/i2_artifacts/driver_result.json"
TOTAL_S = float(sys.argv[4]) if len(sys.argv) > 4 else 95.0
REPLAY = os.path.join(os.path.dirname(os.path.abspath(RESULT)), "i2_flight.replay")

RECORD_LEN = 96
HEADER_LEN = 64

# PX4 packed custom-mode fields (px4_custom_mode.h).
MAIN_OFFBOARD = 6      # PX4_CUSTOM_MAIN_MODE_OFFBOARD
MAIN_AUTO = 4          # PX4_CUSTOM_MAIN_MODE_AUTO
SUB_AUTO_LAND = 6      # PX4_CUSTOM_SUB_MODE_AUTO_LAND

stop = threading.Event()
res = {
    "heartbeat": False,
    "armed": False,
    "offboard_mode_seen": False,
    "land_mode_seen": False,
    "disarmed": False,
    "statustexts": [],
    "truth_z_min": 0.0,       # sim ground truth: most negative z (highest)
    "truth_z_final": None,
    "truth_max_speed": 0.0,
    "truth_ticks": 0,
    "ekf_z_min": 0.0,
    "ekf_samples": 0,
    "arm_s": None,            # seconds from driver start to ARM ack
    "offboard_first_s": None, # seconds from driver start to first OFFBOARD echo
    "offboard_reengages": 0,
    "errors": [],
}

# Latest heartbeat-decoded state (packed custom_mode union + armed bit).
mode_state = {"main": 0, "sub": 0, "armed": False}
last_mode_cmd = [0.0]


def hb_pump(m):
    while not stop.is_set():
        try:
            m.mav.heartbeat_send(6, 8, 0, 0, 0, 3)
        except Exception as e:
            res["errors"].append(f"hb: {e}")
            return
        time.sleep(1.0)


def truth_poll():
    """Ground-truth poll: read the sim's replay record file incrementally.

    The replay (SPEC §8.2) is 64-byte header + 96-byte tick records:
    [8..24] motors u8, [24..92] 17 state floats pos(3) vel(3) q(4)
    omega(3) rotors(4). The writer flushes every 200 records (1 s at
    200 Hz), so this is near-live ground truth (<= 1 s lag).
    """
    off = HEADER_LEN
    while not stop.is_set():
        try:
            with open(REPLAY, "rb") as f:
                f.seek(off)
                data = f.read()
            if data:
                n = len(data) // RECORD_LEN
                for i in range(n):
                    s = struct.unpack_from("<17f", data, i * RECORD_LEN + 24)
                    z, vx, vy, vz = s[2], s[3], s[4], s[5]
                    if not all(math.isfinite(v) for v in (z, vx, vy, vz)):
                        continue  # torn tail record
                    res["truth_z_min"] = min(res["truth_z_min"], z)
                    res["truth_z_final"] = z
                    res["truth_max_speed"] = max(
                        res["truth_max_speed"], abs(vx), abs(vy), abs(vz))
                    res["truth_ticks"] += 1
                off += n * RECORD_LEN
        except Exception:
            pass
        time.sleep(0.2)


def setpoint(m, z, xy=(0.0, 0.0)):
    m.mav.set_position_target_local_ned_send(
        int(time.monotonic() * 1000) & 0xFFFFFFFF,
        m.target_system, m.target_component,
        1,                       # MAV_FRAME_LOCAL_NED
        0b0000111111111000,     # pos-only
        xy[0], xy[1], z, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)


def track_msg(msg):
    t = msg.get_type()
    if t == "STATUSTEXT":
        res["statustexts"].append(msg.text[-120:])
        if len(res["statustexts"]) > 200:
            res["statustexts"] = res["statustexts"][-100:]
    elif t == "HEARTBEAT":
        cm = int(msg.custom_mode)
        mode_state["main"] = (cm >> 16) & 0xFF
        mode_state["sub"] = (cm >> 24) & 0xFF
        mode_state["armed"] = bool(msg.base_mode & 128)
        if mode_state["main"] == MAIN_OFFBOARD:
            res["offboard_mode_seen"] = True
            if res["offboard_first_s"] is None:
                res["offboard_first_s"] = time.time() - T0[0]
        if mode_state["main"] == MAIN_AUTO and mode_state["sub"] == SUB_AUTO_LAND:
            res["land_mode_seen"] = True
        if res["armed"] and not mode_state["armed"]:
            res["disarmed"] = True
    elif t == "LOCAL_POSITION_NED":
        res["ekf_z_min"] = min(res["ekf_z_min"], float(msg.z))
        res["ekf_samples"] += 1


T0 = [time.time()]


def drain(m, seconds):
    t0 = time.time()
    while time.time() - t0 < seconds:
        msg = m.recv_match(blocking=True, timeout=0.05)
        if msg is not None:
            track_msg(msg)


def send_mode(m, main, sub=0):
    """DO_SET_MODE with the v1.16 decomposed layout: param1=1 (custom),
    param2=main mode, param3=sub mode (Commander.cpp:787-790)."""
    m.mav.command_long_send(m.target_system, m.target_component, 176, 0,
                            1.0, float(main), float(sub),
                            0.0, 0.0, 0.0, 0.0)
    last_mode_cmd[0] = time.time()


def stream(m, seconds, z, name=""):
    """Stream position setpoints at >= 10 Hz for `seconds` (PX4's
    COM_OF_LOSS_T = 1.0 s failsafe requires > 1 Hz), drain telemetry,
    and re-send the OFFBOARD mode request if the echo drops out."""
    t0 = time.time()
    while time.time() - t0 < seconds and not stop.is_set():
        setpoint(m, z)
        drain(m, 0.1)
        if (res["offboard_mode_seen"] and mode_state["armed"]
                and mode_state["main"] != MAIN_OFFBOARD
                and time.time() - last_mode_cmd[0] > 3.0):
            send_mode(m, MAIN_OFFBOARD)
            res["offboard_reengages"] += 1
            print(f"[i2] offboard re-engage #{res['offboard_reengages']}", flush=True)
        if time.time() - T0[0] > TOTAL_S:
            res["errors"].append("total timeout")
            break
        time.sleep(0.02)
    if name:
        print(f"[i2] phase {name} done (truth z_min={res['truth_z_min']:.2f}, "
              f"main_mode={mode_state['main']}, armed={mode_state['armed']})", flush=True)


def phase_wait(m, seconds, name):
    t0 = time.time()
    while time.time() - t0 < seconds and not stop.is_set():
        drain(m, 0.25)


def arm(m):
    for attempt in range(8):
        m.mav.command_long_send(m.target_system, m.target_component, 400, 0,
                                1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0)
        t0 = time.time()
        ok = None
        while time.time() - t0 < 3.0:
            ack = m.recv_match(type="COMMAND_ACK", blocking=True, timeout=0.5)
            if ack is not None and ack.command == 400:
                ok = int(ack.result)
                break
        print(f"[i2] arm attempt {attempt + 1}: result={ok}", flush=True)
        if ok == 0:
            return True
        # dump denial texts
        drain(m, 1.5)
    return False


def finish(code):
    stop.set()
    res["exit"] = code
    with open(RESULT, "w") as f:
        json.dump(res, f, indent=1)
    print(f"[i2] driver done exit={code} result={json.dumps({k: v for k, v in res.items() if k != 'statustexts'})}", flush=True)


def main():
    m = mavutil.mavlink_connection(f"udp:0.0.0.0:{GCS_PORT}")
    hb = m.recv_match(type="HEARTBEAT", blocking=True, timeout=50)
    if hb is None:
        res["errors"].append("no PX4 heartbeat within 50 s")
        finish(1)
        return
    m.target_system = hb.get_srcSystem()
    m.target_component = hb.get_srcComponent()
    res["heartbeat"] = True
    print(f"[i2] heartbeat sysid {m.target_system}", flush=True)

    T0[0] = time.time()
    threading.Thread(target=hb_pump, args=(m,), daemon=True).start()
    threading.Thread(target=truth_poll, daemon=True).start()

    # ---- EKF settle window: drain messages for ~18 s.
    phase_wait(m, 28.0, "ekf-settle")

    # ---- ARM (retry ladder, ADR-0008 pattern).
    if not arm(m):
        finish(2)
        return
    res["armed"] = True
    res["arm_s"] = time.time() - T0[0]
    print(f"[i2] ARMED at t={res['arm_s']:.1f}s", flush=True)
    drain(m, 0.5)

    # ---- Offboard: prestream setpoints (fresh offboard_control_mode within
    # COM_OF_LOSS_T), then the mode switch.
    stream(m, 3.0, -2.5, "prestream")
    send_mode(m, MAIN_OFFBOARD)
    print("[i2] DO_SET_MODE OFFBOARD sent", flush=True)

    # ---- Fly: climb to -2.5, hold, descend, land. ~32 s.
    plan = [
        (-2.5, 12.0, "climb+hold"),
        (-1.0, 6.0, "descend-1"),
        (-0.3, 6.0, "descend-2"),
        (0.0, 8.0, "land-hold"),
    ]
    for z, dur, name in plan:
        stream(m, dur, z, name)

    # ---- LAND mode (AUTO_LAND: main=4, sub=6) + disarm.
    send_mode(m, MAIN_AUTO, SUB_AUTO_LAND)
    print("[i2] DO_SET_MODE AUTO_LAND sent", flush=True)
    phase_wait(m, 5.0, "land-mode")
    m.mav.command_long_send(m.target_system, m.target_component, 400, 0,
                            0.0, 21196.0, 0.0, 0.0, 0.0, 0.0, 0.0)  # disarm
    phase_wait(m, 5.0, "post-disarm")
    finish(0)


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        import traceback
        res["errors"].append(f"fatal: {e}")
        traceback.print_exc()
        finish(99)
