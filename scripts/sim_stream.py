#!/usr/bin/env python3
"""Streaming lockstep HIL mini-simulator (rustsitsim prototype), v2.

Protocol facts (empirical + PX4 SimulatorMavlink.cpp source):
  - PX4 connects as TCP client to 127.0.0.1:4560
  - HIL_SENSOR.id is a MAVLink2 EXTENSION field: PX4's boot gate is
    `imu.id == 0`, so the sim MUST speak MAVLink v2 and include id=0
  - PX4 requests HIL_STATE_QUATERNION (115) at 200 Hz via COMMAND_LONG 511
  - sim advances lockstep time via HIL_SENSOR.time_usec
  - once initialized, `simulator_mavlink start` unblocks, rcS continues,
    all modules (sensors, ekf2, commander, mavlink, ...) start
"""
import sys
import time

from pymavlink import mavutil
from pymavlink.dialects.v20 import common as v20common

LISTEN_PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 4560
DURATION = float(sys.argv[2]) if len(sys.argv) > 2 else 35.0

print(f"[*] listening for PX4 TCP connection on 127.0.0.1:{LISTEN_PORT} ...")
mav = mavutil.mavlink_connection(f"tcpin:127.0.0.1:{LISTEN_PORT}")
print("[+] TCP ACCEPTED -> PX4 simulator_mavlink module is connected")

# swap in the v2.0 dialect so extension fields (HIL_SENSOR.id) are sent
mav.mav = v20common.MAVLink(mav, srcSystem=2, srcComponent=1)
mav.mav.mavlink_10 = False  # ensure MAVLink v2 framing

SENSOR_HZ = 200.0
GPS_HZ = 5.0
sim_time_us = 0.0

got = {}
actuator_count = 0
requests = []
gps_time_us = 0.0

start = time.time()
last_sensor = 0.0
last_gps = 0.0
sent_ok = 0
send_err = None

while time.time() - start < DURATION:
    now = time.time()

    m = mav.recv_msg()
    if m is not None:
        t = m.get_type()
        got[t] = got.get(t, 0) + 1
        if t == "HIL_ACTUATOR_CONTROLS":
            actuator_count += 1
        elif t == "COMMAND_LONG":
            requests.append((m.command, round(m.param1, 1), round(m.param2, 1)))
            if m.command == 511:
                try:
                    mav.mav.command_ack_send(511, 0)  # ACCEPTED
                except Exception:
                    pass

    if now - last_sensor >= 1.0 / SENSOR_HZ:
        last_sensor = now
        sim_time_us += 1.0e6 / SENSOR_HZ
        try:
            # HIL_SENSOR with id=0 (extension field, v2 framing)
            mav.mav.hil_sensor_send(
                int(sim_time_us),
                0.0, 0.0, -9.80665,
                0.0, 0.0, 0.0,
                0.0, 0.0, 0.0,
                101325.0, 0.0, 0.0,
                20.0,
                0b011111111,
                0,
            )
            # HIL_STATE_QUATERNION (PX4 explicitly requested this @ 200Hz)
            # NOTE: vx/vy/vz int16 cm/s, airspeeds uint16, acc int16 mG
            mav.mav.hil_state_quaternion_send(
                int(sim_time_us),
                [1.0, 0.0, 0.0, 0.0],   # identity quaternion (w,x,y,z)
                0.0, 0.0, 0.0,          # roll/pitch/yaw speed
                473977000, 85455000,    # lat/lon 1e7 deg
                50000,                  # alt mm
                0, 0, 0,                # vx, vy, vz (cm/s)
                0, 0,                   # airspeeds (cm/s)
                0, 0, -1000,            # acc (mG)
            )
            sent_ok += 1
        except Exception as e:
            send_err = str(e)
            print(f"[!] send error: {e}")
            break

    if now - last_gps >= 1.0 / GPS_HZ:
        last_gps = now
        try:
            mav.mav.hil_gps_send(
                int(sim_time_us),
                3,
                473977000, 85455000,
                50000,
                100, 100,
                0, 0, 0, 0, 0,
                10,
                0, 0,
            )
        except Exception as e:
            print(f"[!] hil_gps_send: {e}")
            break

    time.sleep(0.001)

print(f"[+] HIL_SENSOR+QUAT sent: {sent_ok} (target ~{int(SENSOR_HZ * DURATION)}) err={send_err}")
print(f"[+] messages received from PX4: {dict(sorted(got.items()))}")
for r in requests[:5]:
    print(f"[+] PX4 rate request: cmd={r[0]} msg_id={r[1]} interval_us={r[2]}")
if actuator_count:
    print(f"[+] HIL_ACTUATOR_CONTROLS received ({actuator_count}x) -> FULL HIL LOOP CLOSED")
else:
    print("[!] no actuator controls yet")
