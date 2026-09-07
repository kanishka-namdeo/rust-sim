//! PX4 flight-mode encoding for MAVLink `DO_SET_MODE` (MAV_CMD 176) and the
//! offboard `SET_POSITION_TARGET_LOCAL_NED` type_mask.
//!
//! # V-11 provenance (spec `docs/SPEC.md` §3.2)
//!
//! The constant tables below were **verified against the pinned PX4 v1.16.2
//! source tree** at `PX4-Autopilot/src/modules/commander/px4_custom_mode.h`
//! (the same file the spec's V-11 names). Two findings worth recording:
//!
//! * The main/auto-sub-mode *values* match the spec table exactly
//!   (MANUAL=1, ALTCTL=2, POSCTL=3, AUTO=4, ACRO=5, OFFBOARD=6, STABILIZED=7;
//!   AUTO sub-modes READY=1, TAKEOFF=2, LOITER=3, MISSION=4, RTL=5, LAND=6).
//! * The custom-mode *word layout* in the pinned header is
//!   `data = (main_mode << 16) | (sub_mode << 24)`: the PX4
//!   `union px4_custom_mode { uint16_t reserved; uint8_t main_mode;
//!   uint8_t sub_mode; }` places `reserved` in bytes 0-1, `main_mode` in
//!   byte 2 and `sub_mode` in byte 3 (little-endian). **The spec's §3.2
//!   formula "custom = (main << 8) or sub" is wrong**; this crate implements
//!   the header-verified layout, which is also exactly how pymavlink's
//!   `interpret_px4_mode` decodes heartbeats (`main = (custom & 0xFF0000) >>
//!   16`, `sub = (custom & 0xFF000000) >> 24`).
//!   See `docs/adr/0005-mode-word-layout.md`. The unit test
//!   `test_constants_match_pinned_header` pins the numbers so drift fails the
//!   build, which is the V-11 discipline.
//!
//! # V-12 provenance (type_mask)
//!
//! `POSITION_ONLY` / `VELOCITY_YAWRATE` are encoded per the semantics in
//! spec §3.3: position setpoints with yaw (ignore velocity, accel, and
//! yaw-rate — PX4's flight-mode-manager uses yaw only when the yaw-rate
//! ignore bit is set), and velocity setpoints with yaw_rate (ignore
//! position, accel, and yaw). A 30 s physical offboard hold against a
//! dynamics-grade simulator (the real V-12 verification) is deferred to the
//! rustsitsim integration phase; see ADR-0001.

#![forbid(unsafe_code)]

/// MAVLink base_mode flag: custom mode in use (required by PX4 DO_SET_MODE).
pub const MAV_MODE_FLAG_CUSTOM_ENABLED: u8 = 1;
/// MAVLink base_mode flag: armed.
pub const MAV_MODE_FLAG_SAFETY_ARMED: u8 = 128;

/// PX4 `PX4_CUSTOM_MAIN_MODE_*` (verified against the pinned header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MainMode {
    Manual = 1,
    Altctl = 2,
    Posctl = 3,
    Auto = 4,
    Acro = 5,
    Offboard = 6,
    Stabilized = 7,
}

/// PX4 `PX4_CUSTOM_SUB_MODE_AUTO_*` (verified against the pinned header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AutoSubMode {
    Ready = 1,
    Takeoff = 2,
    Loiter = 3,
    Mission = 4,
    Rtl = 5,
    Land = 6,
}

/// Assemble the 32-bit custom-mode word per the pinned header union layout:
/// `(main << 16) | (sub << 24)` (bytes 0-1 are `reserved`).
pub const fn mode_word(main: MainMode, sub: Option<AutoSubMode>) -> u32 {
    let sub = match sub {
        Some(s) => s as u32,
        None => 0,
    };
    ((main as u32) << 16) | (sub << 24)
}

/// Decode a custom-mode word back to `(main, sub)`.
pub fn decode_mode_word(word: u32) -> (u8, u8) {
    (((word >> 16) & 0xff) as u8, ((word >> 24) & 0xff) as u8)
}

/// Mode words used by the manager (spec §3.2: POSCTL, AUTO RTL, AUTO LAND,
/// OFFBOARD only).
pub const MODE_WORD_POSCTL: u32 = mode_word(MainMode::Posctl, None);
pub const MODE_WORD_AUTO_RTL: u32 = mode_word(MainMode::Auto, Some(AutoSubMode::Rtl));
pub const MODE_WORD_AUTO_LAND: u32 = mode_word(MainMode::Auto, Some(AutoSubMode::Land));
pub const MODE_WORD_AUTO_LOITER: u32 = mode_word(MainMode::Auto, Some(AutoSubMode::Loiter));
pub const MODE_WORD_OFFBOARD: u32 = mode_word(MainMode::Offboard, None);

/// Decode a custom-mode word into a human-readable PX4 mode name.
pub fn mode_name(word: u32) -> String {
    let (main, sub) = decode_mode_word(word);
    let main = match main {
        1 => "MANUAL",
        2 => "ALTCTL",
        3 => "POSCTL",
        4 => "AUTO",
        5 => "ACRO",
        6 => "OFFBOARD",
        7 => "STABILIZED",
        _ => return format!("UNKNOWN({main})"),
    };
    if main == "AUTO" {
        let sub = match sub {
            1 => "READY",
            2 => "TAKEOFF",
            3 => "LOITER",
            4 => "MISSION",
            5 => "RTL",
            6 => "LAND",
            _ => return format!("AUTO.S{sub}"),
        };
        format!("AUTO.{sub}")
    } else {
        main.to_string()
    }
}

// ---------------------------------------------------------------------------
// SET_POSITION_TARGET_LOCAL_NED type_mask (spec §3.3, V-12)
// ---------------------------------------------------------------------------

pub const TM_X_IGNORE: u16 = 1 << 0;
pub const TM_Y_IGNORE: u16 = 1 << 1;
pub const TM_Z_IGNORE: u16 = 1 << 2;
pub const TM_VX_IGNORE: u16 = 1 << 3;
pub const TM_VY_IGNORE: u16 = 1 << 4;
pub const TM_VZ_IGNORE: u16 = 1 << 5;
pub const TM_AX_IGNORE: u16 = 1 << 6;
pub const TM_AY_IGNORE: u16 = 1 << 7;
pub const TM_AZ_IGNORE: u16 = 1 << 8;
pub const TM_FORCE: u16 = 1 << 9;
pub const TM_YAW_IGNORE: u16 = 1 << 10;
pub const TM_YAWRATE_IGNORE: u16 = 1 << 11;

/// Position setpoint with yaw: ignore velocity, accel and yaw_rate
/// (bits 4-9 and 12 in 1-based spec numbering).
pub const POSITION_ONLY: u16 =
    TM_VX_IGNORE | TM_VY_IGNORE | TM_VZ_IGNORE | TM_AX_IGNORE | TM_AY_IGNORE | TM_AZ_IGNORE | TM_YAWRATE_IGNORE;

/// Velocity setpoint with yaw_rate: ignore position, accel and yaw.
pub const VELOCITY_YAWRATE: u16 =
    TM_X_IGNORE | TM_Y_IGNORE | TM_Z_IGNORE | TM_AX_IGNORE | TM_AY_IGNORE | TM_AZ_IGNORE | TM_YAW_IGNORE;

#[cfg(test)]
mod tests {
    use super::*;

    /// V-11: exact values from the pinned PX4 v1.16.2
    /// `src/modules/commander/px4_custom_mode.h`.
    #[test]
    fn test_constants_match_pinned_header() {
        assert_eq!(MainMode::Manual as u8, 1);
        assert_eq!(MainMode::Altctl as u8, 2);
        assert_eq!(MainMode::Posctl as u8, 3);
        assert_eq!(MainMode::Auto as u8, 4);
        assert_eq!(MainMode::Acro as u8, 5);
        assert_eq!(MainMode::Offboard as u8, 6);
        assert_eq!(MainMode::Stabilized as u8, 7);
        assert_eq!(AutoSubMode::Ready as u8, 1);
        assert_eq!(AutoSubMode::Takeoff as u8, 2);
        assert_eq!(AutoSubMode::Loiter as u8, 3);
        assert_eq!(AutoSubMode::Mission as u8, 4);
        assert_eq!(AutoSubMode::Rtl as u8, 5);
        assert_eq!(AutoSubMode::Land as u8, 6);
    }

    #[test]
    fn test_mode_word_layout_is_header_union_layout() {
        // union { u16 reserved; u8 main; u8 sub; }: main = byte 2,
        // sub = byte 3 (little-endian) -> sub is the TOP byte.
        assert_eq!(MODE_WORD_POSCTL, 0x0003_0000);
        assert_eq!(MODE_WORD_AUTO_RTL, 0x0504_0000);
        assert_eq!(MODE_WORD_AUTO_LAND, 0x0604_0000);
        assert_eq!(MODE_WORD_AUTO_LOITER, 0x0304_0000);
        assert_eq!(MODE_WORD_OFFBOARD, 0x0006_0000);
        // and it matches pymavlink's interpret_px4_mode decoding:
        // main = (w & 0xFF0000) >> 16, sub = (w & 0xFF000000) >> 24.
        for (w, main, sub) in [
            (MODE_WORD_POSCTL, 3u32, 0u32),
            (MODE_WORD_AUTO_RTL, 4, 5),
            (MODE_WORD_AUTO_LAND, 4, 6),
            (MODE_WORD_AUTO_LOITER, 4, 3),
            (MODE_WORD_OFFBOARD, 6, 0),
        ] {
            assert_eq!((w & 0xFF0000) >> 16, main);
            assert_eq!((w & 0xFF000000) >> 24, sub);
        }
    }

    #[test]
    fn test_mode_word_roundtrip() {
        for (main, sub) in [
            (MainMode::Offboard, None),
            (MainMode::Posctl, None),
            (MainMode::Auto, Some(AutoSubMode::Rtl)),
            (MainMode::Auto, Some(AutoSubMode::Land)),
            (MainMode::Auto, Some(AutoSubMode::Loiter)),
        ] {
            let w = mode_word(main, sub);
            let (m, s) = decode_mode_word(w);
            assert_eq!(m, main as u8);
            assert_eq!(s, sub.map(|x| x as u8).unwrap_or(0));
        }
    }

    #[test]
    fn test_mode_names() {
        assert_eq!(mode_name(MODE_WORD_OFFBOARD), "OFFBOARD");
        assert_eq!(mode_name(MODE_WORD_AUTO_RTL), "AUTO.RTL");
        assert_eq!(mode_name(MODE_WORD_AUTO_LAND), "AUTO.LAND");
        assert_eq!(mode_name(MODE_WORD_POSCTL), "POSCTL");
    }

    #[test]
    fn test_type_mask_constants() {
        // Position mode: velocity (bits 4-6) + accel (7-9) + yaw_rate (12)
        // ignored; position + yaw active.
        assert_eq!(POSITION_ONLY & (TM_X_IGNORE | TM_Y_IGNORE | TM_Z_IGNORE), 0);
        assert_eq!(POSITION_ONLY & TM_YAW_IGNORE, 0);
        assert_ne!(POSITION_ONLY & TM_YAWRATE_IGNORE, 0);
        assert_ne!(POSITION_ONLY & (TM_VX_IGNORE | TM_VY_IGNORE | TM_VZ_IGNORE), 0);
        assert_ne!(POSITION_ONLY & (TM_AX_IGNORE | TM_AY_IGNORE | TM_AZ_IGNORE), 0);
        // Velocity mode: position + accel + yaw ignored; velocity + yaw_rate active.
        assert_eq!(VELOCITY_YAWRATE & (TM_VX_IGNORE | TM_VY_IGNORE | TM_VZ_IGNORE), 0);
        assert_eq!(VELOCITY_YAWRATE & TM_YAWRATE_IGNORE, 0);
        assert_ne!(VELOCITY_YAWRATE & (TM_X_IGNORE | TM_Y_IGNORE | TM_Z_IGNORE), 0);
        assert_ne!(VELOCITY_YAWRATE & TM_YAW_IGNORE, 0);
        assert_ne!(VELOCITY_YAWRATE & (TM_AX_IGNORE | TM_AY_IGNORE | TM_AZ_IGNORE), 0);
        // Neither mask uses the force bit.
        assert_eq!(POSITION_ONLY & TM_FORCE, 0);
        assert_eq!(VELOCITY_YAWRATE & TM_FORCE, 0);
    }
}
