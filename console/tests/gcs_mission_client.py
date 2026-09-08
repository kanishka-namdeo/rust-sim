#!/usr/bin/env python3
"""GCS-side MAVLink mission client used by the G-3 and G-4 harnesses.

Drives the mission protocol against ``mock_px4_mission.py`` from the GCS
side. Binds UDP 127.0.0.1:14540 (matching the Rust ``LinkConfig::for_instance``
bind port) and sends to 127.0.0.1:14580 (the mock PX4's onboard listen port).

The wire format and message layouts match ``fleet-mavlink/src/messages.rs``
(the Rust codec): MISSION_COUNT carries ``count|target_system|target_component|
mission_type`` (5 bytes), MISSION_ITEM_INT carries 38 bytes, MISSION_REQUEST
is 4 bytes, MISSION_ACK is 4 bytes — all with the CRC extras listed in the
spec (43→132, 44→221, 47→153, 51→226, 73→38, 40→228).

Subcommands:
  upload   Run G-3: upload 3 missions (mission/fence/rally) + a rollback probe.
  download Run G-4: download the 3-waypoint mission the mock was preloaded with.

Exit code 0 on success, non-zero on failure. Prints a JSON summary line on
stdout that the bash harness consumes with ``jq``.
"""
from __future__ import annotations

import argparse
import json
import socket
import sys
import time
from collections import defaultdict, deque
from typing import Deque, Dict, List, Optional, Tuple

from pymavlink.dialects.v20 import common as mb

GCS_SYSID = 255
GCS_COMPID = 190
PX4_SYSID = 1
PX4_COMPID = 1

# Mission types (mirror messages.rs::enums).
MISSION = mb.MAV_MISSION_TYPE_MISSION     # 0
FENCE = mb.MAV_MISSION_TYPE_FENCE         # 1
RALLY = mb.MAV_MISSION_TYPE_RALLY         # 2

MISSION_ACCEPTED = mb.MAV_MISSION_ACCEPTED                  # 0
MISSION_UNSUPPORTED = mb.MAV_MISSION_UNSUPPORTED            # 3

# Three probe missions, one per MAV_MISSION_TYPE. Coordinates are scaled-int
# (lat/lon × 1e7) as the wire format requires. Each mission has 3 items so
# the G-3 assertion can simply check items_acked == 3 per type.
SAMPLE_ITEMS: List[dict] = [
    # Mission (type 0): 3 NAV_WAYPOINT (cmd 16) items, frame=3 (MAV_FRAME_GLOBAL_RELATIVE_ALT)
    {"mission_type": MISSION, "seq": 0, "command": 16, "frame": 3,
     "x": 374133000, "y": -1221017000, "z": 12.0,
     "param1": 0.5, "param2": 2.0, "param3": 0.0, "param4": 0.0,
     "current": 0, "autocontinue": 1},
    {"mission_type": MISSION, "seq": 1, "command": 16, "frame": 3,
     "x": 374137000, "y": -1221013000, "z": 15.0,
     "param1": 0.5, "param2": 2.0, "param3": 0.0, "param4": 0.0,
     "current": 0, "autocontinue": 1},
    {"mission_type": MISSION, "seq": 2, "command": 16, "frame": 3,
     "x": 374140000, "y": -1221010000, "z": 18.0,
     "param1": 0.5, "param2": 2.0, "param3": 0.0, "param4": 0.0,
     "current": 0, "autocontinue": 1},
    # Geofence (type 1): 3 NAV_FENCE_RETURN_POINT (cmd 5000) / fence vertices
    # (cmd 5001) — PX4 uses 5000 once + N vertices; we use 3 vertices for the
    # round-trip probe.
    {"mission_type": FENCE, "seq": 0, "command": 5000, "frame": 3,
     "x": 374130000, "y": -1221020000, "z": 0.0,
     "param1": 0.0, "param2": 0.0, "param3": 0.0, "param4": 0.0,
     "current": 0, "autocontinue": 1},
    {"mission_type": FENCE, "seq": 1, "command": 5001, "frame": 3,
     "x": 374140000, "y": -1221020000, "z": 0.0,
     "param1": 0.0, "param2": 0.0, "param3": 0.0, "param4": 0.0,
     "current": 0, "autocontinue": 1},
    {"mission_type": FENCE, "seq": 2, "command": 5001, "frame": 3,
     "x": 374140000, "y": -1221010000, "z": 0.0,
     "param1": 0.0, "param2": 0.0, "param3": 0.0, "param4": 0.0,
     "current": 0, "autocontinue": 1},
    # Rally (type 2): 3 NAV_RALLY_POINT (cmd 0 / frame=3) items.
    {"mission_type": RALLY, "seq": 0, "command": 0, "frame": 3,
     "x": 374135000, "y": -1221015000, "z": 0.0,
     "param1": 0.0, "param2": 0.0, "param3": 0.0, "param4": 0.0,
     "current": 0, "autocontinue": 1},
    {"mission_type": RALLY, "seq": 1, "command": 0, "frame": 3,
     "x": 374136000, "y": -1221016000, "z": 0.0,
     "param1": 0.0, "param2": 0.0, "param3": 0.0, "param4": 0.0,
     "current": 0, "autocontinue": 1},
    {"mission_type": RALLY, "seq": 2, "command": 0, "frame": 3,
     "x": 374137000, "y": -1221017000, "z": 0.0,
     "param1": 0.0, "param2": 0.0, "param3": 0.0, "param4": 0.0,
     "current": 0, "autocontinue": 1},
]


class GcsClient:
    """Minimal pymavlink-backed GCS that speaks the mission protocol."""

    def __init__(self, bind_addr: Tuple[str, int], px4_addr: Tuple[str, int],
                 timeout: float = 10.0):
        self.px4_addr = px4_addr
        self.timeout = timeout

        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(bind_addr)
        self.sock.settimeout(0.5)

        # Encoder: writes through to the PX4 socket.
        self.out = _UDPOutput(self.sock, px4_addr)
        self.mav = mb.MAVLink(self.out, srcSystem=GCS_SYSID, srcComponent=GCS_COMPID)
        # Decoder: stateless, used only via parse_buffer.
        self._dec = mb.MAVLink(_NullOut(), srcSystem=0, srcComponent=0)

        # Per-type receive queue: drains the socket, dispatches frames into
        # these deques so a frame received while waiting for type A doesn't
        # starve a later wait for type B (the upload loop's "ACK arrived
        # instead of next REQUEST_INT" race).
        self._queues: Dict[str, Deque[object]] = defaultdict(deque)

        self.start_ts = time.time()

    # ------------------------------------------------------------------
    # Low-level wire helpers
    # ------------------------------------------------------------------
    def _send(self, msg) -> None:
        self.mav.send(msg)

    def _drain_socket(self, deadline: float) -> None:
        """Read any pending datagrams and dispatch frames into per-type queues."""
        while time.time() < deadline:
            remaining = deadline - time.time()
            self.sock.settimeout(max(0.01, min(0.5, remaining)))
            try:
                data, _ = self.sock.recvfrom(2048)
            except socket.timeout:
                return
            for msg in self._dec.parse_buffer(data) or []:
                self._queues[msg.get_type()].append(msg)

    def _recv(self, want_type: Optional[str] = None,
              deadline: Optional[float] = None) -> Optional[object]:
        """Block (up to deadline) for a frame of the requested type.

        Other frames received during the wait are buffered in their per-type
        queues so a subsequent ``_recv(other_type)`` returns them immediately.
        When ``want_type is None`` (or empty), returns the next frame of ANY
        type (round-robin across the queues).
        """
        if deadline is None:
            deadline = time.time() + self.timeout
        while time.time() < deadline:
            if want_type is None or want_type == "":
                # Return any frame: round-robin the queues (heartbeats last).
                priority_types = ("MISSION_REQUEST", "MISSION_REQUEST_INT",
                                   "MISSION_ACK", "MISSION_ITEM_INT",
                                   "MISSION_COUNT", "HEARTBEAT")
                for t in priority_types:
                    q = self._queues.get(t)
                    if q:
                        return q.popleft()
                # Fallback: any queue with content.
                for t, q in self._queues.items():
                    if q:
                        return q.popleft()
            else:
                q = self._queues.get(want_type)
                if q:
                    return q.popleft()
            # Poll for new frames (sub-deadline so we don't oversleep).
            self._drain_socket(min(deadline, time.time() + 0.5))
        # One last queue check after the drain loop ended on the deadline.
        if want_type is None or want_type == "":
            for t, q in self._queues.items():
                if q:
                    return q.popleft()
        else:
            q = self._queues.get(want_type)
            if q:
                return q.popleft()
        return None

    def _wait_heartbeat(self, deadline: Optional[float] = None) -> bool:
        return self._recv(want_type="HEARTBEAT", deadline=deadline) is not None

    # ------------------------------------------------------------------
    # Upload (GCS → PX4)
    # ------------------------------------------------------------------
    def upload(self, items: List[dict]) -> dict:
        """Upload a single mission_type. Returns the MISSION_ACK result dict.

        Items must all share the same ``mission_type``.
        """
        mtype = int(items[0]["mission_type"])
        count = len(items)

        # 1. Announce MISSION_COUNT.
        self._send(mb.MAVLink_mission_count_message(
            target_system=PX4_SYSID, target_component=PX4_COMPID,
            count=count, mission_type=mtype,
        ))
        # 2. Loop: each MISSION_REQUEST(_INT)(seq) we receive, reply with the
        # matching MISSION_ITEM_INT(seq). PX4 may re-request the same seq on a
        # dropped packet; tolerate that by tracking which seqs we've sent.
        sent_seqs = set()
        deadline = time.time() + self.timeout
        while time.time() < deadline:
            # The next frame is either a MISSION_REQUEST_INT (PX4 wants another
            # item) or the final MISSION_ACK (transaction done).
            msg = self._recv(deadline=time.time() + self.timeout)
            if msg is None:
                break
            t = msg.get_type()
            if t == "MISSION_ACK":
                return _ack_to_dict(msg)
            if t in ("MISSION_REQUEST", "MISSION_REQUEST_INT"):
                seq = int(msg.seq)
                if seq < 0 or seq >= count:
                    continue  # protocol error — ignore
                item = dict(items[seq])
                self._send(mb.MAVLink_mission_item_int_message(
                    target_system=PX4_SYSID, target_component=PX4_COMPID,
                    seq=seq, frame=int(item["frame"]),
                    command=int(item["command"]),
                    current=int(item.get("current", 0)),
                    autocontinue=int(item.get("autocontinue", 1)),
                    param1=float(item["param1"]), param2=float(item["param2"]),
                    param3=float(item["param3"]), param4=float(item["param4"]),
                    x=int(item["x"]), y=int(item["y"]), z=float(item["z"]),
                    mission_type=mtype,
                ))
                sent_seqs.add(seq)
            # All other frame types (e.g. HEARTBEAT) are silently ignored.
        # Timed out without an ACK.
        return {"mission_type": mtype, "result": -1,
                "reason": "no MISSION_ACK within timeout",
                "acked": len(sent_seqs), "count": count}

    def upload_rollback_probe(self) -> dict:
        """Upload a single item with command=999 — expect MISSION_ACK(UNSUPPORTED).

        This exercises G-3's "rollback on failure" assertion: the mock must
        NACK the transaction with result=MAV_MISSION_UNSUPPORTED(3) and discard
        any partial state for that mission_type.
        """
        items = [{"mission_type": MISSION, "seq": 0, "command": 999, "frame": 3,
                  "x": 0, "y": 0, "z": 0.0,
                  "param1": 0.0, "param2": 0.0, "param3": 0.0, "param4": 0.0,
                  "current": 0, "autocontinue": 1}]
        return self.upload(items)

    # ------------------------------------------------------------------
    # Download (PX4 → GCS)
    # ------------------------------------------------------------------
    def download(self, mission_type: int) -> dict:
        """Download items of the given mission_type. Returns a dict with the
        items and the ack we sent."""
        # 1. Send MISSION_REQUEST_LIST.
        self._send(mb.MAVLink_mission_request_list_message(
            target_system=PX4_SYSID, target_component=PX4_COMPID,
            mission_type=mission_type,
        ))
        # 2. Wait for MISSION_COUNT.
        mc = self._recv(want_type="MISSION_COUNT", deadline=time.time() + self.timeout)
        if mc is None:
            return {"mission_type": mission_type, "items": [],
                    "reason": "no MISSION_COUNT", "count": 0}
        count = int(mc.count)
        items: List[dict] = []
        # 3. Loop: send MISSION_REQUEST(seq), wait for MISSION_ITEM_INT(seq).
        for seq in range(count):
            self._send(mb.MAVLink_mission_request_message(
                target_system=PX4_SYSID, target_component=PX4_COMPID,
                seq=seq, mission_type=mission_type,
            ))
            deadline = time.time() + self.timeout
            got: Optional[object] = None
            while time.time() < deadline:
                msg = self._recv(want_type="MISSION_ITEM_INT",
                                  deadline=time.time() + 0.5)
                if msg is None:
                    continue
                if int(msg.seq) == seq:
                    got = msg
                    break
                # Out-of-order item — buffer it back to the queue so a later
                # iteration can pick it up.
                self._queues["MISSION_ITEM_INT"].append(msg)
            if got is None:
                return {"mission_type": mission_type, "items": items,
                        "reason": f"no MISSION_ITEM_INT for seq={seq}",
                        "count": count}
            items.append(_item_to_dict(got))
        # 4. Send MISSION_ACK(accepted) to close the transaction.
        self._send(mb.MAVLink_mission_ack_message(
            target_system=PX4_SYSID, target_component=PX4_COMPID,
            type=MISSION_ACCEPTED, mission_type=mission_type,
        ))
        return {"mission_type": mission_type, "items": items, "count": count,
                "ack_sent": MISSION_ACCEPTED}


def _ack_to_dict(msg) -> dict:
    return {
        "mission_type": int(msg.mission_type),
        "result": int(msg.type),
        "acked": True,
    }


def _item_to_dict(msg) -> dict:
    return {
        "seq": int(msg.seq), "command": int(msg.command),
        "frame": int(msg.frame),
        "x": int(msg.x), "y": int(msg.y), "z": float(msg.z),
        "param1": float(msg.param1), "param2": float(msg.param2),
        "param3": float(msg.param3), "param4": float(msg.param4),
        "current": int(msg.current), "autocontinue": int(msg.autocontinue),
        "mission_type": int(msg.mission_type),
    }


class _UDPOutput:
    """File-like shim that MAVLink writes encoded frames to."""

    def __init__(self, sock: socket.socket, px4_addr: Tuple[str, int]):
        self.sock = sock
        self.addr = px4_addr

    def write(self, buf) -> int:
        try:
            self.sock.sendto(bytes(buf), self.addr)
        except OSError as e:
            sys.stderr.write(f"[gcs] sendto {self.addr} error: {e}\n")
        return len(buf)


class _NullOut:
    def write(self, buf) -> int:  # pragma: no cover
        return len(buf)


# ---------------------------------------------------------------------------
# Subcommands
# ---------------------------------------------------------------------------
def cmd_upload(args) -> int:
    bind_addr = ("127.0.0.1", args.bind_port)
    px4_addr = ("127.0.0.1", args.px4_port)
    gcs = GcsClient(bind_addr, px4_addr, timeout=args.timeout)

    # Wait for the first heartbeat so we know the mock is up.
    if not gcs._wait_heartbeat(deadline=time.time() + args.timeout):
        print(json.dumps({"ok": False, "reason": "no heartbeat from mock PX4"}))
        return 1

    results = []
    for mtype in (MISSION, FENCE, RALLY):
        items = [i for i in SAMPLE_ITEMS if i["mission_type"] == mtype]
        r = gcs.upload(items)
        results.append({"type": mtype, "items_count": len(items), "ack": r})

    # Rollback probe (G-3 explicit): upload a single item with command=999;
    # the mock should NACK with MAV_MISSION_UNSUPPORTED(3).
    rb = gcs.upload_rollback_probe()
    results.append({"type": "rollback", "items_count": 1, "ack": rb})

    summary = {
        "ok": True,
        "transactions": results,
        "heartbeat": True,
    }
    # Validate: each of the 3 type uploads must have result=0 (accepted), and
    # the rollback probe must have result=3 (unsupported).
    fail_reasons = []
    for r in results[:3]:
        if not r["ack"].get("acked"):
            fail_reasons.append(f"type {r['type']}: no ack")
        elif r["ack"].get("result") != MISSION_ACCEPTED:
            fail_reasons.append(
                f"type {r['type']}: result={r['ack'].get('result')} "
                f"(expected {MISSION_ACCEPTED}=MAV_MISSION_ACCEPTED)")
    rb_result = results[3]["ack"].get("result")
    if rb_result != MISSION_UNSUPPORTED:
        fail_reasons.append(
            f"rollback: result={rb_result} "
            f"(expected {MISSION_UNSUPPORTED}=MAV_MISSION_UNSUPPORTED)")
    if fail_reasons:
        summary["ok"] = False
        summary["reasons"] = fail_reasons
    print(json.dumps(summary))
    return 0 if summary["ok"] else 1


def cmd_emit_preload(args) -> int:
    """Emit the SAMPLE_ITEMS JSON for the mock PX4's --preload flag.

    The bash harness pipes this into a file and passes to mock_px4_mission.py
    so the GCS and mock agree on the expected mission bytes.
    """
    print(json.dumps({"items": SAMPLE_ITEMS}))
    return 0


def cmd_download(args) -> int:
    bind_addr = ("127.0.0.1", args.bind_port)
    px4_addr = ("127.0.0.1", args.px4_port)
    gcs = GcsClient(bind_addr, px4_addr, timeout=args.timeout)

    if not gcs._wait_heartbeat(deadline=time.time() + args.timeout):
        print(json.dumps({"ok": False, "reason": "no heartbeat from mock PX4"}))
        return 1

    # The mock was preloaded with the SAMPLE_ITEMS for mission_type=0 (mission)
    # — assert round-trip equality on (seq, command, x, y, z).
    r = gcs.download(MISSION)
    expected = [i for i in SAMPLE_ITEMS if i["mission_type"] == MISSION]
    summary = {
        "ok": True,
        "heartbeat": True,
        "count": r["count"],
        "items": r["items"],
    }
    fail_reasons = []
    if r["count"] != len(expected):
        fail_reasons.append(f"count mismatch: got {r['count']}, "
                             f"expected {len(expected)}")
    if len(r["items"]) != len(expected):
        fail_reasons.append(f"items len mismatch: got {len(r['items'])}, "
                             f"expected {len(expected)}")
    else:
        for got, want in zip(r["items"], expected):
            for field in ("seq", "command", "x", "y"):
                if int(got[field]) != int(want[field]):
                    fail_reasons.append(
                        f"seq={got['seq']} field {field}: got {got[field]}, "
                        f"expected {want[field]}")
            # z is a float — compare within 1 mm.
            if abs(float(got["z"]) - float(want["z"])) > 1e-3:
                fail_reasons.append(
                    f"seq={got['seq']} field z: got {got['z']}, "
                    f"expected {want['z']}")
    if fail_reasons:
        summary["ok"] = False
        summary["reasons"] = fail_reasons
    print(json.dumps(summary))
    return 0 if summary["ok"] else 1


def main() -> int:
    ap = argparse.ArgumentParser(description="GCS mission test client")
    sub = ap.add_subparsers(dest="cmd", required=True)

    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--bind-port", type=int, default=14540,
                        help="GCS bind port (default 14540)")
    common.add_argument("--px4-port", type=int, default=14580,
                        help="PX4 onboard listen port (default 14580)")
    common.add_argument("--timeout", type=float, default=10.0,
                        help="Per-transaction timeout seconds (default 10)")

    ap_upload = sub.add_parser("upload", parents=[common],
                                help="Run G-3 upload probe")
    ap_upload.set_defaults(func=cmd_upload)

    ap_pre = sub.add_parser("emit-preload",
                             help="Print the SAMPLE_ITEMS JSON (for mock PX4)")
    ap_pre.set_defaults(func=cmd_emit_preload)

    ap_dl = sub.add_parser("download", parents=[common],
                            help="Run G-4 download probe")
    ap_dl.set_defaults(func=cmd_download)

    args = ap.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
