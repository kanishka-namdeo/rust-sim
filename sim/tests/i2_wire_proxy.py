#!/usr/bin/env python3
"""I-2 harness wire-format adapter: PX4 v1.16.2 -> sitsim HIL_ACTUATOR_CONTROLS.

WHY THIS EXISTS (diagnosed 2026-09-07, live I-2 run):
  PX4 v1.16.2's bundled dialect (src/modules/mavlink/mavlink/message_definitions/
  v1.0/common.xml, mirrored byte-exactly in the generated
  build/px4_sitl_default/mavlink/common/mavlink_msg_hil_actuator_controls.h)
  packs HIL_ACTUATOR_CONTROLS(93) with size-sorted CORE fields:

      time_usec:u64 @ 0,  flags:u64 @ 8,  controls:float[16] @ 16,  mode:u8 @ 80

  sitsim-mavlink's codec decodes the layout of the current OFFICIAL common.xml
  (where `flags` is a trailing extension field):

      time_usec:u64 @ 0,  controls:float[16] @ 8,  mode:u8 @ 72,  flags:u64 @ 73

  The 8-byte shift has two live-observed consequences (replay of the failed
  run: motor bytes nonzero only at decoded indices 2..5, rotor speeds exactly
  0.0 for all 13436 ticks, truth z pinned at 0 while PX4 logged "Takeoff
  detected"):
    (a) PX4's controls[0..3] (the four [0,1] motor thrusts) land in decoded
        slots 2..5, so the engine's controls[0..3] motor mapping sees two dead
        channels plus PX4 motors 3/4;
    (b) the armed bit (PX4 mode@80 = 0x81) is read from decoded byte 72 —
        actually PX4 controls[14]'s LSB — never 0x80, so the engine believes
        DISARMED and commands all four rotors stopped => zero thrust.

  The engine mapping itself (ADR-0011r: per-motor [0,1] normalized thrust,
  armed = mode & 0x80) is CORRECT for the values PX4 sends once the offsets
  are right; the sitsim-mavlink crate fix is a 3-line offset change plus
  regenerated golden vectors, deliberately left for review. This adapter
  re-lays-out ONLY msgid 93 payloads in the PX4->sim direction and recomputes
  the frame CRC (X.25 over header[1..10] + payload + crc_extra 47, identical
  rule to sitsim-mavlink frame.rs). Everything else, including the whole
  sim->PX4 direction, is forwarded byte-untouched.

Usage: i2_wire_proxy.py <listen_port_for_px4> <sitsim_tcp_port> [log_path]
"""
import socket
import sys
import threading
import time

LISTEN_PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 4560
SIM_PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 4570
LOG = sys.argv[3] if len(sys.argv) > 3 else "/dev/null"

MSG_HIL_ACTUATOR_CONTROLS = 93
CRC_EXTRA_93 = 47
FULL_LEN_93 = 81

stats = {"rewritten": 0, "fwd_px4_to_sim": 0, "fwd_sim_to_px4": 0}
lock = threading.Lock()


def x25crc(data):
    crc = 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0x8408
            else:
                crc >>= 1
    return crc


def parse_frames(buf):
    """Return ([(start, end), ...], consumed) for complete v1/v2 frames."""
    frames = []
    i = 0
    n = len(buf)
    while i < n:
        b = buf[i]
        if b == 0xFD:
            if i + 2 > n:
                break
            total = 12 + buf[i + 1]
            if i + total > n:
                break
            frames.append((i, i + total))
            i += total
        elif b == 0xFE:
            if i + 2 > n:
                break
            total = 10 + buf[i + 1]
            if i + total > n:
                break
            frames.append((i, i + total))
            i += total
        else:
            i += 1  # resync scan
    return frames, i


def rewrite_93(frame):
    """Re-layout a v2 HIL_ACTUATOR_CONTROLS frame PX4 -> sitsim."""
    ln = frame[1]
    payload = frame[10:10 + ln]
    old = payload + b"\x00" * max(0, FULL_LEN_93 - len(payload))
    time_usec = old[0:8]        # same position in both layouts
    flags = old[8:16]           # PX4: u64 core field
    controls = old[16:80]       # PX4: float[16]
    mode = old[80:81]           # PX4: u8 (0x01 | 0x80 when armed)
    newp = bytearray(FULL_LEN_93)
    newp[0:8] = time_usec
    newp[8:72] = controls       # sitsim: controls @ 8
    newp[72:73] = mode          # sitsim: mode   @ 72
    newp[73:81] = flags         # sitsim: flags  @ 73 (unused by the engine)
    head = bytearray(frame[0:10])
    head[1] = FULL_LEN_93
    crc = x25crc(bytes(head[1:10]) + bytes(newp) + bytes([CRC_EXTRA_93]))
    return bytes(head) + bytes(newp) + bytes([crc & 0xFF, (crc >> 8) & 0xFF])


def pump(src, dst, rewrite):
    buf = b""
    try:
        while True:
            chunk = src.recv(65536)
            if not chunk:
                break
            buf += chunk
            spans, consumed = parse_frames(buf)
            out = b""
            for (s, e) in spans:
                f = buf[s:e]
                if rewrite and len(f) >= 10 and f[0] == 0xFD:
                    msgid = f[7] | (f[8] << 8) | (f[9] << 16)
                    if msgid == MSG_HIL_ACTUATOR_CONTROLS:
                        f = rewrite_93(f)
                        with lock:
                            stats["rewritten"] += 1
                    else:
                        with lock:
                            stats["fwd_px4_to_sim"] += 1
                else:
                    with lock:
                        stats["fwd_sim_to_px4"] += 1
                out += f
            buf = buf[consumed:]
            if out:
                dst.sendall(out)
    except OSError:
        pass


def main():
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", LISTEN_PORT))
    listener.listen(1)
    print(f"[proxy] listening 127.0.0.1:{LISTEN_PORT} -> sim 127.0.0.1:{SIM_PORT}", flush=True)

    sim_sock = None
    for _ in range(300):
        try:
            sim_sock = socket.create_connection(("127.0.0.1", SIM_PORT), timeout=2)
            break
        except OSError:
            time.sleep(0.1)
    if sim_sock is None:
        print(f"[proxy] FATAL: could not connect to sim on {SIM_PORT}", flush=True)
        sys.exit(1)
    print("[proxy] connected to sitsim", flush=True)

    px4_sock, addr = listener.accept()
    print(f"[proxy] PX4 connected from {addr}", flush=True)

    def p2s():
        pump(px4_sock, sim_sock, True)
        # PX4 side closed: propagate EOF so the sim ends Px4Disconnected.
        try:
            sim_sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass

    def s2p():
        pump(sim_sock, px4_sock, False)
        try:
            px4_sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass

    t1 = threading.Thread(target=p2s, daemon=True)
    t2 = threading.Thread(target=s2p, daemon=True)
    t1.start()
    t2.start()

    t_end = time.time() + 600
    while time.time() < t_end and (t1.is_alive() or t2.is_alive()):
        time.sleep(2)
        with open(LOG, "a") as fh:
            fh.write(f"t={time.time():.0f} {stats}\n")
    print(f"[proxy] done: {stats}", flush=True)
    try:
        px4_sock.close()
        sim_sock.close()
    except OSError:
        pass


if __name__ == "__main__":
    main()
