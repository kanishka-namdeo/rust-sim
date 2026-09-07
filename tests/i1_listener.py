#!/usr/bin/env python3
"""I-1 telemetry oracle: bind PX4's onboard remote endpoint (14540+i) and
count the boot-evidence message classes (SPEC §10.3 Case I-1):
  - HEARTBEAT (expected sysid = i+1)
  - ESTIMATOR_STATUS / ATTITUDE / LOCAL_POSITION_NED (estimator flowing)
Prints a JSON summary on stdout and exits 0. Run with python3.13 (pymavlink).

Usage: i1_listener.py <udp_port> <seconds> <expected_sysid>
"""
import json
import sys
import time

from pymavlink import mavutil


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 14540
    seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 20.0
    want_sysid = int(sys.argv[3]) if len(sys.argv) > 3 else 1

    # Bind the remote endpoint: PX4 streams to whoever holds it (OBS).
    mav = mavutil.mavlink_connection(f"0.0.0.0:{port}")

    counts = {}
    heartbeat_sysid = None
    t0 = time.time()
    while time.time() - t0 < seconds:
        m = mav.recv_msg()
        if m is None:
            time.sleep(0.005)
            continue
        t = m.get_type()
        counts[t] = counts.get(t, 0) + 1
        if t == "HEARTBEAT" and heartbeat_sysid is None:
            heartbeat_sysid = m.get_srcSystem()

    mav.close()
    print(
        json.dumps(
            {
                "seconds": round(time.time() - t0, 1),
                "heartbeat_sysid": heartbeat_sysid,
                "counts": dict(sorted(counts.items())),
                "estimator_status": counts.get("ESTIMATOR_STATUS", 0),
                "attitude": counts.get("ATTITUDE", 0),
                "attitude_quaternion": counts.get("ATTITUDE_QUATERNION", 0),
                "local_position_ned": counts.get("LOCAL_POSITION_NED", 0),
                "global_position_int": counts.get("GLOBAL_POSITION_INT", 0),
                "heartbeat": counts.get("HEARTBEAT", 0),
                "sysid_ok": heartbeat_sysid == want_sysid,
            }
        )
    )


if __name__ == "__main__":
    main()
