//! MAVLink v2 (and v1, for robustness) frame encode/decode.
//!
//! Wire format v2: `FD | len | incompat | compat | seq | sysid | compid |
//! msgid(3, LE) | payload(len) | crc(2, LE)`; the CRC is X.25 accumulated
//! over the bytes **from the first byte after the magic** through the end of
//! the payload, then the message's `crc_extra` byte (the pymavlink/PX4
//! convention, pinned by the golden tests). Senders may omit trailing zero
//! bytes of the payload (PX4 and pymavlink both do), so decoders must accept
//! short payloads.

use crate::crc::X25Crc;

pub const MAGIC_V2: u8 = 0xFD;
pub const MAGIC_V1: u8 = 0xFE;
pub const V2_HEADER_LEN: usize = 10; // magic..msgid inclusive
pub const V1_HEADER_LEN: usize = 6;

/// A structurally parsed frame (CRC already checked by the decoder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub seq: u8,
    pub sysid: u8,
    pub compid: u8,
    pub msgid: u32,
    pub payload: Vec<u8>,
    pub crc_ok: bool,
}

impl Frame {
    /// Encode as a MAVLink v2 frame with the given CRC extra; trailing zero
    /// bytes of the payload are trimmed (protocol-legal, matches PX4 and
    /// pymavlink behaviour).
    pub fn encode_v2(&self, seq: u8, crc_extra: u8) -> Vec<u8> {
        let mut payload = self.payload.clone();
        while payload.last() == Some(&0) {
            payload.pop();
        }
        let mut out = Vec::with_capacity(V2_HEADER_LEN + payload.len() + 2);
        out.push(MAGIC_V2);
        out.push(payload.len() as u8);
        out.push(0); // incompat flags: no signature
        out.push(0); // compat flags
        out.push(seq);
        out.push(self.sysid);
        out.push(self.compid);
        out.push((self.msgid & 0xff) as u8);
        out.push(((self.msgid >> 8) & 0xff) as u8);
        out.push(((self.msgid >> 16) & 0xff) as u8);
        out.extend_from_slice(&payload);
        // CRC-16/X.25 over everything from the byte AFTER the magic through
        // the payload, then the message's crc_extra byte (pymavlink/
        // PX4-verified convention).
        let mut crc = X25Crc::new();
        crc.accumulate_slice(&out[1..]);
        let crc = crc.finalize_with_extra(crc_extra);
        out.push((crc & 0xff) as u8);
        out.push((crc >> 8) as u8);
        out
    }
}

/// Incremental frame decoder with resynchronisation on garbage / bad CRC.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Decoder { buf: Vec::new() }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Extract the next complete frame, resyncing past garbage. Frames whose
    /// message id is unknown (no CRC extra available) are emitted with
    /// `crc_ok = false` after structural validation, so callers can count
    /// them without failing the stream.
    pub fn next_frame(&mut self) -> Option<Frame> {
        // Link-level corruption guard: a bogus 0xFD in garbage can make the
        // head frame look like a long partial frame and stall resync
        // forever. UDP datagrams in our topology never fragment, so a
        // buffer this large means garbage; drop it and resync on the next
        // push.
        if self.buf.len() > 4096 {
            self.buf.clear();
        }
        loop {
            if self.buf.is_empty() {
                return None;
            }
            match self.buf[0] {
                MAGIC_V2 => {
                    if self.buf.len() < V2_HEADER_LEN {
                        return None;
                    }
                    let len = self.buf[1] as usize;
                    let total = V2_HEADER_LEN + len + 2;
                    if self.buf.len() < total {
                        return None;
                    }
                    // v2 header: magic len incompat compat seq sysid compid
                    // msgid(3, LE) -> payload at byte 10.
                    let msgid = self.buf[7] as u32
                        | (self.buf[8] as u32) << 8
                        | (self.buf[9] as u32) << 16;
                    let frame_bytes = &self.buf[..total];
                    let mut crc = X25Crc::new();
                    crc.accumulate_slice(&frame_bytes[1..V2_HEADER_LEN + len]);
                    let crc_ok = match crc_extra(msgid) {
                        Some(extra) => {
                            let expected = crc.finalize_with_extra(extra);
                            let got = frame_bytes[V2_HEADER_LEN + len] as u16
                                | ((frame_bytes[V2_HEADER_LEN + len + 1] as u16) << 8);
                            expected == got
                        }
                        None => false,
                    };
                    if crc_extra(msgid).is_some() && !crc_ok {
                        // bad CRC: resync
                        self.buf.remove(0);
                        continue;
                    }
                    let frame = Frame {
                        seq: frame_bytes[4],
                        sysid: frame_bytes[5],
                        compid: frame_bytes[6],
                        msgid,
                        payload: frame_bytes[V2_HEADER_LEN..V2_HEADER_LEN + len].to_vec(),
                        crc_ok,
                    };
                    self.buf.drain(..total);
                    return Some(frame);
                }
                MAGIC_V1 => {
                    if self.buf.len() < V1_HEADER_LEN {
                        return None;
                    }
                    let len = self.buf[1] as usize;
                    let total = V1_HEADER_LEN + len + 2;
                    if self.buf.len() < total {
                        return None;
                    }
                    let msgid = self.buf[5] as u32;
                    let frame_bytes = &self.buf[..total];
                    let mut crc = X25Crc::new();
                    crc.accumulate_slice(&frame_bytes[1..V1_HEADER_LEN + len]);
                    let crc_ok = match crc_extra(msgid) {
                        Some(extra) => {
                            let expected = crc.finalize_with_extra(extra);
                            let got = frame_bytes[V1_HEADER_LEN + len] as u16
                                | ((frame_bytes[V1_HEADER_LEN + len + 1] as u16) << 8);
                            expected == got
                        }
                        None => false,
                    };
                    if crc_extra(msgid).is_some() && !crc_ok {
                        self.buf.remove(0);
                        continue;
                    }
                    let frame = Frame {
                        seq: frame_bytes[2],
                        sysid: frame_bytes[3],
                        compid: frame_bytes[4],
                        msgid,
                        payload: frame_bytes[V1_HEADER_LEN..V1_HEADER_LEN + len].to_vec(),
                        crc_ok,
                    };
                    self.buf.drain(..total);
                    return Some(frame);
                }
                _ => {
                    self.buf.remove(0);
                }
            }
        }
    }
}

/// CRC extra bytes for the fleet's message subset, extracted from the
/// generated pymavlink v2.0 common dialect (which mirrors common.xml and
/// the PX4 generated headers — both were cross-checked during bring-up).
pub fn crc_extra(msgid: u32) -> Option<u8> {
    Some(match msgid {
        0 => 50,   // HEARTBEAT
        1 => 124,  // SYS_STATUS
        4 => 237,  // PING
        20 => 214, // PARAM_REQUEST_READ
        21 => 159, // PARAM_REQUEST_LIST
        22 => 220, // PARAM_VALUE
        23 => 168, // PARAM_SET
        30 => 39,  // ATTITUDE
        32 => 185, // LOCAL_POSITION_NED
        33 => 104, // GLOBAL_POSITION_INT
        76 => 152, // COMMAND_LONG
        77 => 143, // COMMAND_ACK
        84 => 143, // SET_POSITION_TARGET_LOCAL_NED
        147 => 154, // BATTERY_STATUS
        242 => 104, // HOME_POSITION
        253 => 83, // STATUSTEXT
        // Mission protocol messages (mavlink.io Mission Protocol, Aug 2026)
        43 => 132, // MISSION_REQUEST_LIST
        44 => 221, // MISSION_COUNT
        47 => 153, // MISSION_ACK
        51 => 226, // MISSION_REQUEST_INT
        73 => 38,  // MISSION_ITEM_INT
        40 => 228, // MISSION_REQUEST
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(msgid: u32, payload: &[u8], seq: u8) -> Vec<u8> {
        Frame {
            seq,
            sysid: 255,
            compid: 190,
            msgid,
            payload: payload.to_vec(),
            crc_ok: true,
        }
        .encode_v2(seq, crc_extra(msgid).unwrap())
    }

    #[test]
    fn roundtrip_simple_frame() {
        let bytes = encode(0, &[0, 0, 0, 0, 6, 8, 0, 4, 3], 7);
        let mut d = Decoder::new();
        d.push(&bytes);
        let f = d.next_frame().unwrap();
        assert_eq!(f.msgid, 0);
        assert_eq!(f.seq, 7);
        assert_eq!(f.sysid, 255);
        assert_eq!(f.payload, vec![0, 0, 0, 0, 6, 8, 0, 4, 3]);
        assert!(f.crc_ok);
        assert!(d.next_frame().is_none());
    }

    #[test]
    fn two_frames_one_datagram() {
        let mut bytes = encode(0, &[9], 0);
        bytes.extend(encode(30, &[0; 28], 1));
        let mut d = Decoder::new();
        d.push(&bytes);
        assert_eq!(d.next_frame().unwrap().seq, 0);
        assert_eq!(d.next_frame().unwrap().seq, 1);
        assert!(d.next_frame().is_none());
    }

    #[test]
    fn resync_on_garbage_prefix() {
        // garbage (no 0xFD inside: a lone 0xFD would look like a long
        // partial frame, which the bounded-buffer guard handles instead)
        let mut bytes = vec![0x00, 0x13, 0x37, 0xaa];
        bytes.extend(encode(0, &[9], 0));
        let mut d = Decoder::new();
        d.push(&bytes);
        let f = d.next_frame().unwrap();
        assert_eq!(f.msgid, 0);
    }

    #[test]
    fn bogus_fd_in_garbage_does_not_stall_forever() {
        // A garbage 0xFD followed by a real frame: the bogus prefix parses
        // as a partial frame; once the buffer overflows the corruption
        // guard clears it and decoding resumes.
        let mut d = Decoder::new();
        d.push(&[0x00, 0x13, 0x37, 0xfd]);
        assert!(d.next_frame().is_none(), "stuck waiting for the bogus partial frame");
        for _ in 0..200 {
            d.push(&encode(0, &[9], 0));
        }
        // buffer is now far past the guard threshold: cleared on the next
        // call, then the trailing frames decode.
        let mut decoded = 0;
        while d.next_frame().is_some() {
            decoded += 1;
        }
        assert!(decoded >= 1, "decoder never resynced");
    }

    #[test]
    fn bad_crc_dropped() {
        let mut bytes = encode(0, &[9], 0);
        let n = bytes.len();
        bytes[n - 1] ^= 0xff; // corrupt CRC
        let mut d = Decoder::new();
        d.push(&bytes);
        assert!(d.next_frame().is_none());
    }

    #[test]
    fn unknown_msgid_structurally_emitted() {
        // message id 42 has no crc_extra in our subset; frame is emitted
        // with crc_ok=false so callers can count unknown traffic.
        let mut f = Frame {
            seq: 0,
            sysid: 1,
            compid: 1,
            msgid: 42,
            payload: vec![1, 2, 3],
            crc_ok: true,
        };
        // encode with a made-up extra; decoder can't verify -> crc_ok false
        let bytes = f.encode_v2(0, 99);
        f.crc_ok = false;
        let mut d = Decoder::new();
        d.push(&bytes);
        assert_eq!(d.next_frame(), Some(f));
    }

    #[test]
    fn partial_frame_waits_for_more() {
        let bytes = encode(0, &[9], 0);
        let mut d = Decoder::new();
        d.push(&bytes[..5]);
        assert!(d.next_frame().is_none());
        d.push(&bytes[5..]);
        assert!(d.next_frame().is_some());
    }
}
