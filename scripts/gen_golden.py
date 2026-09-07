#!/usr/bin/env python3
"""Regenerate the MAVLink v2 golden vectors used by fleet-mavlink tests.

The fixtures pin the wire contract (field order, CRC extra bytes, trailing
zero trimming, framing) against pymavlink — the reference decoder, and the
same bytes PX4's generated C headers produce. Requires pymavlink; run:

    python3 scripts/gen_golden.py > crates/fleet-mavlink/tests/golden_mavlink.json
"""
import json
from pymavlink.dialects.v20 import common as c


class Cap:
    def __init__(self):
        self.buf = b""

    def write(self, b):
        self.buf += bytes(b)


POS_ONLY = (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << 11)
VEL_YAWRATE = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << 10)

vectors = []


def gen(name, fn, sysid=255, compid=190):
    cap = Cap()
    m = c.MAVLink(cap, srcSystem=sysid, srcComponent=compid)
    m.mavlink_10 = False  # MAVLink v2 framing
    fn(m)
    assert len(cap.buf) > 0, name
    vectors.append({"name": name, "sysid": sysid, "compid": compid, "hex": cap.buf.hex()})


gen("HEARTBEAT_gcs", lambda m: m.heartbeat_send(6, 8, 0, 0, 4, 3))
gen("COMMAND_LONG_setmsginterval", lambda m: m.command_long_send(1, 1, 511, 0, 32, 20000.0, 0.0, 0.0, 0.0, 0.0, 0.0))
gen("COMMAND_LONG_domode_offboard", lambda m: m.command_long_send(1, 1, 176, 0, 1.0, float(6 << 16), 0.0, 0.0, 0.0, 0.0, 0.0))
gen("COMMAND_LONG_domode_rtl", lambda m: m.command_long_send(1, 1, 176, 0, 1.0, float((4 << 16) | (5 << 24)), 0.0, 0.0, 0.0, 0.0, 0.0))
gen("COMMAND_LONG_domode_land", lambda m: m.command_long_send(1, 1, 176, 0, 1.0, float((4 << 16) | (6 << 24)), 0.0, 0.0, 0.0, 0.0, 0.0))
gen("COMMAND_LONG_arm", lambda m: m.command_long_send(1, 1, 400, 0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0))
gen("COMMAND_LONG_takeoff", lambda m: m.command_long_send(1, 1, 22, 0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 12.0))
gen("COMMAND_LONG_rtl", lambda m: m.command_long_send(1, 1, 20, 0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0))
gen("COMMAND_LONG_land", lambda m: m.command_long_send(1, 1, 21, 0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0))
gen("COMMAND_LONG_disarm", lambda m: m.command_long_send(1, 1, 400, 0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0))
gen("SET_POSITION_TARGET_LOCAL_NED_position", lambda m: m.set_position_target_local_ned_send(
    12345, 1, 1, 1, POS_ONLY, 10.0, -20.0, -30.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.5, 0.0))
gen("SET_POSITION_TARGET_LOCAL_NED_velocity", lambda m: m.set_position_target_local_ned_send(
    67890, 2, 1, 1, VEL_YAWRATE, 0.0, 0.0, 0.0, 1.0, 2.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.25))
gen("STATUSTEXT", lambda m: m.statustext_send(4, b"hello fleet\0" + b"\0" * 39))
gen("PING", lambda m: m.ping_send(1234567890, 0, 0, 0))
gen("HEARTBEAT_px4", lambda m: m.heartbeat_send(2, 12, 209, (4 << 16) | (3 << 24), 4, 3), sysid=2, compid=1)  # AUTO.LOITER, header layout
gen("ATTITUDE", lambda m: m.attitude_send(1000, 0.1, -0.2, 3.0, 0.01, -0.02, 0.03), sysid=2, compid=1)
gen("LOCAL_POSITION_NED", lambda m: m.local_position_ned_send(2000, 1.5, -2.5, -10.0, 0.1, 0.2, -0.3), sysid=2, compid=1)
gen("GLOBAL_POSITION_INT", lambda m: m.global_position_int_send(3000, 473977000, 85455000, 50000, 48000, 10, -20, 30, 18000), sysid=2, compid=1)
gen("SYS_STATUS", lambda m: m.sys_status_send(12345, 12345, 12345, 500, 11800, -1500, 80, 0, 0, 0, 0, 0, 0), sysid=2, compid=1)
gen("BATTERY_STATUS", lambda m: m.battery_status_send(0, 0, 3, 2500, [11800, 0, 0, 0, 0, 0, 0, 0, 0, 0], -1500, 500, 0, 75, 0, 0, [0, 0, 0, 0], 0, 0), sysid=2, compid=1)
gen("HOME_POSITION", lambda m: m.home_position_send(473977000, 85455000, 50000, 0.0, 0.0, 0.0, [1.0, 0, 0, 0], 0.0, 0.0, 0.0, 12345678), sysid=2, compid=1)
gen("COMMAND_ACK", lambda m: m.command_ack_send(176, 0, 255, 0, 255, 190), sysid=2, compid=1)

print(json.dumps(vectors, indent=1))
