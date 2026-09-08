#!/usr/bin/env python3
"""Generate golden vectors for the MAVLink parameter-protocol messages.

Appends PARAM_REQUEST_READ / PARAM_REQUEST_LIST / PARAM_VALUE / PARAM_SET
fixtures to fleet-mavlink's golden_mavlink.json, using pymavlink 2.4.49
(v2.0 common dialect) exactly like the original bring-up generator did.
Frames are v2, seq 0, sysid 255 compid 190 (GCS side) or sysid 1 compid 1
(vehicle side for PARAM_VALUE).

Re-run after changing any param codec: the Rust tests pin these bytes.
"""

import json
import pathlib
import struct
import sys

from pymavlink.dialects.v20 import common as mav


class Sink:
    def write(self, _b):
        pass


def frame_hex(link, msg):
    raw = msg.pack(link, 0)  # seq 0
    return raw.hex()


def main() -> int:
    out_path = pathlib.Path(__file__).parent / "golden_mavlink.json"
    fixtures = json.loads(out_path.read_text())
    # idempotency: drop any prior param fixtures we are about to regenerate
    drop = {
        "PARAM_REQUEST_LIST",
        "PARAM_REQUEST_READ",
        "PARAM_VALUE_echo",
        "PARAM_SET_nav_dll_act",
    }
    fixtures = [f for f in fixtures if f["name"] not in drop]

    link = mav.MAVLink(Sink(), srcSystem=255, srcComponent=190)
    vehicle = mav.MAVLink(Sink(), srcSystem=1, srcComponent=1)

    # PARAM_REQUEST_LIST (21) from the GCS.
    fixtures.append(
        {
            "name": "PARAM_REQUEST_LIST",
            "sysid": 255,
            "compid": 190,
            "hex": frame_hex(link, mav.MAVLink_param_request_list_message(1, 1)),
        }
    )

    # PARAM_REQUEST_READ (20) from the GCS: by id (index -1).
    fixtures.append(
        {
            "name": "PARAM_REQUEST_READ",
            "sysid": 255,
            "compid": 190,
            "hex": frame_hex(link, mav.MAVLink_param_request_read_message(1, 1, b"BAT_N_CELLS", -1)),
        }
    )

    # PARAM_VALUE (22) from the vehicle: a mid-download echo with
    # count/index set (non-trivial values so offset errors cannot pass).
    fixtures.append(
        {
            "name": "PARAM_VALUE_echo",
            "sysid": 1,
            "compid": 1,
            "hex": frame_hex(
                vehicle,
                mav.MAVLink_param_value_message(b"NAV_DLL_ACT", 0.0, 9, 722, 101),
            ),
        }
    )

    # PARAM_SET (23) from the GCS: the manager's NAV_DLL_ACT=0 write.
    fixtures.append(
        {
            "name": "PARAM_SET_nav_dll_act",
            "sysid": 255,
            "compid": 190,
            "hex": frame_hex(
                link,
                mav.MAVLink_param_set_message(1, 1, b"NAV_DLL_ACT", 0.0, 9),
            ),
        }
    )

    out_path.write_text(json.dumps(fixtures, indent=1) + "\n")
    print(f"wrote {len(fixtures)} fixtures to {out_path}")
    for f in fixtures[-4:]:
        print(" ", f["name"], f["hex"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
