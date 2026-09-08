#!/usr/bin/env python3
"""Mock PX4 SITL for the G-5/G-6/G-7/G-8 Fly-View + arm/disarm + setup harnesses.

Wire topology follows ``fleet-mavlink/src/link.rs`` §3.1 (the same convention
the G-3/G-4 mock uses): the Rust GCS link **binds** 127.0.0.1:14540+i and
**sends** commands to 127.0.0.1:14580+i. This mock plays PX4's role: it binds
:14580+i and sends heartbeats + telemetry to :14540+i. The first datagram
received from a GCS at a different source address updates the mock's reply
destination (PX4's own behaviour — replies go to the source of the most
recent inbound packet).

The mock emits a 10 Hz telemetry stream (ATTITUDE, LOCAL_POSITION_NED,
GLOBAL_POSITION_INT, SYS_STATUS) and a 1 Hz HEARTBEAT (matches PX4's
default rcS stream set, ADR-0001 §3.1). It also broadcasts HOME_POSITION
once per second so the FSM can transition BOOTING → READY without waiting
the 45 s home-fallback gate. The position drifts slowly so the harness can
assert lat/lon/attitude change between snapshots (G-5's "10 Hz telemetry
moves on map + strip + attitude HUD" gate).

For G-7, the mock responds to every COMMAND_LONG with COMMAND_ACK result=0
(MAV_RESULT_ACCEPTED). Arm/disarm (command 400) is special-cased: param1=1
flips the heartbeat's MAV_MODE_FLAG_SAFETY_ARMED bit on, param1=0 flips it
off — the same wire behaviour PX4 has when its commander state machine
actually changes the armed flag. The mock remembers its armed state across
heartbeats so the GCS-side snapshot's ``armed`` field flips within one
heartbeat period (1 s) of the COMMAND_LONG round-trip.

For G-8 (Vehicle Setup extensions), the mock also responds to:
  * ``PARAM_REQUEST_LIST`` (msgid 21) by streaming 20 ``PARAM_VALUE``
    (msgid 22) messages — the same 20 params the backend's defaults table
    knows about (``fleet-cli/src/setup.rs::PARAM_DEFAULTS``). Each frame
    carries the right ``param_count`` (20), ``param_index`` (0..19) and
    ``param_type`` (6 = INT32, 9 = REAL32) so the link's param-store
    completion logic flips to ``Complete`` after the last frame.
  * ``PARAM_SET`` (msgid 23) by echoing back a ``PARAM_VALUE`` with the
    new value (PX4's echo-confirmation pattern). The mock's param store
    is updated in-memory so subsequent ``PARAM_REQUEST_LIST`` requests
    reflect the new value — the same PX4 autosave behaviour.

The 20 params and their compiled-in defaults (PX4 v1.16.2):

    MPC_XY_VEL_MAX=12.0, MPC_Z_VEL_MAX_UP=3.0, MPC_Z_VEL_MAX_DN=1.0,
    MPC_XY_CRUISE=5.0, MPC_CRUISE_90=5.0, MPC_TKO_SPEED=1.0,
    MC_ROLLRATE_P=6.5, MC_ROLLRATE_I=0.0, MC_ROLLRATE_D=0.003,
    MC_PITCHRATE_P=6.5, MC_PITCHRATE_I=0.0, MC_PITCHRATE_D=0.003,
    MC_YAWRATE_P=200.0, MC_YAWRATE_I=0.0, MC_YAWRATE_D=0.0,
    FW_AIRSPD_TRIM=15.0, BAT_N_CELLS=4 (int), BAT_V_EMPTY=3.4,
    BAT_V_CHARGED=4.1, NAV_DLL_ACT=0 (int)

CLI:
    python3 mock_px4_fly.py [--port 14580] [--gcs-port 14540]
                            [--state PATH] [--instance 0]
                            [--sysid 1] [--ttl-secs 120]
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
from typing import Optional, Tuple

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
# field). The mock drifts around this point; the harness's "telemetry is
# moving" assertion compares two snapshots taken a few seconds apart.
DEFAULT_LAT_E7 = 473977700   # 47.397770 deg * 1e7
DEFAULT_LON_E7 = 85455800    # 8.545580  deg * 1e7

# ---------------------------------------------------------------------------
# G-8 param store — the 20 PX4 v1.16.2 defaults the backend's
# ``fleet-cli/src/setup.rs::PARAM_DEFAULTS`` table also knows about. Every
# entry is the wire f32 the mock should send on the wire: REAL32 params
# carry the float as-is, INT32 params (BAT_N_CELLS=4, NAV_DLL_ACT=0) carry
# the integer bit-cast into the f32 field (PX4's own wire convention — see
# ``fleet-mavlink/src/link.rs::ParamVal::to_wire``).
#
# ``type`` is the MAVLink param_type byte: 6 = INT32, 9 = REAL32 (PX4's
# own param-groupings — fleet-cli/src/setup.rs's PARAM_DEFAULTS table
# mirrors this exactly so ``is_changed`` is false after the initial
# download).
# ---------------------------------------------------------------------------
MAVLINK_TYPE_INT32 = 6
MAVLINK_TYPE_REAL32 = 9

# (id, wire_value, param_type) — order matters: this is the wire order the
# mock streams them in response to PARAM_REQUEST_LIST (PX4 streams them in
# ROMFS-load order; we just need a stable, indexed order).
MOCK_PARAMS: list[tuple[str, float, int]] = [
    ("MPC_XY_VEL_MAX",  12.0, MAVLINK_TYPE_REAL32),
    ("MPC_Z_VEL_MAX_UP", 3.0, MAVLINK_TYPE_REAL32),
    ("MPC_Z_VEL_MAX_DN", 1.0, MAVLINK_TYPE_REAL32),
    ("MPC_XY_CRUISE",    5.0, MAVLINK_TYPE_REAL32),
    ("MPC_CRUISE_90",    5.0, MAVLINK_TYPE_REAL32),
    ("MPC_TKO_SPEED",    1.0, MAVLINK_TYPE_REAL32),
    ("MC_ROLLRATE_P",    6.5,   MAVLINK_TYPE_REAL32),
    ("MC_ROLLRATE_I",    0.0,   MAVLINK_TYPE_REAL32),
    ("MC_ROLLRATE_D",    0.003, MAVLINK_TYPE_REAL32),
    ("MC_PITCHRATE_P",   6.5,   MAVLINK_TYPE_REAL32),
    ("MC_PITCHRATE_I",   0.0,   MAVLINK_TYPE_REAL32),
    ("MC_PITCHRATE_D",   0.003, MAVLINK_TYPE_REAL32),
    ("MC_YAWRATE_P",    200.0,  MAVLINK_TYPE_REAL32),
    ("MC_YAWRATE_I",     0.0,   MAVLINK_TYPE_REAL32),
    ("MC_YAWRATE_D",     0.0,   MAVLINK_TYPE_REAL32),
    ("FW_AIRSPD_TRIM",  15.0,   MAVLINK_TYPE_REAL32),
    # INT32 params: the wire value is the integer bit-cast into f32
    # (f32::from_bits(4) and f32::from_bits(0) — the same PX4 wires).
    ("BAT_N_CELLS",     float.frombits(4 & 0xFFFFFFFF), MAVLINK_TYPE_INT32),
    ("BAT_V_EMPTY",      3.4, MAVLINK_TYPE_REAL32),
    ("BAT_V_CHARGED",     4.1, MAVLINK_TYPE_REAL32),
    ("NAV_DLL_ACT",     float.frombits(0 & 0xFFFFFFFF), MAVLINK_TYPE_INT32),
]
assert len(MOCK_PARAMS) == 20

# ---------------------------------------------------------------------------
# State (guarded by `state_lock`)
# ---------------------------------------------------------------------------
state_lock = threading.Lock()
# Send lock: the MAVLink encoder is not thread-safe (it maintains an internal
# sequence counter). All mav.send() / heartbeat_send() calls go through this
# lock so concurrent sends from the heartbeat + telemetry threads can't
# corrupt the encoder state.
send_lock = threading.Lock()
last_command: Optional[int] = None
last_command_param1: Optional[float] = None
last_command_result: int = 0
command_count: int = 0
armed: bool = False
running: bool = True

# G-8: in-memory param store, keyed by param id. The mock seeds itself from
# ``MOCK_PARAMS`` (PX4's compiled-in defaults) on startup. ``PARAM_SET``
# updates this dict in place (PX4's autosave behaviour) so subsequent
# ``PARAM_REQUEST_LIST`` streams reflect the new value — the same way a real
# PX4 makes the written value persistent across a fresh download. Each value
# is ``(wire_value, param_type)`` — the wire f32 + MAVLink param_type byte.
param_store: dict[str, tuple[float, int]] = {pid: (val, ptype) for pid, val, ptype in MOCK_PARAMS}
# How many PARAM_REQUEST_LIST requests the mock has seen — the harness can
# read this from the state file to verify the catalog's full download
# actually fired (a regression guard: if the catalog stops sending
# PARAM_REQUEST_LIST, G-8 fails on the param-search test before the mock
# ever streams anything).
param_list_request_count: int = 0
param_value_send_count: int = 0
param_set_count: int = 0


def reset_param_store() -> None:
    """Restore the param store to the PX4-compiled-in defaults.

    Called once at startup; safe to call again to roll back any in-memory
    writes (the harness doesn't, but it's useful for ad-hoc testing).
    """
    global param_store
    param_store = {pid: (val, ptype) for pid, val, ptype in MOCK_PARAMS}


def log(msg: str) -> None:
    sys.stderr.write(f"[mock_px4_fly] {msg}\n")
    sys.stderr.flush()


def write_state(state_path: str, *, instance: int, sysid: int,
                lat_e7: int, lon_e7: int, armed_: bool) -> None:
    """Atomically rewrite the state JSON file so the harness can read it."""
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
        # G-8: param-protocol counters — the harness can assert
        # ``param_list_request_count >= 1`` after the refresh endpoint
        # fires, and ``param_value_send_count >= 20`` once the burst
        # completes. ``param_set_count`` flips to >= 1 after Test 4's
        # MPC_XY_VEL_MAX=8.0 write.
        "param_list_request_count": param_list_request_count,
        "param_value_send_count": param_value_send_count,
        "param_set_count": param_set_count,
        "params_known": len(param_store),
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
    """A 10 Hz telemetry pump + COMMAND_LONG → COMMAND_ACK responder.

    The class owns:
      - one UDP socket bound to LISTEN_ADDR (default 127.0.0.1:14580)
      - the latest GCS source address (updated on every recvfrom)
      - one MAVLink encoder for outbound frames
      - one MAVLink decoder for inbound frames (kept stateless via parse_buffer)
    """

    def __init__(self, listen_addr: Tuple[str, int], gcs_addr: Tuple[str, int],
                 sysid: int, lat_e7: int, lon_e7: int,
                 state_path: str, ttl_secs: float, telemetry_hz: float):
        self.listen_addr = listen_addr
        self.gcs_addr = gcs_addr
        self.sysid = sysid
        self.lat_e7_start = lat_e7
        self.lon_e7_start = lon_e7
        self.state_path = state_path
        self.ttl_secs = ttl_secs
        self.telemetry_hz = telemetry_hz
        self.start_ts = time.time()

        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(listen_addr)
        self.sock.settimeout(0.25)

        self.out = _UDPOutput(self.sock, self.gcs_addr)
        self.mav = mb.MAVLink(self.out, srcSystem=sysid, srcComponent=PX4_COMPID)
        self._dec = mb.MAVLink(_NullOut(), srcSystem=0, srcComponent=0)

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
                    # GLOBAL_POSITION_INT (msgid 33) — PX4's own geo estimate
                    # (ADR-0017: what the operator map renders).
                    self.mav.global_position_int_send(
                        time_boot_ms,
                        lat_e7, lon_e7,
                        500000,   # alt mm (ASL, 500 m)
                        -30000,   # relative_alt mm (-30 m AGL, well above the
                                  # geofence floor+10 m boundary-warning band)
                        0, 0, 0,  # vx, vy, vz cm/s
                        int(math.degrees(yaw) * 100) & 0xFFFF,  # hdg cdeg
                    )
                    # SYS_STATUS (msgid 1) — battery + sensors. pymavlink
                    # v2.0 common dialect exposes the v1 fields (13-arg
                    # signature) — the v2 extension fields (errors_count5)
                    # aren't part of the wire layout PX4 emits either.
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
                    # home-fallback gate. pymavlink v2.0 common dialect
                    # exposes fields: lat, lon, alt, x, y, z, q[4],
                    # approach_x/y/z, time_usec (the Rust codec only decodes
                    # lat/lon/alt/x/y/z — see HomePosition::unpack).
                    if cycle % int(self.telemetry_hz) == 0:
                        self.mav.home_position_send(
                            int(self.lat_e7_start),  # latitude degE7
                            int(self.lon_e7_start),  # longitude degE7
                            500000,                  # altitude mm ASL
                            0.0, 0.0, 0.0,           # x, y, z m NED (home is origin)
                            [1.0, 0.0, 0.0, 0.0],    # q (quaternion, identity)
                            0.0, 0.0, 0.0,           # approach_x, approach_y, approach_z
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
        hb_thread = threading.Thread(target=self.heartbeat_loop, daemon=True)
        tel_thread = threading.Thread(target=self.telemetry_loop, daemon=True)
        hb_thread.start()
        tel_thread.start()
        log(f"listening on {self.listen_addr[0]}:{self.listen_addr[1]} "
            f"sysid={self.sysid} lat_e7={self.lat_e7_start} lon_e7={self.lon_e7_start} "
            f"sending to {self.gcs_addr[0]}:{self.gcs_addr[1]} at {self.telemetry_hz} Hz")
        # Initial state file write (harness ready-signal).
        write_state(self.state_path, instance=self._instance_from_port(),
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
        write_state(self.state_path, instance=self._instance_from_port(),
                    sysid=self.sysid, lat_e7=self.lat_e7_start,
                    lon_e7=self.lon_e7_start, armed_=armed)
        log("shutdown complete")

    def _instance_from_port(self) -> int:
        return self.listen_addr[1] - 14580

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
        global last_command, last_command_param1, last_command_result, command_count, armed
        global param_list_request_count, param_value_send_count, param_set_count
        t = msg.get_type()
        if t == "COMMAND_LONG":
            command = int(msg.command)
            param1 = float(msg.param1)
            with state_lock:
                last_command = command
                last_command_param1 = param1
                last_command_result = mb.MAV_RESULT_ACCEPTED
                command_count += 1
                # Arm/disarm (COMMAND_LONG 400, COMPONENT_ARM_DISARM):
                # param1=1 → arm, param1=0 → disarm. Flip the mock's armed
                # flag so the next HEARTBEAT (1 s) carries the new state to
                # the GCS-side snapshot.
                if command == 400:
                    armed = bool(int(param1))
                    log(f"COMMAND_LONG cmd=400 (ARM_DISARM) param1={param1} "
                        f"-> armed={armed}")
                else:
                    log(f"COMMAND_LONG cmd={command} param1={param1} "
                        f"(acking ACCEPTED)")
            # Send COMMAND_ACK result=0 (ACCEPTED) for every command — PX4's
            # commander accepts the arm/disarm/calibration requests we test
            # against. target_system = GCS manager sysid (255) so the link's
            # ack-correlation code path matches (link.rs:922).
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
            write_state(self.state_path, instance=self._instance_from_port(),
                        sysid=self.sysid, lat_e7=self.lat_e7_start,
                        lon_e7=self.lon_e7_start, armed_=armed)
        elif t == "HEARTBEAT":
            # GCS heartbeat — no action (we already know the GCS is alive).
            pass
        elif t == "PING":
            # Ignore — the link's watchdog probes don't expect a reply here.
            pass
        elif t == "PARAM_REQUEST_LIST":
            # G-8 (Vehicle Setup): the GCS sent a PARAM_REQUEST_LIST
            # (msgid 21). PX4 responds by streaming every param it has
            # as a burst of PARAM_VALUE frames — one per param id, each
            # carrying the right ``param_count`` (total) and ``param_index``
            # (this param's position in the stream). The link's param store
            # flips to ``Complete`` once ``received_unique >= total``.
            with state_lock:
                param_list_request_count += 1
                # Snapshot the store under the lock so the burst loop
                # doesn't race with a concurrent PARAM_SET write.
                snapshot = list(param_store.items())
                total = len(snapshot)
            log(f"PARAM_REQUEST_LIST -> streaming {total} PARAM_VALUE frames")
            # Send each PARAM_VALUE frame. PX4 sends them at full tilt
            # (the link's parser keeps up; the link's recv task buffers
            # in a 2048-byte datagram and processes synchronously). We
            # interleave the heartbeat/telemetry pumps' send_lock so we
            # don't corrupt the encoder's sequence counter.
            for idx, (pid, (wire_value, ptype)) in enumerate(snapshot):
                pid_bytes = pid.encode("ascii", errors="replace")
                try:
                    with send_lock:
                        self.mav.param_value_send(
                            pid_bytes.ljust(16, b"\x00")[:16],
                            wire_value,
                            ptype,
                            total,
                            idx,
                        )
                    with state_lock:
                        param_value_send_count += 1
                except Exception as e:  # pragma: no cover — defensive
                    log(f"param_value_send error (id={pid}): {e}")
            write_state(self.state_path, instance=self._instance_from_port(),
                        sysid=self.sysid, lat_e7=self.lat_e7_start,
                        lon_e7=self.lon_e7_start, armed_=armed)
        elif t == "PARAM_SET":
            # G-8: A parameter write from the GCS's param editor
            # (POST /api/vehicles/{i}/params). PX4's receiver accepts the
            # new value, persists it (autosave), and echoes back a
            # PARAM_VALUE with the new value as confirmation. We model
            # both halves: update the in-memory param store (so a
            # subsequent PARAM_REQUEST_LIST reflects the new value — the
            # same way PX4's autosave makes it visible across reboots),
            # and send the PARAM_VALUE echo so the link's pending_param
            # slot completes (link.rs:set_param_typed awaits the echo).
            raw = msg.param_id
            if isinstance(raw, (bytes, bytearray)):
                param_id_bytes = bytes(raw).rstrip(b"\x00")
            else:
                # pymavlink returns the param_id as a str (latin-1) — encode
                # back to bytes for the wire.
                param_id_bytes = raw.encode("latin-1", errors="replace").rstrip(b"\x00")
            param_id_str = param_id_bytes.decode("ascii", errors="replace")
            value = float(msg.param_value)
            ptype = int(msg.param_type)
            with state_lock:
                # Persist the new value in the mock's param store. The
                # wire f32 we keep is the value PX4 sent — for INT32
                # params this is the integer bit-cast, which is what
                # the link's ingest() will decode back through
                # typed_value().
                param_store[param_id_str] = (value, ptype)
                param_set_count += 1
            try:
                with send_lock:
                    self.mav.param_value_send(
                        param_id_bytes.ljust(16, b"\x00")[:16],
                        value,
                        ptype,
                        len(param_store),
                        # The mock doesn't track per-param index for the
                        # echo; PX4 itself echoes back the param's own
                        # index. The link's pending_param correlation
                        # matches on param_id, not param_index, so 0 is
                        # fine for the wire.
                        0,
                    )
                with state_lock:
                    param_value_send_count += 1
            except Exception as e:  # pragma: no cover
                log(f"param_value_send error: {e}")
            write_state(self.state_path, instance=self._instance_from_port(),
                        sysid=self.sysid, lat_e7=self.lat_e7_start,
                        lon_e7=self.lon_e7_start, armed_=armed)
            log(f"PARAM_SET id={param_id_str!r} value={value} -> PARAM_VALUE echo")
        else:
            # Silent ignore — PX4 drops unknown msgids.
            pass


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def main() -> int:
    ap = argparse.ArgumentParser(description="Mock PX4 for G-5/G-6/G-7/G-8 harnesses")
    ap.add_argument("--port", type=int, default=14580,
                    help="PX4 onboard listen port (default: 14580)")
    ap.add_argument("--gcs-port", type=int, default=14540,
                    help="GCS bind port to send telemetry to (default: 14540)")
    ap.add_argument("--state", default="/tmp/mock_px4_fly_state.json",
                    help="Path to JSON state file (default: %(default)s)")
    ap.add_argument("--instance", type=int, default=None,
                    help="Vehicle instance (default: port - 14580)")
    ap.add_argument("--sysid", type=int, default=None,
                    help="MAVLink sysid (default: instance + 1, PX4's convention)")
    ap.add_argument("--ttl-secs", type=float, default=120.0,
                    help="Auto-shutdown after this many seconds (default: 120)")
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
                   args.state, args.ttl_secs, args.telemetry_hz)

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
