//! Message structs for the HIL lockstep subset, with pack/unpack against
//! the MAVLink v2 payload layouts of PX4-Autopilot v1.16.2.
//!
//! Field orders, widths and extension-field boundaries follow the generated
//! PX4 headers (`build/px4_sitl_default/mavlink/common/mavlink_msg_*.h`) and
//! were additionally verified byte-exact against pymavlink 2.4.49 golden
//! vectors (`tests/codec.rs`).
//!
//! Two v1.16.2 subtleties that differ from the naive reading of common.xml:
//! - `HIL_SENSOR.fields_updated` is **uint32** (offset 60, 4 bytes).
//! - `HIL_GPS` carries `fix_type` at offset 34 (after `cog`), and the
//!   extension fields are `id: u8` + `yaw: u16` (cdeg), not u32.

use core::fmt;

fn put_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_i16(buf: &mut [u8], off: usize, v: i16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_i32(buf: &mut [u8], off: usize, v: i32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
fn put_f32(buf: &mut [u8], off: usize, v: f32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn get_u16(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([buf[off], buf[off + 1]])
}
fn get_i16(buf: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([buf[off], buf[off + 1]])
}
fn get_u32(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}
fn get_i32(buf: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}
fn get_u64(buf: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[off..off + 8]);
    u64::from_le_bytes(b)
}
fn get_f32(buf: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// Decode failure with enough context to log the offending frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    pub msgid: u32,
    pub payload_len: usize,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "malformed payload for msgid {} (len {})",
            self.msgid, self.payload_len
        )
    }
}

impl std::error::Error for DecodeError {}

/// HIL_SENSOR (107). SIM -> PX4, 200 Hz default. SPEC §3.6.
///
/// `id` is a MAVLink v2 extension field and MUST be 0: PX4's boot gate is
/// `imu.id == 0` (SimulatorMavlink.cpp:514). `fields_updated` is uint32 in
/// v1.16.2 (see module docs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HilSensor {
    pub time_usec: u64,
    pub xacc: f32,
    pub yacc: f32,
    pub zacc: f32,
    pub xgyro: f32,
    pub ygyro: f32,
    pub zgyro: f32,
    pub xmag: f32,
    pub ymag: f32,
    pub zmag: f32,
    /// Absolute pressure in **hPa** (PX4 multiplies by 100 to Pa).
    pub abs_pressure: f32,
    pub diff_pressure: f32,
    pub pressure_alt: f32,
    pub temperature: f32,
    /// Sensor-freshness bitmask; PX4's SensorSource consumes
    /// ACCEL=0b111 | GYRO=0b111000 | MAG=0b111000000 | BARO=0b1101000000000.
    pub fields_updated: u32,
    /// Sensor instance id; 0 gates PX4 boot (v2 extension field).
    pub id: u8,
}

impl HilSensor {
    pub const FULL_LEN: usize = 65;

    pub fn pack_into(&self, buf: &mut [u8]) {
        assert!(buf.len() >= Self::FULL_LEN, "buffer too small");
        put_u64(buf, 0, self.time_usec);
        put_f32(buf, 8, self.xacc);
        put_f32(buf, 12, self.yacc);
        put_f32(buf, 16, self.zacc);
        put_f32(buf, 20, self.xgyro);
        put_f32(buf, 24, self.ygyro);
        put_f32(buf, 28, self.zgyro);
        put_f32(buf, 32, self.xmag);
        put_f32(buf, 36, self.ymag);
        put_f32(buf, 40, self.zmag);
        put_f32(buf, 44, self.abs_pressure);
        put_f32(buf, 48, self.diff_pressure);
        put_f32(buf, 52, self.pressure_alt);
        put_f32(buf, 56, self.temperature);
        put_u32(buf, 60, self.fields_updated);
        buf[64] = self.id;
    }

    /// `payload` may be shorter than 65 bytes (MAVLink v2 trailing-zero
    /// truncation); missing fields decode as zero.
    pub fn unpack(payload: &[u8], msgid: u32) -> Result<Self, DecodeError> {
        if payload.len() > Self::FULL_LEN {
            return Err(DecodeError { msgid, payload_len: payload.len() });
        }
        let mut buf = [0u8; Self::FULL_LEN];
        buf[..payload.len()].copy_from_slice(payload);
        Ok(Self {
            time_usec: get_u64(&buf, 0),
            xacc: get_f32(&buf, 8),
            yacc: get_f32(&buf, 12),
            zacc: get_f32(&buf, 16),
            xgyro: get_f32(&buf, 20),
            ygyro: get_f32(&buf, 24),
            zgyro: get_f32(&buf, 28),
            xmag: get_f32(&buf, 32),
            ymag: get_f32(&buf, 36),
            zmag: get_f32(&buf, 40),
            abs_pressure: get_f32(&buf, 44),
            diff_pressure: get_f32(&buf, 48),
            pressure_alt: get_f32(&buf, 52),
            temperature: get_f32(&buf, 56),
            fields_updated: get_u32(&buf, 60),
            id: buf[64],
        })
    }
}

/// HIL_STATE_QUATERNION (115). SIM -> PX4, 200 Hz default. SPEC §3.7.
///
/// Unit traps (both verified live): accelerations are **int16 millig**
/// (1 mG = 0.00980665 m/s^2), velocities are **int16 cm/s**, airspeeds are
/// **uint16 cm/s**. The quaternion is w-first, body-to-NED, normalized.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HilStateQuaternion {
    pub time_usec: u64,
    /// w, x, y, z.
    pub attitude_quaternion: [f32; 4],
    pub rollspeed: f32,
    pub pitchspeed: f32,
    pub yawspeed: f32,
    /// degE7.
    pub lat: i32,
    /// degE7.
    pub lon: i32,
    /// MSL, mm.
    pub alt: i32,
    /// NED, cm/s.
    pub vx: i16,
    pub vy: i16,
    /// Down-positive, cm/s.
    pub vz: i16,
    pub ind_airspeed: u16,
    pub true_airspeed: u16,
    /// Body specific force, mG.
    pub xacc: i16,
    pub yacc: i16,
    pub zacc: i16,
}

impl HilStateQuaternion {
    pub const FULL_LEN: usize = 64;

    pub fn pack_into(&self, buf: &mut [u8]) {
        assert!(buf.len() >= Self::FULL_LEN, "buffer too small");
        put_u64(buf, 0, self.time_usec);
        for (i, &q) in self.attitude_quaternion.iter().enumerate() {
            put_f32(buf, 8 + 4 * i, q);
        }
        put_f32(buf, 24, self.rollspeed);
        put_f32(buf, 28, self.pitchspeed);
        put_f32(buf, 32, self.yawspeed);
        put_i32(buf, 36, self.lat);
        put_i32(buf, 40, self.lon);
        put_i32(buf, 44, self.alt);
        put_i16(buf, 48, self.vx);
        put_i16(buf, 50, self.vy);
        put_i16(buf, 52, self.vz);
        put_u16(buf, 54, self.ind_airspeed);
        put_u16(buf, 56, self.true_airspeed);
        put_i16(buf, 58, self.xacc);
        put_i16(buf, 60, self.yacc);
        put_i16(buf, 62, self.zacc);
    }

    pub fn unpack(payload: &[u8], msgid: u32) -> Result<Self, DecodeError> {
        if payload.len() > Self::FULL_LEN {
            return Err(DecodeError { msgid, payload_len: payload.len() });
        }
        // Zero-fill a truncated payload (v2 trailing-zero truncation is
        // value-preserving).
        let mut buf = [0u8; Self::FULL_LEN];
        buf[..payload.len()].copy_from_slice(payload);
        let mut q = [0f32; 4];
        for (i, slot) in q.iter_mut().enumerate() {
            *slot = get_f32(&buf, 8 + 4 * i);
        }
        Ok(Self {
            time_usec: get_u64(&buf, 0),
            attitude_quaternion: q,
            rollspeed: get_f32(&buf, 24),
            pitchspeed: get_f32(&buf, 28),
            yawspeed: get_f32(&buf, 32),
            lat: get_i32(&buf, 36),
            lon: get_i32(&buf, 40),
            alt: get_i32(&buf, 44),
            vx: get_i16(&buf, 48),
            vy: get_i16(&buf, 50),
            vz: get_i16(&buf, 52),
            ind_airspeed: get_u16(&buf, 54),
            true_airspeed: get_u16(&buf, 56),
            xacc: get_i16(&buf, 58),
            yacc: get_i16(&buf, 60),
            zacc: get_i16(&buf, 62),
        })
    }
}

/// HIL_GPS (113). SIM -> PX4, 5 Hz default. SPEC §3.8.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HilGps {
    pub time_usec: u64,
    /// degE7.
    pub lat: i32,
    /// degE7.
    pub lon: i32,
    /// MSL, mm.
    pub alt: i32,
    /// cm.
    pub eph: u16,
    /// cm.
    pub epv: u16,
    /// 3D speed, cm/s.
    pub vel: u16,
    /// NED, cm/s.
    pub vn: i16,
    pub ve: i16,
    pub vd: i16,
    /// Course over ground, centidegrees 0..35999.
    pub cog: u16,
    /// 0 = no fix, 2 = 2D, 3 = 3D.
    pub fix_type: u8,
    pub satellites_visible: u8,
    /// GPS id (v2 extension).
    pub id: u8,
    /// Dual-antenna heading, cdeg (v2 extension; not modeled, 0).
    pub yaw: u16,
}

impl HilGps {
    pub const FULL_LEN: usize = 39;

    pub fn pack_into(&self, buf: &mut [u8]) {
        assert!(buf.len() >= Self::FULL_LEN, "buffer too small");
        put_u64(buf, 0, self.time_usec);
        put_i32(buf, 8, self.lat);
        put_i32(buf, 12, self.lon);
        put_i32(buf, 16, self.alt);
        put_u16(buf, 20, self.eph);
        put_u16(buf, 22, self.epv);
        put_u16(buf, 24, self.vel);
        put_i16(buf, 26, self.vn);
        put_i16(buf, 28, self.ve);
        put_i16(buf, 30, self.vd);
        put_u16(buf, 32, self.cog);
        buf[34] = self.fix_type;
        buf[35] = self.satellites_visible;
        buf[36] = self.id;
        put_u16(buf, 37, self.yaw);
    }

    pub fn unpack(payload: &[u8], msgid: u32) -> Result<Self, DecodeError> {
        if payload.len() > Self::FULL_LEN {
            return Err(DecodeError { msgid, payload_len: payload.len() });
        }
        let mut buf = [0u8; Self::FULL_LEN];
        buf[..payload.len()].copy_from_slice(payload);
        Ok(Self {
            time_usec: get_u64(&buf, 0),
            lat: get_i32(&buf, 8),
            lon: get_i32(&buf, 12),
            alt: get_i32(&buf, 16),
            eph: get_u16(&buf, 20),
            epv: get_u16(&buf, 22),
            vel: get_u16(&buf, 24),
            vn: get_i16(&buf, 26),
            ve: get_i16(&buf, 28),
            vd: get_i16(&buf, 30),
            cog: get_u16(&buf, 32),
            fix_type: buf[34],
            satellites_visible: buf[35],
            id: buf[36],
            yaw: get_u16(&buf, 37),
        })
    }
}

/// HIL_ACTUATOR_CONTROLS (93). PX4 -> SIM. SPEC §3.9.
///
/// PX4 emits normalized outputs in [-1, +1]; the sim maps
/// `u_i = (controls[i] + 1) / 2` (PWM 1000-2000 us convention).
#[derive(Debug, Clone, PartialEq)]
pub struct HilActuatorControls {
    pub time_usec: u64,
    pub controls: [f32; 16],
    /// MAV_MODE_FLAG bits.
    pub mode: u8,
    pub flags: u64,
}

impl HilActuatorControls {
    pub const FULL_LEN: usize = 81;

    pub fn pack_into(&self, buf: &mut [u8]) {
        assert!(buf.len() >= Self::FULL_LEN, "buffer too small");
        put_u64(buf, 0, self.time_usec);
        for (i, &c) in self.controls.iter().enumerate() {
            put_f32(buf, 8 + 4 * i, c);
        }
        buf[72] = self.mode;
        put_u64(buf, 73, self.flags);
    }

    pub fn unpack(payload: &[u8], msgid: u32) -> Result<Self, DecodeError> {
        if payload.len() > Self::FULL_LEN {
            return Err(DecodeError { msgid, payload_len: payload.len() });
        }
        // Zero-fill a truncated payload (v2 trailing-zero truncation is
        // value-preserving; PX4 itself sends HIL_ACTUATOR_CONTROLS with the
        // zero `flags` tail trimmed).
        let mut buf = [0u8; Self::FULL_LEN];
        buf[..payload.len()].copy_from_slice(payload);
        let mut controls = [0f32; 16];
        for (i, slot) in controls.iter_mut().enumerate() {
            *slot = get_f32(&buf, 8 + 4 * i);
        }
        Ok(Self {
            time_usec: get_u64(&buf, 0),
            controls,
            mode: buf[72],
            flags: get_u64(&buf, 73),
        })
    }
}

/// COMMAND_LONG (76). PX4 -> SIM (rate negotiation). SPEC §3.4.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandLong {
    pub param1: f32,
    pub param2: f32,
    pub param3: f32,
    pub param4: f32,
    pub param5: f32,
    pub param6: f32,
    pub param7: f32,
    pub command: u16,
    pub target_system: u8,
    pub target_component: u8,
    pub confirmation: u8,
}

impl CommandLong {
    pub const FULL_LEN: usize = 33;

    pub fn pack_into(&self, buf: &mut [u8]) {
        assert!(buf.len() >= Self::FULL_LEN, "buffer too small");
        put_f32(buf, 0, self.param1);
        put_f32(buf, 4, self.param2);
        put_f32(buf, 8, self.param3);
        put_f32(buf, 12, self.param4);
        put_f32(buf, 16, self.param5);
        put_f32(buf, 20, self.param6);
        put_f32(buf, 24, self.param7);
        put_u16(buf, 28, self.command);
        buf[30] = self.target_system;
        buf[31] = self.target_component;
        buf[32] = self.confirmation;
    }

    pub fn unpack(payload: &[u8], msgid: u32) -> Result<Self, DecodeError> {
        if payload.len() > Self::FULL_LEN {
            return Err(DecodeError { msgid, payload_len: payload.len() });
        }
        let mut buf = [0u8; Self::FULL_LEN];
        buf[..payload.len()].copy_from_slice(payload);
        Ok(Self {
            param1: get_f32(&buf, 0),
            param2: get_f32(&buf, 4),
            param3: get_f32(&buf, 8),
            param4: get_f32(&buf, 12),
            param5: get_f32(&buf, 16),
            param6: get_f32(&buf, 20),
            param7: get_f32(&buf, 24),
            command: get_u16(&buf, 28),
            target_system: buf[30],
            target_component: buf[31],
            confirmation: buf[32],
        })
    }
}

/// COMMAND_ACK (77). SIM -> PX4 (rate negotiation reply). SPEC §3.4.
///
/// `result`: 0 = ACCEPTED, 3 = UNSUPPORTED (see MAV_RESULT enum).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandAck {
    pub command: u16,
    pub result: u8,
    /// Extension: progress %, 0 when unused.
    pub progress: u8,
    /// Extension.
    pub result_param2: i32,
    /// Extension: target of the acked command (PX4's sysid).
    pub target_system: u8,
    /// Extension.
    pub target_component: u8,
}

impl CommandAck {
    pub const FULL_LEN: usize = 10;

    /// Minimal (v1-compatible, all-zero-extension) ack.
    pub fn new(command: u16, result: u8) -> Self {
        Self {
            command,
            result,
            progress: 0,
            result_param2: 0,
            target_system: 0,
            target_component: 0,
        }
    }

    pub fn pack_into(&self, buf: &mut [u8]) {
        assert!(buf.len() >= Self::FULL_LEN, "buffer too small");
        put_u16(buf, 0, self.command);
        buf[2] = self.result;
        buf[3] = self.progress;
        put_i32(buf, 4, self.result_param2);
        buf[8] = self.target_system;
        buf[9] = self.target_component;
    }

    pub fn unpack(payload: &[u8], msgid: u32) -> Result<Self, DecodeError> {
        if payload.len() > Self::FULL_LEN {
            return Err(DecodeError { msgid, payload_len: payload.len() });
        }
        let mut buf = [0u8; Self::FULL_LEN];
        buf[..payload.len()].copy_from_slice(payload);
        Ok(Self {
            command: get_u16(&buf, 0),
            result: buf[2],
            progress: buf[3],
            result_param2: get_i32(&buf, 4),
            target_system: buf[8],
            target_component: buf[9],
        })
    }
}

/// The full set of messages rustsitsim exchanges on the HIL link.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    HilSensor(HilSensor),
    HilStateQuaternion(HilStateQuaternion),
    HilGps(HilGps),
    HilActuatorControls(HilActuatorControls),
    CommandLong(CommandLong),
    CommandAck(CommandAck),
}

impl Message {
    /// Message id (24-bit, as carried in the frame header).
    pub fn msgid(&self) -> u32 {
        match self {
            Message::HilSensor(_) => super::msg_id::HIL_SENSOR,
            Message::HilStateQuaternion(_) => super::msg_id::HIL_STATE_QUATERNION,
            Message::HilGps(_) => super::msg_id::HIL_GPS,
            Message::HilActuatorControls(_) => super::msg_id::HIL_ACTUATOR_CONTROLS,
            Message::CommandLong(_) => super::msg_id::COMMAND_LONG,
            Message::CommandAck(_) => super::msg_id::COMMAND_ACK,
        }
    }

    /// Full (un-truncated) payload length.
    pub fn full_len(&self) -> usize {
        match self {
            Message::HilSensor(_) => HilSensor::FULL_LEN,
            Message::HilStateQuaternion(_) => HilStateQuaternion::FULL_LEN,
            Message::HilGps(_) => HilGps::FULL_LEN,
            Message::HilActuatorControls(_) => HilActuatorControls::FULL_LEN,
            Message::CommandLong(_) => CommandLong::FULL_LEN,
            Message::CommandAck(_) => CommandAck::FULL_LEN,
        }
    }

    /// Pack into a freshly allocated payload buffer, applying MAVLink v2
    /// trailing-zero truncation (identical rule to pymavlink: drop trailing
    /// zero bytes while more than one byte remains). Decoders zero-fill, so
    /// this is value-preserving.
    pub fn pack_payload(&self) -> Vec<u8> {
        let mut full = vec![0u8; self.full_len()];
        match self {
            Message::HilSensor(m) => m.pack_into(&mut full),
            Message::HilStateQuaternion(m) => m.pack_into(&mut full),
            Message::HilGps(m) => m.pack_into(&mut full),
            Message::HilActuatorControls(m) => m.pack_into(&mut full),
            Message::CommandLong(m) => m.pack_into(&mut full),
            Message::CommandAck(m) => m.pack_into(&mut full),
        }
        let mut plen = full.len();
        while plen > 1 && full[plen - 1] == 0 {
            plen -= 1;
        }
        full.truncate(plen);
        full
    }

    /// Decode a payload into a typed message (zero-filling semantics for
    /// truncated payloads).
    pub fn unpack_payload(msgid: u32, payload: &[u8]) -> Result<Message, DecodeError> {
        match msgid {
            super::msg_id::HIL_SENSOR => Ok(Message::HilSensor(HilSensor::unpack(payload, msgid)?)),
            super::msg_id::HIL_STATE_QUATERNION => Ok(Message::HilStateQuaternion(
                HilStateQuaternion::unpack(payload, msgid)?,
            )),
            super::msg_id::HIL_GPS => Ok(Message::HilGps(HilGps::unpack(payload, msgid)?)),
            super::msg_id::HIL_ACTUATOR_CONTROLS => Ok(Message::HilActuatorControls(
                HilActuatorControls::unpack(payload, msgid)?,
            )),
            super::msg_id::COMMAND_LONG => Ok(Message::CommandLong(CommandLong::unpack(
                payload, msgid,
            )?)),
            super::msg_id::COMMAND_ACK => Ok(Message::CommandAck(CommandAck::unpack(
                payload, msgid,
            )?)),
            _ => Err(DecodeError { msgid, payload_len: payload.len() }),
        }
    }
}
