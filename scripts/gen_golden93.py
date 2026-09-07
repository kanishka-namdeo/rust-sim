#!/usr/bin/env python3
"""Regenerate GOLDEN_HIL_ACTUATOR_CONTROLS in the TRUE PX4 v1.16.2 wire layout.

PX4 v1.16.2's pinned dialect packs HIL_ACTUATOR_CONTROLS(93) with size-sorted
core fields (live-captured during I-2 bring-up; see rustsitsim
docs/adr/0015-px4-v116-actuator-layout.md):

    time_usec:u64 @ 0,  flags:u64 @ 8,  controls:f32[16] @ 16,  mode:u8 @ 80

This differs from the current official common.xml (where flags is a trailing
extension field at 73 and controls sit at 8). PX4's own generated headers
(build/px4_sitl_default/mavlink/common/mavlink_msg_hil_actuator_controls.h)
and its live wire behavior are the decoding authority.

CRC_EXTRA for msg 93 is 47 in both dialects. Frame identity matches the other
golden vectors: SYSID=2, COMPID=1, SEQ=3.

Usage:
    python3 scripts/gen_golden93.py   # prints the Rust const line
"""
import struct

SYS_ID, COMP_ID, SEQ, MSG_ID, CRC_EXTRA = 2, 1, 3, 93, 47

# Same field VALUES as the old vector: t=1e6, controls[0..4]=(0.1,-0.2,0.3,-0.4),
# mode=1, flags=0 — only the byte layout changes.
controls = [0.1, -0.2, 0.3, -0.4] + [0.0] * 12

payload = struct.pack("<Q", 1_000_000)        # time_usec @0
payload += struct.pack("<Q", 0)               # flags @8
payload += struct.pack("<16f", *controls)     # controls @16
payload += bytes([1])                         # mode @80 (nonzero -> no trim)
assert len(payload) == 81


def x25crc(data: bytes) -> int:
    crc = 0xFFFF
    for b in data:
        crc ^= b
        for _ in range(8):
            crc = (crc >> 1) ^ 0x8408 if crc & 1 else crc >> 1
    return crc


header = bytes(
    [
        0xFD,
        len(payload),
        0,
        0,
        SEQ,
        SYS_ID,
        COMP_ID,
        MSG_ID & 0xFF,
        (MSG_ID >> 8) & 0xFF,
        (MSG_ID >> 16) & 0xFF,
    ]
)
crc = x25crc(header[1:10] + payload + bytes([CRC_EXTRA]))
frame = header + payload + bytes([crc & 0xFF, (crc >> 8) & 0xFF])

print("pub const GOLDEN_HIL_ACTUATOR_CONTROLS: &[u8] = b\""
      + "".join(f"\\x{b:02x}" for b in frame)
      + "\";")
