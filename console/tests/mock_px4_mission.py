#!/usr/bin/env python3
"""Mock PX4 SITL for the G-3/G-4 MAVLink mission-protocol harnesses.

Wire topology follows ``fleet-mavlink/src/link.rs`` §3.1: the Rust GCS link
**binds** 127.0.0.1:14540+i and **sends** to 127.0.0.1:14580+i. This mock
therefore plays PX4's role: it binds :14580 and sends heartbeats/telemetry to
:14540 (the GCS bind port). On the first datagram received from a GCS at a
different source address, the mock records that source and echoes replies to
it as well — this lets a pymavlink GCS test driver with an ephemeral source
port talk to the same mock that the Rust link would use.

The mock speaks MAVLink v2 and implements both halves of the mission protocol
(`MAVLink Mission Protocol <https://mavlink.io/en/services/mission.html>`_):

  Upload (GCS → PX4):
      MISSION_COUNT(count, mission_type)
      → PX4 sends MISSION_REQUEST_INT(seq=0)
      GCS sends MISSION_ITEM_INT(seq=0)
      → PX4 sends MISSION_REQUEST_INT(seq=1)
      ...
      GCS sends MISSION_ITEM_INT(seq=N-1)
      → PX4 sends MISSION_ACK(MAV_MISSION_ACCEPTED) on success, or
        MISSION_ACK(MAV_MISSION_UNSUPPORTED) when an item's command is 999
        (the rollback-on-failure probe for G-3).

  Download (PX4 → GCS):
      GCS sends MISSION_REQUEST_LIST(mission_type)
      → PX4 sends MISSION_COUNT(N, mission_type) with N = len(storage[type])
      GCS sends MISSION_REQUEST(seq=i)
      → PX4 sends MISSION_ITEM_INT for seq=i
      ...
      GCS sends MISSION_ACK(MAV_MISSION_ACCEPTED) when the last item arrives.

pymavlink 2.4.49 (common v2.0 dialect) is used for framing + CRC. The wire
layout of every message matches ``fleet-mavlink/src/messages.rs`` (CRC extras:
43→132, 44→221, 47→153, 51→226, 73→38, 40→228, 0→50).

The mock writes a JSON state file (``--state PATH``) after every transaction so
the bash harness can assert:
  - received_items[mission_type] count
  - transaction_log (one entry per COUNT/ACK pair)
  - last_error (when the mock NACKed an upload)

CLI:
  python3 mock_px4_mission.py [--state PATH] [--preload PATH]
                              [--port 14580] [--gcs-port 14540]
                              [--ttl-secs 120]
"""
from __future__ import annotations

import argparse
import json
import os
import signal
import socket
import sys
import threading
import time
from typing import Dict, List, Optional, Tuple

# Use the v2.0 common dialect — same wire layout PX4 emits/expects and same
# the Rust codec was pinned against (see fleet-mavlink/src/messages.rs header).
from pymavlink.dialects.v20 import common as mb

# ---------------------------------------------------------------------------
# Constants — mirror fleet-mavlink/src/messages.rs enums
# ---------------------------------------------------------------------------
PX4_SYSID = 1        # PX4 default sysid (matches link.rs cfg.instance + 1 for i=0)
PX4_COMPID = 1       # PX4 default compid
GCS_SYSID = 255      # Manager sysid (link.rs LinkConfig::manager_sysid)
GCS_COMPID = 190     # Manager compid (link.rs LinkConfig::manager_compid)

MISSION_TYPE_MISSION = mb.MAV_MISSION_TYPE_MISSION      # 0
MISSION_TYPE_FENCE = mb.MAV_MISSION_TYPE_FENCE          # 1
MISSION_TYPE_RALLY = mb.MAV_MISSION_TYPE_RALLY          # 2

MISSION_ACCEPTED = mb.MAV_MISSION_ACCEPTED              # 0
MISSION_UNSUPPORTED = mb.MAV_MISSION_UNSUPPORTED        # 3

ROLLBACK_CMD = 999   # G-3 probe: an item with command=999 triggers a NACK

# ---------------------------------------------------------------------------
# State (guarded by `state_lock`)
# ---------------------------------------------------------------------------
state_lock = threading.Lock()
# Send lock: the MAVLink encoder is not thread-safe (it maintains an internal
# sequence counter). All mav.send() / heartbeat_send() calls go through this
# lock so concurrent sends from the heartbeat thread and the main dispatch
# thread can't corrupt the encoder state.
send_lock = threading.Lock()
# Per mission_type: list of MISSION_ITEM_INT dicts as
# {seq, command, frame, x, y, z, param1..4, current, autocontinue, mission_type}.
received_items: Dict[int, List[dict]] = {0: [], 1: [], 2: []}
# One entry per protocol transaction: {"kind": "upload"|"download",
#  "mission_type": 0|1|2, "count": N, "result": 0|3, "ts": epoch}.
transaction_log: List[dict] = []
last_error: Optional[str] = None
running = True


def item_to_dict(msg) -> dict:
    return {
        "seq": int(msg.seq),
        "command": int(msg.command),
        "frame": int(msg.frame),
        "x": int(msg.x),
        "y": int(msg.y),
        "z": float(msg.z),
        "param1": float(msg.param1),
        "param2": float(msg.param2),
        "param3": float(msg.param3),
        "param4": float(msg.param4),
        "current": int(msg.current),
        "autocontinue": int(msg.autocontinue),
        "mission_type": int(msg.mission_type),
    }


def write_state(state_path: str) -> None:
    """Atomically rewrite the state JSON file so the harness can read it."""
    with state_lock:
        snapshot = {
            "received_items": dict(received_items),
            "transaction_log": list(transaction_log),
            "last_error": last_error,
            "ts": time.time(),
        }
    tmp = state_path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(snapshot, f, indent=2)
    os.replace(tmp, state_path)


def log(msg: str) -> None:
    sys.stderr.write(f"[mock_px4] {msg}\n")
    sys.stderr.flush()


# ---------------------------------------------------------------------------
# Mock PX4 state machine
# ---------------------------------------------------------------------------
class MockPX4:
    """Implements the upload + download halves of the MAVLink mission protocol.

    The class owns:
      - one UDP socket bound to LISTEN_ADDR (default 127.0.0.1:14580)
      - the latest GCS source address (updated on every recvfrom)
      - one MAVLink encoder for outbound frames
      - one MAVLink decoder for inbound frames (kept stateless via parse_buffer)
    """

    def __init__(self, listen_addr: Tuple[str, int], gcs_addr: Tuple[str, int],
                 state_path: str, ttl_secs: float):
        self.listen_addr = listen_addr
        self.gcs_addr = gcs_addr
        self.state_path = state_path
        self.ttl_secs = ttl_secs
        self.start_ts = time.time()

        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(listen_addr)
        self.sock.settimeout(0.25)

        # Encoder: heartbeat + mission frames write into self.sock via sendto.
        self.out = _UDPOutput(self.sock, self)
        self.mav = mb.MAVLink(self.out, srcSystem=PX4_SYSID, srcComponent=PX4_COMPID)
        # Decoder: stateless instance — parse_buffer doesn't keep cross-call state
        # for the fields we care about.
        self._dec = mb.MAVLink(_NullOut(), srcSystem=0, srcComponent=0)

        # Upload transaction state per mission_type (only one in flight at a time
        # per type; PX4 refuses concurrent transactions of the same type).
        self.upload_count: Dict[int, int] = {}        # mission_type -> expected count
        self.upload_received: Dict[int, int] = {}     # mission_type -> items received so far
        # Snapshot of received_items[mtype] taken at MISSION_COUNT time. If the
        # upload fails (rollback probe / unsupported command), PX4 must restore
        # the previous mission intact — see mavlink_mission.cpp's
        # `mission_item_update`/`mission_item_to_mission_item` flow.
        self.upload_backup: Dict[int, List[dict]] = {}

    # ------------------------------------------------------------------
    # Heartbeat pump
    # ------------------------------------------------------------------
    def heartbeat_loop(self) -> None:
        """Send a HEARTBEAT to the GCS every 1 s (link.rs cadence)."""
        global running
        while running:
            try:
                with send_lock:
                    self.mav.heartbeat_send(
                        mb.MAV_TYPE_QUADROTOR,
                        mb.MAV_AUTOPILOT_PX4,
                        mb.MAV_MODE_FLAG_SAFETY_ARMED
                        | mb.MAV_MODE_FLAG_CUSTOM_MODE_ENABLED,
                        0,  # custom_mode
                        mb.MAV_STATE_ACTIVE,
                    )
            except Exception as e:  # pragma: no cover — defensive
                log(f"heartbeat_send error: {e}")
            time.sleep(1.0)

    # ------------------------------------------------------------------
    # Main loop
    # ------------------------------------------------------------------
    def run(self) -> None:
        global running, last_error
        hb_thread = threading.Thread(target=self.heartbeat_loop, daemon=True)
        hb_thread.start()
        log(f"listening on {self.listen_addr[0]}:{self.listen_addr[1]}, "
            f"sending to {self.gcs_addr[0]}:{self.gcs_addr[1]}")
        write_state(self.state_path)  # initial empty state file
        while running:
            if time.time() - self.start_ts > self.ttl_secs:
                log(f"ttl {self.ttl_secs}s elapsed — exiting")
                break
            try:
                data, src = self.sock.recvfrom(2048)
            except socket.timeout:
                continue
            except OSError as e:
                if not running:
                    break
                log(f"recvfrom error: {e}")
                continue
            # Track the latest GCS source so we can reply (PX4 echoes to the
            # source of the most-recent packet).
            if src != self.out.addr:
                self.out.addr = src
                log(f"GCS source address updated to {src}")
            self._handle_datagram(data)
        # Final state write
        write_state(self.state_path)
        log("shutdown complete")

    # ------------------------------------------------------------------
    # Frame dispatch
    # ------------------------------------------------------------------
    def _handle_datagram(self, data: bytes) -> None:
        try:
            msgs = self._dec.parse_buffer(data)
        except Exception as e:
            log(f"parse_buffer error: {e}")
            return
        for msg in msgs or []:
            self._dispatch(msg)

    def _dispatch(self, msg) -> None:
        t = msg.get_type()
        if t == "MISSION_COUNT":
            self._on_mission_count(msg)
        elif t == "MISSION_ITEM_INT":
            self._on_mission_item_int(msg)
        elif t == "MISSION_REQUEST_LIST":
            self._on_mission_request_list(msg)
        elif t == "MISSION_REQUEST":
            self._on_mission_request(msg)
        elif t == "MISSION_REQUEST_INT":
            # The Rust GCS uses MISSION_REQUEST (not INT) for download; upload
            # uses MISSION_REQUEST_INT from PX4. If the GCS sends an INT, treat
            # it like a REQUEST (covers both code paths).
            self._on_mission_request(msg)
        elif t == "MISSION_ACK":
            self._on_mission_ack(msg)
        elif t == "HEARTBEAT":
            # GCS heartbeat — no action (we already know the GCS is alive).
            pass
        else:
            # Silent ignore — PX4 drops unknown msgids.
            pass

    # ------------------------------------------------------------------
    # Upload half: MISSION_COUNT → MISSION_REQUEST_INT × N → MISSION_ACK
    # ------------------------------------------------------------------
    def _on_mission_count(self, msg) -> None:
        mtype = int(msg.mission_type)
        count = int(msg.count)
        with state_lock:
            # Snapshot the current items for this mission_type. If the upload
            # fails (rollback probe), PX4 restores this snapshot — the
            # previously-stored mission is unaffected by an aborted upload.
            self.upload_backup[mtype] = list(received_items.get(mtype, []))
            self.upload_count[mtype] = count
            self.upload_received[mtype] = 0
            # Per protocol, receiving a fresh MISSION_COUNT resets the
            # in-flight transaction; previous pending items are discarded.
            received_items[mtype] = []
        log(f"MISSION_COUNT count={count} mission_type={mtype} — "
            f"requesting seq=0 (backup={len(self.upload_backup.get(mtype, []))} items)")
        # Always send MISSION_REQUEST_INT for the INT variant (matches link.rs
        # which handles both REQUEST and REQUEST_INT in the same branch).
        self._send_request_int(seq=0, mission_type=mtype)
        write_state(self.state_path)

    def _on_mission_item_int(self, msg) -> None:
        mtype = int(msg.mission_type)
        seq = int(msg.seq)
        item = item_to_dict(msg)
        # Decide what to do under the state lock, then perform the send
        # *outside* the lock so we never hold state_lock across a socket
        # send (which could deadlock with the heartbeat thread).
        action: str
        next_seq: int = 0
        ack_result: int = 0
        with state_lock:
            # Rollback-on-failure probe (G-3): an item carrying command=999
            # means the GCS is testing the failure path. Reply with an ERROR
            # ACK and discard the partial upload.
            if int(msg.command) == ROLLBACK_CMD:
                last_error = f"unsupported command {ROLLBACK_CMD} at seq {seq}"
                self._record_upload_txn(mtype, len(received_items[mtype]),
                                         MISSION_UNSUPPORTED)
                # Restore the snapshot taken at MISSION_COUNT time — PX4 keeps
                # the previously-stored mission intact when an upload fails
                # mid-transaction (the "rollback on failure" semantic that
                # G-3 asserts against).
                backup = self.upload_backup.pop(mtype, [])
                n_restored = len(backup)
                received_items[mtype] = backup
                self.upload_count.pop(mtype, None)
                self.upload_received.pop(mtype, None)
                log(f"MISSION_ITEM_INT seq={seq} cmd={ROLLBACK_CMD} — "
                    f"rolling back, sending NACK(result={MISSION_UNSUPPORTED}), "
                    f"restored {n_restored} prior items")
                action = "nack"
                ack_result = MISSION_UNSUPPORTED
            else:
                # Append / replace by seq (PX4 overwrites duplicate seqs).
                items = received_items[mtype]
                if seq < len(items):
                    items[seq] = item
                else:
                    while len(items) < seq:
                        items.append({})
                    items.append(item)
                self.upload_received[mtype] = seq + 1
                count = self.upload_count.get(mtype, 0)
                log(f"MISSION_ITEM_INT seq={seq} cmd={msg.command} "
                    f"({seq + 1}/{count})")
                if seq + 1 < count:
                    action = "request_next"
                    next_seq = seq + 1
                else:
                    # All items received — ack accepted.
                    self._record_upload_txn(mtype, len(items),
                                             MISSION_ACCEPTED)
                    log(f"upload complete for mission_type={mtype} "
                        f"({len(items)} items) — sending MISSION_ACK(0)")
                    action = "ack"
                    ack_result = MISSION_ACCEPTED
        # Send outside the state lock.
        if action == "nack":
            self._send_mission_ack(mtype, ack_result)
        elif action == "request_next":
            self._send_request_int(seq=next_seq, mission_type=mtype)
        elif action == "ack":
            self._send_mission_ack(mtype, ack_result)
        write_state(self.state_path)

    # ------------------------------------------------------------------
    # Download half: MISSION_REQUEST_LIST → MISSION_COUNT → MISSION_REQUEST
    # → MISSION_ITEM_INT → MISSION_ACK
    # ------------------------------------------------------------------
    def _on_mission_request_list(self, msg) -> None:
        mtype = int(msg.mission_type)
        with state_lock:
            n = len(received_items.get(mtype, []))
        log(f"MISSION_REQUEST_LIST mission_type={mtype} — replying with "
            f"MISSION_COUNT({n})")
        self._send_mission_count(count=n, mission_type=mtype)
        write_state(self.state_path)

    def _on_mission_request(self, msg) -> None:
        mtype = int(msg.mission_type)
        seq = int(msg.seq)
        with state_lock:
            items = received_items.get(mtype, [])
            if 0 <= seq < len(items):
                item = dict(items[seq])
            else:
                log(f"MISSION_REQUEST seq={seq} out of range "
                    f"(have {len(items)} items for type={mtype})")
                return
        log(f"MISSION_REQUEST seq={seq} mission_type={mtype} — sending item")
        self._send_mission_item_int(item)
        write_state(self.state_path)

    def _on_mission_ack(self, msg) -> None:
        mtype = int(msg.mission_type)
        result = int(msg.type)
        with state_lock:
            self._record_download_txn(mtype, len(received_items.get(mtype, [])),
                                        result)
            log(f"MISSION_ACK mission_type={mtype} result={result} — "
                f"download complete")
        write_state(self.state_path)

    # ------------------------------------------------------------------
    # Senders (use the MAVLink encoder + UDPOutput)
    # ------------------------------------------------------------------
    def _send_request_int(self, seq: int, mission_type: int) -> None:
        msg = mb.MAVLink_mission_request_int_message(
            target_system=GCS_SYSID, target_component=GCS_COMPID,
            seq=seq, mission_type=mission_type,
        )
        with send_lock:
            self.mav.send(msg)

    def _send_mission_count(self, count: int, mission_type: int) -> None:
        msg = mb.MAVLink_mission_count_message(
            target_system=GCS_SYSID, target_component=GCS_COMPID,
            count=count, mission_type=mission_type,
        )
        with send_lock:
            self.mav.send(msg)

    def _send_mission_item_int(self, item: dict) -> None:
        msg = mb.MAVLink_mission_item_int_message(
            target_system=GCS_SYSID, target_component=GCS_COMPID,
            seq=int(item["seq"]), frame=int(item["frame"]),
            command=int(item["command"]),
            current=int(item.get("current", 0)),
            autocontinue=int(item.get("autocontinue", 1)),
            param1=float(item["param1"]), param2=float(item["param2"]),
            param3=float(item["param3"]), param4=float(item["param4"]),
            x=int(item["x"]), y=int(item["y"]), z=float(item["z"]),
            mission_type=int(item["mission_type"]),
        )
        with send_lock:
            self.mav.send(msg)

    def _send_mission_ack(self, mission_type: int, result: int) -> None:
        msg = mb.MAVLink_mission_ack_message(
            target_system=GCS_SYSID, target_component=GCS_COMPID,
            type=result, mission_type=mission_type,
        )
        with send_lock:
            self.mav.send(msg)

    # ------------------------------------------------------------------
    # Transaction log helpers
    # ------------------------------------------------------------------
    def _record_upload_txn(self, mtype: int, n: int, result: int) -> None:
        transaction_log.append({
            "kind": "upload",
            "mission_type": mtype,
            "count": n,
            "result": result,
            "ts": time.time(),
        })

    def _record_download_txn(self, mtype: int, n: int, result: int) -> None:
        transaction_log.append({
            "kind": "download",
            "mission_type": mtype,
            "count": n,
            "result": result,
            "ts": time.time(),
        })


class _UDPOutput:
    """File-like shim that MAVLink writes encoded frames to.

    The destination address is mutable so the mock can reply to whatever GCS
    source port it most recently received from (PX4's own behaviour).
    """

    def __init__(self, sock: socket.socket, owner: MockPX4):
        self.sock = sock
        self.owner = owner
        # Default to the Rust link's bind port — this is what fleet-cli will
        # be listening on, and lets the mock announce itself before the first
        # GCS packet arrives.
        self.addr: Tuple[str, int] = owner.gcs_addr

    def write(self, buf) -> int:
        try:
            self.sock.sendto(bytes(buf), self.addr)
        except OSError as e:
            log(f"sendto error to {self.addr}: {e}")
        return len(buf)


class _NullOut:
    """No-op sink for the decoder MAVLink instance (we never call send on it)."""

    def write(self, buf) -> int:  # pragma: no cover — defensive
        return len(buf)


# ---------------------------------------------------------------------------
# Preload — for G-4, the mock starts with a 3-waypoint mission so the GCS can
# download it without first uploading.
# ---------------------------------------------------------------------------
def preload(state_path: str, preload_path: str) -> None:
    """Populate received_items from a JSON file before the mock starts.

    Expected format:
      {"items": [
         {"mission_type": 0, "seq": 0, "command": 16, "frame": 3,
          "x": 374133000, "y": -1221017000, "z": 12.0,
          "param1": 0.5, "param2": 2.0, "param3": 0.0, "param4": 0.0,
          "current": 0, "autocontinue": 1},
         ...
      ]}
    """
    with open(preload_path) as f:
        data = json.load(f)
    with state_lock:
        for item in data.get("items", []):
            mtype = int(item["mission_type"])
            received_items[mtype].append(dict(item))
    n_per_type = {k: len(v) for k, v in received_items.items()}
    log(f"preloaded: {n_per_type}")
    write_state(state_path)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def main() -> int:
    ap = argparse.ArgumentParser(description="Mock PX4 for G-3/G-4 harnesses")
    ap.add_argument("--state", default="/tmp/mock_px4_state.json",
                    help="Path to JSON state file (default: %(default)s)")
    ap.add_argument("--preload", default=None,
                    help="JSON file with mission items to pre-load")
    ap.add_argument("--port", type=int, default=14580,
                    help="PX4 onboard listen port (default: 14580)")
    ap.add_argument("--gcs-port", type=int, default=14540,
                    help="GCS bind port to send heartbeats to (default: 14540)")
    ap.add_argument("--ttl-secs", type=float, default=120.0,
                    help="Auto-shutdown after this many seconds (default: 120)")
    args = ap.parse_args()

    listen_addr = ("127.0.0.1", args.port)
    gcs_addr = ("127.0.0.1", args.gcs_port)

    if args.preload:
        preload(args.state, args.preload)

    mock = MockPX4(listen_addr, gcs_addr, args.state, args.ttl_secs)

    def _sigterm(*_):
        global running
        running = False
        log("SIGTERM received — shutting down")

    signal.signal(signal.SIGTERM, _sigterm)
    signal.signal(signal.SIGINT, _sigterm)
    try:
        mock.run()
    finally:
        write_state(args.state)
    return 0


if __name__ == "__main__":
    sys.exit(main())
