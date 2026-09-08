#!/usr/bin/env python3
"""Mock PX4 SITL for the G-9/G-10 fleet mission-binding + orchestration harnesses.

This mock is the union of the G-3/G-4 ``mock_px4_mission.py`` (MAVLink mission
protocol: MISSION_COUNT → MISSION_REQUEST_INT → MISSION_ITEM_INT →
MISSION_ACK; MISSION_REQUEST_LIST → MISSION_COUNT → MISSION_REQUEST →
MISSION_ITEM_INT → MISSION_ACK) and the G-5/G-6/G-7/G-8 ``mock_px4_fly.py``
(10 Hz telemetry + 1 Hz heartbeat + COMMAND_LONG→COMMAND_ACK + arm/disarm +
PARAM_REQUEST_LIST + PARAM_SET). The M5 fleet endpoints (`/api/fleet/mission-
bindings` + `/api/fleet/start` in parallel/sequential mode) require a single
mock per vehicle that does **both**: the orchestration flow uploads missions
to each vehicle through the existing `/api/vehicles/{i}/mission/upload`
endpoint, which drives the MAVLink mission-protocol; the fleet's per-vehicle
binding table then needs the link established with telemetry so the snapshot
carries `armed=false`, `heartbeat_seen=true`, etc.

Wire topology follows ``fleet-mavlink/src/link.rs`` §3.1 (the same convention
the G-3..G-8 mocks use): the Rust GCS link **binds** 127.0.0.1:14540+i and
**sends** commands to 127.0.0.1:14580+i. This mock plays PX4's role: it binds
:14580+i and sends heartbeats + telemetry to :14540+i. The first datagram
received from a GCS at a different source address updates the mock's reply
destination (PX4's own behaviour — replies go to the source of the most
recent inbound packet).

Telemetry pump (10 Hz, the rcS default after the GCS subscribes):
  * ATTITUDE (msgid 30) — small roll oscillation so the HUD needle moves
  * LOCAL_POSITION_NED (msgid 32) — slow 5 m circle so the map marker drifts
  * GLOBAL_POSITION_INT (msgid 33) — PX4's own geo estimate
  * SYS_STATUS (msgid 1) — battery + sensors
  * HOME_POSITION (msgid 242) — once per second so the FSM parks at READY
    without the 45 s home-fallback gate

Heartbeat pump (1 Hz): the base_mode's MAV_MODE_FLAG_SAFETY_ARMED bit tracks
the mock's ``armed`` flag (set by COMMAND_LONG(400, param1=1)). PX4's
commander does the same — the armed state propagates through the heartbeat,
which is what the GCS aggregates into the vehicle snapshot.

COMMAND_LONG (msgid 76) → COMMAND_ACK (msgid 77, result=0 ACCEPTED) for
every command. Arm/disarm (command 400) is special-cased: param1=1 flips
the heartbeat's SAFETY_ARMED bit on, param1=0 flips it off. The mock
remembers its armed state across heartbeats so the GCS-side snapshot's
``armed`` field flips within one heartbeat period (1 s) of the COMMAND_LONG
round-trip.

Mission protocol (the union of the upload + download halves):

  Upload (GCS → PX4):
      MISSION_COUNT(count, mission_type)
      → PX4 sends MISSION_REQUEST_INT(seq=0)
      GCS sends MISSION_ITEM_INT(seq=0)
      → PX4 sends MISSION_REQUEST_INT(seq=1)
      ...
      GCS sends MISSION_ITEM_INT(seq=N-1)
      → PX4 sends MISSION_ACK(MAV_MISSION_ACCEPTED) on success, or
        MISSION_ACK(MAV_MISSION_UNSUPPORTED) when an item's command is 999
        (the rollback-on-failure probe from G-3).

  Download (PX4 → GCS):
      GCS sends MISSION_REQUEST_LIST(mission_type)
      → PX4 sends MISSION_COUNT(N, mission_type) with N = len(storage[type])
      GCS sends MISSION_REQUEST(seq=i)
      → PX4 sends MISSION_ITEM_INT for seq=i
      ...
      GCS sends MISSION_ACK(MAV_MISSION_ACCEPTED) when the last item arrives.

pymavlink 2.4.49 (common v2.0 dialect) is used for framing + CRC. The wire
layout of every message matches ``fleet-mavlink/src/messages.rs`` (CRC extras
match because pymavlink's auto-generated tables use the same common.xml source).

State file (--state PATH) — written after every transaction, so the bash
harness can assert:
  * instance, sysid, lat_e7, lon_e7, armed
  * last_command / last_command_param1 / last_command_result / command_count
  * received_items: {0: [...], 1: [...], 2: [...]} — per-mission_type lists
  * transaction_log — one entry per upload/download COUNT/ACK pair
  * last_error — when the mock NACKed an upload (rollback probe)

CLI:
  python3 mock_px4_fleet.py [--port 14580] [--gcs-port 14540]
                            [--state PATH] [--instance 0]
                            [--sysid 1] [--ttl-secs 240]
                            [--telemetry-hz 10]
                            [--lat-e7 473977700] [--lon-e7 85455800]
"""
from __future__ import annotations

import argparse
import json
import math
import os
import signal
import socket
import struct
import sys
import threading
import time
from typing import Dict, List, Optional, Tuple

# pymavlink 2.4.49 common v2.0 dialect — same wire layout PX4 emits/expects
# and the same the Rust codec in fleet-mavlink/src/messages.rs is pinned to.
from pymavlink.dialects.v20 import common as mb

# ---------------------------------------------------------------------------
# Constants — mirror fleet-mavlink/src/messages.rs enums (CRC extras match
# because pymavlink's auto-generated tables use the same common.xml source).
# ---------------------------------------------------------------------------
GCS_SYSID = 255        # manager sysid (link.rs LinkConfig::manager_sysid)
GCS_COMPID = 190       # manager compid (link.rs LinkConfig::manager_compid)
PX4_COMPID = 1         # PX4 default compid

# Default geo origin — fleet-core/src/geo.rs GeoOrigin::DEFAULT (the PX4 test
# field). The mock drifts around this point so the harness can assert lat/lon
# change between snapshots.
DEFAULT_LAT_E7 = 473977700   # 47.397770 deg * 1e7
DEFAULT_LON_E7 = 85455800    # 8.545580  deg * 1e7

# Mission protocol constants (mirror mock_px4_mission.py).
MISSION_TYPE_MISSION = mb.MAV_MISSION_TYPE_MISSION      # 0
MISSION_TYPE_FENCE = mb.MAV_MISSION_TYPE_FENCE          # 1
MISSION_TYPE_RALLY = mb.MAV_MISSION_TYPE_RALLY          # 2

MISSION_ACCEPTED = mb.MAV_MISSION_ACCEPTED              # 0
MISSION_UNSUPPORTED = mb.MAV_MISSION_UNSUPPORTED          # 3

ROLLBACK_CMD = 999   # G-3 probe: an item with command=999 triggers a NACK

# ---------------------------------------------------------------------------
# State (guarded by `state_lock`)
# ---------------------------------------------------------------------------
state_lock = threading.Lock()
# Send lock: the MAVLink encoder is not thread-safe (it maintains an internal
# sequence counter). All mav.send() / *_send() calls go through this lock so
# concurrent sends from the heartbeat + telemetry + mission threads can't
# corrupt the encoder state.
send_lock = threading.Lock()

# COMMAND_LONG protocol state (from mock_px4_fly.py)
last_command: Optional[int] = None
last_command_param1: Optional[float] = None
last_command_result: int = 0
command_count: int = 0
armed: bool = False
running: bool = True

# Mission protocol state (from mock_px4_mission.py):
# Per mission_type: list of MISSION_ITEM_INT dicts as
# {seq, command, frame, x, y, z, param1..4, current, autocontinue, mission_type}.
received_items: Dict[int, List[dict]] = {0: [], 1: [], 2: []}
# One entry per protocol transaction: {"kind": "upload"|"download",
#  "mission_type": 0|1|2, "count": N, "result": 0|3, "ts": epoch}.
transaction_log: List[dict] = []
last_error: Optional[str] = None


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


def log(msg: str) -> None:
    sys.stderr.write(f"[mock_px4_fleet] {msg}\n")
    sys.stderr.flush()


def write_state(state_path: str, *, instance: int, sysid: int,
                lat_e7: int, lon_e7: int, armed_: bool) -> None:
    """Atomically rewrite the state JSON file so the harness can read it."""
    with state_lock:
        snapshot = {
            "instance": instance,
            "sysid": sysid,
            "lat_e7": lat_e7,
            "lon_e7": lon_e7,
            "armed": armed_,
            "last_command": last_command,
            "last_command_param1": last_command_param1,
            "last_command_result": last_command_result,
            "command_count": command_count,
            # Mission-protocol state — the harness asserts
            # received_items[type] count after a parallel/sequential start,
            # and transaction_log length matches the # of attempted uploads.
            "received_items": dict(received_items),
            "transaction_log": list(transaction_log),
            "last_error": last_error,
            "ts": time.time(),
        }
    tmp = state_path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(snapshot, f, indent=2)
    os.replace(tmp, state_path)


class _UDPOutput:
    """File-like shim that MAVLink writes encoded frames to.

    The destination address is mutable so the mock can reply to whatever GCS
    source port it most recently received from (PX4's own behaviour).
    """

    def __init__(self, sock: socket.socket, gcs_addr: Tuple[str, int]):
        self.sock = sock
        # Default to the Rust link's bind port — this is what fleet-cli will
        # be listening on, and lets the mock announce itself before the first
        # GCS packet arrives.
        self.addr: Tuple[str, int] = gcs_addr

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


class MockPX4:
    """A 10 Hz telemetry pump + COMMAND_LONG→COMMAND_ACK responder + MAVLink
    mission-protocol responder.

    The class owns:
      - one UDP socket bound to LISTEN_ADDR (default 127.0.0.1:14580)
      - the latest GCS source address (updated on every recvfrom)
      - one MAVLink encoder for outbound frames
      - one MAVLink decoder for inbound frames (kept stateless via parse_buffer)
      - upload transaction state per mission_type (only one in flight at a time
        per type; PX4 refuses concurrent transactions of the same type).
    """

    def __init__(self, listen_addr: Tuple[str, int], gcs_addr: Tuple[str, int],
                 sysid: int, lat_e7: int, lon_e7: int,
                 state_path: str, ttl_secs: float, telemetry_hz: float,
                 instance: int):
        self.listen_addr = listen_addr
        self.gcs_addr = gcs_addr
        self.sysid = sysid
        self.lat_e7_start = lat_e7
        self.lon_e7_start = lon_e7
        self.state_path = state_path
        self.ttl_secs = ttl_secs
        self.telemetry_hz = telemetry_hz
        self.instance = instance
        self.start_ts = time.time()

        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(listen_addr)
        self.sock.settimeout(0.25)

        self.out = _UDPOutput(self.sock, self.gcs_addr)
        self.mav = mb.MAVLink(self.out, srcSystem=sysid, srcComponent=PX4_COMPID)
        self._dec = mb.MAVLink(_NullOut(), srcSystem=0, srcComponent=0)

        # Upload transaction state per mission_type.
        self.upload_count: Dict[int, int] = {}        # mission_type -> expected count
        self.upload_received: Dict[int, int] = {}     # mission_type -> items received so far
        # Snapshot of received_items[mtype] taken at MISSION_COUNT time. If the
        # upload fails (rollback probe / unsupported command), PX4 must restore
        # the previous mission intact.
        self.upload_backup: Dict[int, List[dict]] = {}

    # ------------------------------------------------------------------
    # Heartbeat pump (1 Hz, PX4 default)
    # ------------------------------------------------------------------
    def heartbeat_loop(self) -> None:
        """Send a HEARTBEAT to the GCS every 1 s (link.rs cadence).

        The base_mode's MAV_MODE_FLAG_SAFETY_ARMED bit tracks the mock's
        ``armed`` flag (set by COMMAND_LONG(400, param1=1)). PX4's commander
        does the same — the armed state propagates through the heartbeat,
        which is what the GCS aggregates into the vehicle snapshot.
        """
        global running
        while running:
            try:
                base_mode = mb.MAV_MODE_FLAG_CUSTOM_MODE_ENABLED
                if armed:
                    base_mode |= mb.MAV_MODE_FLAG_SAFETY_ARMED
                with send_lock:
                    self.mav.heartbeat_send(
                        mb.MAV_TYPE_QUADROTOR,
                        mb.MAV_AUTOPILOT_PX4,
                        base_mode,
                        0,  # custom_mode (PX4 main_state)
                        mb.MAV_STATE_ACTIVE,
                    )
            except Exception as e:  # pragma: no cover — defensive
                log(f"heartbeat_send error: {e}")
            time.sleep(1.0)

    # ------------------------------------------------------------------
    # Telemetry pump (10 Hz, the rcS default after the GCS subscribes)
    # ------------------------------------------------------------------
    def telemetry_loop(self) -> None:
        """Send ATTITUDE + LOCAL_POSITION_NED + GLOBAL_POSITION_INT +
        SYS_STATUS + HOME_POSITION at ``telemetry_hz``.

        The position drifts in a slow circle so the harness can assert that
        the snapshot's lat/lon/attitude change between two polls a few
        seconds apart (G-5's "10 Hz telemetry moves on map + strip +
        attitude HUD" gate). The drift is small enough (5 m radius, 0.1 rad/s
        yaw rate) that the GCS-side ``position_ned_m`` won't leave the
        default geofence box.
        """
        global running
        cycle = 0
        while running:
            t = time.time() - self.start_ts
            # Slow circle in NED: radius 5 m, period ~63 s. Altitude -30 m
            # (well above the typical geofence floor+10 m boundary-warning
            # band — see fleet-safety/src/geofence.rs BOUNDARY_WARN_M).
            angle = t * 0.1
            radius_m = 5.0
            x = radius_m * math.cos(angle)
            y = radius_m * math.sin(angle)
            z = -30.0
            vx = -radius_m * 0.1 * math.sin(angle)
            vy = radius_m * 0.1 * math.cos(angle)
            vz = 0.0
            # Attitude: small roll oscillation, yaw follows the circle
            # direction so the HUD needle visibly moves.
            roll = 0.1 * math.sin(t * 0.5)
            pitch = 0.0
            yaw = angle % (2 * math.pi)

            # Lat/lon offset: 1 deg ≈ 111319.9 m at the equator, so 1 m
            # ≈ 8.98e-6 deg ≈ 89.8 lat_e7. Round to keep the wire int stable.
            lat_e7 = self.lat_e7_start + int(x * 89.8)
            lon_e7 = self.lon_e7_start + int(y * 89.8)

            time_boot_ms = int(t * 1000) & 0xFFFFFFFF
            try:
                with send_lock:
                    # ATTITUDE (msgid 30)
                    self.mav.attitude_send(
                        time_boot_ms,
                        roll, pitch, yaw,
                        0.05 * math.cos(t * 0.5),  # rollspeed
                        0.0,                        # pitchspeed
                        0.1,                        # yawspeed
                    )
                    # LOCAL_POSITION_NED (msgid 32)
                    self.mav.local_position_ned_send(
                        time_boot_ms,
                        x, y, z,
                        vx, vy, vz,
                    )
                    # GLOBAL_POSITION_INT (msgid 33)
                    self.mav.global_position_int_send(
                        time_boot_ms,
                        lat_e7, lon_e7,
                        500000,   # alt mm (ASL, 500 m)
                        -30000,   # relative_alt mm (-30 m AGL)
                        0, 0, 0,  # vx, vy, vz cm/s
                        int(math.degrees(yaw) * 100) & 0xFFFF,  # hdg cdeg
                    )
                    # SYS_STATUS (msgid 1)
                    self.mav.sys_status_send(
                        0,        # onboard_control_sensors_present
                        0,        # onboard_control_sensors_enabled
                        0,        # onboard_control_sensors_health
                        500,      # load (5%)
                        12000,    # voltage_battery_mv (12 V)
                        1000,     # current_battery_ca (1 A)
                        80,       # battery_remaining_pct
                        0, 0,     # drop_rate_comm, errors_comm
                        0, 0, 0, 0,  # errors_count1..4
                    )
                    # HOME_POSITION (msgid 242) — once per second, so the FSM
                    # transitions BOOTING → READY without waiting the 45 s
                    # home-fallback gate.
                    if cycle % int(self.telemetry_hz) == 0:
                        self.mav.home_position_send(
                            int(self.lat_e7_start),
                            int(self.lon_e7_start),
                            500000,                  # altitude mm ASL
                            0.0, 0.0, 0.0,           # x, y, z m NED
                            [1.0, 0.0, 0.0, 0.0],    # q
                            0.0, 0.0, 0.0,           # approach_x/y/z
                            int(time.time() * 1e6),  # time_usec
                        )
            except Exception as e:  # pragma: no cover — defensive
                log(f"telemetry_send error: {e}")
            cycle += 1
            time.sleep(1.0 / self.telemetry_hz)

    # ------------------------------------------------------------------
    # Main loop
    # ------------------------------------------------------------------
    def run(self) -> None:
        global running, last_command, last_command_param1, last_command_result, command_count, armed
        global last_error
        hb_thread = threading.Thread(target=self.heartbeat_loop, daemon=True)
        tel_thread = threading.Thread(target=self.telemetry_loop, daemon=True)
        hb_thread.start()
        tel_thread.start()
        log(f"listening on {self.listen_addr[0]}:{self.listen_addr[1]} "
            f"instance={self.instance} sysid={self.sysid} "
            f"lat_e7={self.lat_e7_start} lon_e7={self.lon_e7_start} "
            f"sending to {self.gcs_addr[0]}:{self.gcs_addr[1]} "
            f"at {self.telemetry_hz} Hz")
        # Initial state file write (harness ready-signal).
        write_state(self.state_path, instance=self.instance,
                    sysid=self.sysid, lat_e7=self.lat_e7_start,
                    lon_e7=self.lon_e7_start, armed_=armed)
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
            if src != self.out.addr:
                self.out.addr = src
                log(f"GCS source address updated to {src}")
            self._handle_datagram(data)
        # Final state write
        write_state(self.state_path, instance=self.instance,
                    sysid=self.sysid, lat_e7=self.lat_e7_start,
                    lon_e7=self.lon_e7_start, armed_=armed)
        log("shutdown complete")

    # ------------------------------------------------------------------
    # Frame dispatch — routes both COMMAND_LONG and the mission-protocol
    # messages to their handlers.
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
        global last_command, last_command_param1, last_command_result, command_count, armed
        global last_error
        t = msg.get_type()
        if t == "COMMAND_LONG":
            self._on_command_long(msg)
        elif t == "MISSION_COUNT":
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
            # GCS heartbeat — no action.
            pass
        elif t == "PING":
            pass
        else:
            # Silent ignore — PX4 drops unknown msgids.
            pass

    # ------------------------------------------------------------------
    # COMMAND_LONG → COMMAND_ACK (arm/disarm special-cased)
    # ------------------------------------------------------------------
    def _on_command_long(self, msg) -> None:
        global last_command, last_command_param1, last_command_result, command_count, armed
        command = int(msg.command)
        param1 = float(msg.param1)
        with state_lock:
            last_command = command
            last_command_param1 = param1
            last_command_result = mb.MAV_RESULT_ACCEPTED
            command_count += 1
            # Arm/disarm (COMMAND_LONG 400, COMPONENT_ARM_DISARM):
            # param1=1 → arm, param1=0 → disarm.
            if command == 400:
                armed = bool(int(param1))
                log(f"COMMAND_LONG cmd=400 (ARM_DISARM) param1={param1} "
                    f"-> armed={armed}")
            else:
                log(f"COMMAND_LONG cmd={command} param1={param1} "
                    f"(acking ACCEPTED)")
        ack_msg = mb.MAVLink_command_ack_message(
            command=command,
            result=mb.MAV_RESULT_ACCEPTED,
            progress=0,
            result_param2=0,
            target_system=GCS_SYSID,
            target_component=GCS_COMPID,
        )
        with send_lock:
            self.mav.send(ack_msg)
        write_state(self.state_path, instance=self.instance,
                    sysid=self.sysid, lat_e7=self.lat_e7_start,
                    lon_e7=self.lon_e7_start, armed_=armed)

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
        write_state(self.state_path, instance=self.instance,
                    sysid=self.sysid, lat_e7=self.lat_e7_start,
                    lon_e7=self.lon_e7_start, armed_=armed)

    def _on_mission_item_int(self, msg) -> None:
        global last_error
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
        write_state(self.state_path, instance=self.instance,
                    sysid=self.sysid, lat_e7=self.lat_e7_start,
                    lon_e7=self.lon_e7_start, armed_=armed)

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
        write_state(self.state_path, instance=self.instance,
                    sysid=self.sysid, lat_e7=self.lat_e7_start,
                    lon_e7=self.lon_e7_start, armed_=armed)

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
        write_state(self.state_path, instance=self.instance,
                    sysid=self.sysid, lat_e7=self.lat_e7_start,
                    lon_e7=self.lon_e7_start, armed_=armed)

    def _on_mission_ack(self, msg) -> None:
        mtype = int(msg.mission_type)
        result = int(msg.type)
        with state_lock:
            self._record_download_txn(mtype, len(received_items.get(mtype, [])),
                                        result)
            log(f"MISSION_ACK mission_type={mtype} result={result} — "
                f"download complete")
        write_state(self.state_path, instance=self.instance,
                    sysid=self.sysid, lat_e7=self.lat_e7_start,
                    lon_e7=self.lon_e7_start, armed_=armed)

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


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def main() -> int:
    ap = argparse.ArgumentParser(
        description="Mock PX4 for G-9/G-10 fleet orchestration harnesses "
                    "(telemetry + arm/disarm + mission protocol)")
    ap.add_argument("--port", type=int, default=14580,
                    help="PX4 onboard listen port (default: 14580)")
    ap.add_argument("--gcs-port", type=int, default=14540,
                    help="GCS bind port to send telemetry to (default: 14540)")
    ap.add_argument("--state", default="/tmp/mock_px4_fleet_state.json",
                    help="Path to JSON state file (default: %(default)s)")
    ap.add_argument("--instance", type=int, default=None,
                    help="Vehicle instance (default: port - 14580)")
    ap.add_argument("--sysid", type=int, default=None,
                    help="MAVLink sysid (default: instance + 1, PX4's convention)")
    ap.add_argument("--ttl-secs", type=float, default=240.0,
                    help="Auto-shutdown after this many seconds (default: 240)")
    ap.add_argument("--telemetry-hz", type=float, default=10.0,
                    help="Telemetry stream rate (default: 10 Hz)")
    ap.add_argument("--lat-e7", type=int, default=DEFAULT_LAT_E7,
                    help="Starting latitude degE7 (default: %(default)s)")
    ap.add_argument("--lon-e7", type=int, default=DEFAULT_LON_E7,
                    help="Starting longitude degE7 (default: %(default)s)")
    args = ap.parse_args()

    instance = args.instance if args.instance is not None else args.port - 14580
    sysid = args.sysid if args.sysid is not None else instance + 1

    listen_addr = ("127.0.0.1", args.port)
    gcs_addr = ("127.0.0.1", args.gcs_port)

    mock = MockPX4(listen_addr, gcs_addr, sysid, args.lat_e7, args.lon_e7,
                   args.state, args.ttl_secs, args.telemetry_hz, instance)

    def _sigterm(*_):
        global running
        running = False
        log("SIGTERM received — shutting down")

    signal.signal(signal.SIGTERM, _sigterm)
    signal.signal(signal.SIGINT, _sigterm)
    try:
        mock.run()
    finally:
        write_state(args.state, instance=instance, sysid=sysid,
                    lat_e7=args.lat_e7, lon_e7=args.lon_e7, armed_=armed)
    return 0


if __name__ == "__main__":
    sys.exit(main())
