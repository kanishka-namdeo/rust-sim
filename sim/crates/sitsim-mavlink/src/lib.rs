//! Hand-written MAVLink v2 codec for the PX4 SITL HIL lockstep subset.
//!
//! Scope (SPEC §2.2, §3.5): exactly the six messages of the lockstep loop.
//! Owning the codec keeps golden-vector tests byte-exact and avoids pulling
//! in a full generated dialect.
//!
//! Wire format (MAVLink v2, verified byte-exact against pymavlink 2.4.49 and
//! against PX4-Autopilot v1.16.2 generated headers):
//!
//! ```text
//! [0] 0xFD  STX
//! [1] LEN   payload length after trailing-zero truncation
//! [2] INCOMPAT_FLAGS (0; no signature)
//! [3] COMPAT_FLAGS (0)
//! [4] SEQ   per-link rolling sequence number
//! [5] SYSID
//! [6] COMPID
//! [7..10] MSGID little-endian 24-bit
//! [10..10+LEN] PAYLOAD
//! [..]  CRC16 X.25 (little-endian), computed over bytes[1..10] + payload
//!       + one CRC_EXTRA byte seeded per message type.
//! ```
//!
//! The checksum covering the post-STX header (not just the payload) and the
//! payload trailing-zero truncation rule are both verified against the
//! reference implementations (PX4 `mavlink_helpers.h`, pymavlink
//! `MAVLink_message._pack`); see `tests/codec.rs` golden vectors.
//!
//! rustsitsim speaks SYSID 2 / COMPID 1 on the HIL link (matching the
//! prototype bring-up) and accepts any source sysid on receive.

pub mod frame;
pub mod messages;

pub use frame::{Frame, FrameParser};
pub use messages::{
    CommandAck, CommandLong, DecodeError, HilActuatorControls, HilGps, HilSensor,
    HilStateQuaternion, Message,
};

/// rustsitsim's MAVLink system id on the HIL link (SPEC §3.2).
pub const SYS_ID: u8 = 2;
/// rustsitsim's MAVLink component id on the HIL link.
pub const COMP_ID: u8 = 1;
/// MAVLink v2 start-of-frame byte.
pub const STX_V2: u8 = 0xFD;
/// MAVLink v1 start-of-frame byte (never sent; tolerated on receive).
pub const STX_V1: u8 = 0xFE;

/// CRC_EXTRA seeds per message (SPEC §3.5–3.9; values cross-checked against
/// PX4 v1.16.2 `mavlink_msg_*.h` and pymavlink 2.4.49 — see golden vectors).
pub mod crc_extra {
    pub const HIL_SENSOR: u8 = 108;
    pub const HIL_STATE_QUATERNION: u8 = 4;
    pub const HIL_GPS: u8 = 124;
    pub const HIL_ACTUATOR_CONTROLS: u8 = 47;
    pub const COMMAND_LONG: u8 = 152;
    pub const COMMAND_ACK: u8 = 143;
}

/// Message ids of the HIL subset (SPEC §3.5).
pub mod msg_id {
    pub const HIL_SENSOR: u32 = 107;
    pub const HIL_STATE_QUATERNION: u32 = 115;
    pub const HIL_GPS: u32 = 113;
    pub const HIL_ACTUATOR_CONTROLS: u32 = 93;
    pub const COMMAND_LONG: u32 = 76;
    pub const COMMAND_ACK: u32 = 77;
    pub const HEARTBEAT: u32 = 0;
}

/// CRC-16/X.25 (MAVLink variant: init 0xFFFF, reflected poly 0x8408, no
/// final XOR). Bit-exact with PX4 `crc_calculate` / pymavlink `x25crc`.
pub fn x25_crc(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0x8408;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// CRC_EXTRA lookup for the HIL subset; `None` for unknown message ids.
pub fn crc_extra_for(msgid: u32) -> Option<u8> {
    match msgid {
        msg_id::HIL_SENSOR => Some(crc_extra::HIL_SENSOR),
        msg_id::HIL_STATE_QUATERNION => Some(crc_extra::HIL_STATE_QUATERNION),
        msg_id::HIL_GPS => Some(crc_extra::HIL_GPS),
        msg_id::HIL_ACTUATOR_CONTROLS => Some(crc_extra::HIL_ACTUATOR_CONTROLS),
        msg_id::COMMAND_LONG => Some(crc_extra::COMMAND_LONG),
        msg_id::COMMAND_ACK => Some(crc_extra::COMMAND_ACK),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference vector for the X.25 core: "123456789" must give 0x6F91
    /// (verified against pymavlink.mavutil.x25crc on the reference platform;
    /// same algorithm as PX4 crc_calculate()).
    #[test]
    fn x25_crc_reference_vector() {
        assert_eq!(x25_crc(b"123456789"), 0x6F91);
        // Identity of the initial state and a single zero byte.
        assert_eq!(x25_crc(b""), 0xFFFF);
        assert_eq!(x25_crc(b"\x00"), 0x0F87);
    }
}
