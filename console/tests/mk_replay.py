#!/usr/bin/env python3
"""
mk_replay.py — generate a minimal valid .replay file for G-11/G-12 harnesses.

Writes the RustSim v1 replay format documented in `sim/docs/SPEC.md` §8.2:
  - 64-byte header (magic "RSITSIM1", version, header_len, tick_rate_millihz,
    seed, start_t_us, scenario_sha256)
  - N × 96-byte tick records (t_us, 16 motor inputs, 17 state floats,
    fault_flags, CRC-16/X.25)

The state vector layout (per `sim/crates/sitsim-sdk/src/replay.rs`):
  state[0..3]   = pos_ned_m     (x, y, z)
  state[3..6]   = vel_ned_ms    (x, y, z)
  state[6..10]  = q_wxyz        (w, x, y, z)
  state[10..13] = omega_rads    (x, y, z)
  state[13..17] = rotors        (r0, r1, r2, r3)

The harness cares about `pos_ned_m` being monotonically increasing in x
so the G-12 "scrub + monotonic" assertion has a real signal to check.

Usage:
  mk_replay.py <output_path> <num_records> [rate_hz] [seed]
"""
import struct
import sys
import hashlib


def x25_crc(data: bytes) -> int:
    """CRC-16/X.25 — poly 0x8408 (reflected 0x1021), init 0xFFFF, no final XOR.

    Mirrors `sitsim_mavlink::x25_crc` exactly. Reference vectors:
      x25_crc(b"")           == 0xFFFF
      x25_crc(b"123456789")  == 0x6F91
      x25_crc(b"\\x00")       == 0x0F87
    """
    crc = 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0x8408
            else:
                crc >>= 1
    return crc


def encode_header(rate_hz: float, seed: int, start_t_us: int,
                  scenario_hash: bytes) -> bytes:
    assert len(scenario_hash) == 32
    millihz = int(round(rate_hz * 1000.0))
    buf = bytearray(64)
    buf[0:8] = b"RSITSIM1"
    buf[8:10] = struct.pack("<H", 1)            # version
    buf[10:12] = struct.pack("<H", 64)          # header_len
    buf[12:16] = struct.pack("<I", millihz)     # tick rate, millihz
    buf[16:24] = struct.pack("<Q", seed)
    buf[24:32] = struct.pack("<Q", start_t_us)
    buf[32:64] = scenario_hash
    return bytes(buf)


def encode_record(t_us: int, motors: list[float], state: list[float],
                  fault_flags: int) -> bytes:
    assert len(motors) == 16
    assert len(state) == 17
    buf = bytearray(96)
    buf[0:8] = struct.pack("<Q", t_us)
    for i, m in enumerate(motors):
        buf[8 + i] = int(round(max(0.0, min(1.0, m)) * 255.0))
    for i, v in enumerate(state):
        struct.pack_into("<f", buf, 24 + 4 * i, v)
    buf[92:94] = struct.pack("<H", fault_flags)
    crc = x25_crc(bytes(buf[:94]))
    buf[94:96] = struct.pack("<H", crc)
    return bytes(buf)


def main():
    if len(sys.argv) < 3:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    out_path = sys.argv[1]
    n_records = int(sys.argv[2])
    rate_hz = float(sys.argv[3]) if len(sys.argv) > 3 else 200.0
    seed = int(sys.argv[4]) if len(sys.argv) > 4 else 42

    tick_dt_us = int(round(1_000_000 / rate_hz))

    # Scenario hash: a deterministic 32-byte SHA-256 of the parameters
    # (so two runs with the same args produce byte-identical files — the
    # I-5 determinism property the spec calls for).
    h = hashlib.sha256()
    h.update(f"{rate_hz}:{seed}:{n_records}".encode())
    scenario_hash = h.digest()
    assert len(scenario_hash) == 32

    with open(out_path, "wb") as f:
        f.write(encode_header(rate_hz, seed, 0, scenario_hash))
        for tick in range(n_records):
            t_us = tick * tick_dt_us
            # pos_ned_m: x = 0.1 * tick (monotonic), y = 0, z = -0.001 * tick (climbs)
            # vel_ned_ms: x = 0.5 (constant), y = 0, z = -0.05 (climbs)
            # q_wxyz: identity quaternion (w=1, x=y=z=0)
            # omega_rads: zeros
            # rotors: 0.5 (hover throttle) for all 4
            pos = [0.1 * tick, 0.0, -0.001 * tick]
            vel = [0.5, 0.0, -0.05]
            q = [1.0, 0.0, 0.0, 0.0]
            omega = [0.0, 0.0, 0.0]
            rotors = [0.5, 0.5, 0.5, 0.5]
            state = pos + vel + q + omega + rotors
            assert len(state) == 17
            motors = [0.5] * 16
            rec = encode_record(t_us, motors, state, 0)
            f.write(rec)

    print(f"wrote {out_path}: {n_records} records @ {rate_hz} Hz "
          f"({64 + n_records * 96} bytes)", file=sys.stderr)


if __name__ == "__main__":
    main()
