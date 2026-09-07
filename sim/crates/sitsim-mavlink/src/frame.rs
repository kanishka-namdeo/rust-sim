//! MAVLink v2 framing: frame encode/decode and an incremental stream parser.

use crate::messages::{DecodeError, Message};
use crate::{crc_extra_for, x25_crc, STX_V1, STX_V2};

/// A fully validated MAVLink v2 frame.
///
/// On receive, the CRC has already been verified by [`FrameParser`]; on send,
/// `encode()` computes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub seq: u8,
    pub sysid: u8,
    pub compid: u8,
    pub msgid: u32,
    /// Payload with the sender's trailing-zero truncation still applied;
    /// message decoders zero-fill missing tail bytes.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Build a wire frame from a typed message. `seq` is the per-link rolling
    /// sequence number owned by the sender.
    pub fn from_message(msg: &Message, seq: u8, sysid: u8, compid: u8) -> Frame {
        Frame {
            seq,
            sysid,
            compid,
            msgid: msg.msgid(),
            payload: msg.pack_payload(),
        }
    }

    /// Serialize to wire bytes (including CRC).
    ///
    /// The CRC covers the 9 header bytes after STX (LEN, flags, seq, ids,
    /// msgid) + payload + CRC_EXTRA, matching PX4 `mavlink_helpers.h`
    /// (`crc_calculate(&buf[1], header_len)` + payload + crc_extra) and
    /// pymavlink (`x25crc(self._msgbuf[1:])` + crc_extra).
    pub fn encode(&self) -> Vec<u8> {
        let extra = crc_extra_for(self.msgid).unwrap_or(0);
        let mut out = Vec::with_capacity(12 + self.payload.len());
        out.push(STX_V2);
        out.push(self.payload.len() as u8);
        out.push(0); // incompat flags: no signature
        out.push(0); // compat flags
        out.push(self.seq);
        out.push(self.sysid);
        out.push(self.compid);
        out.extend_from_slice(&self.msgid.to_le_bytes()[..3]);
        out.extend_from_slice(&self.payload);
        let mut crc_input = Vec::with_capacity(10 + self.payload.len() + 1);
        crc_input.extend_from_slice(&out[1..10]);
        crc_input.extend_from_slice(&self.payload);
        crc_input.push(extra);
        let crc = x25_crc(&crc_input);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// CRC of this frame as it would be / was carried on the wire. The
    /// parser uses the same computation to verify received frames.
    pub fn computed_crc(&self) -> u16 {
        let extra = crc_extra_for(self.msgid).unwrap_or(0);
        let mut crc_input = Vec::with_capacity(10 + self.payload.len() + 1);
        crc_input.push(self.payload.len() as u8);
        crc_input.push(0u8);
        crc_input.push(0u8);
        crc_input.push(self.seq);
        crc_input.push(self.sysid);
        crc_input.push(self.compid);
        crc_input.extend_from_slice(&self.msgid.to_le_bytes()[..3]);
        crc_input.extend_from_slice(&self.payload);
        crc_input.push(extra);
        x25_crc(&crc_input)
    }

    /// Decode the (already CRC-verified) payload into a typed message.
    pub fn decode(&self) -> Result<Message, DecodeError> {
        Message::unpack_payload(self.msgid, &self.payload)
    }
}

/// Incremental byte-stream parser for the HIL link.
///
/// - Accepts MAVLink v2 frames; verifies the CRC against the HIL subset's
///   CRC_EXTRA table; unknown-but-well-formed ids are counted and dropped.
/// - Tolerates MAVLink v1 frames (counted, skipped) and resynchronizes on
///   garbage bytes.
pub struct FrameParser {
    buf: Vec<u8>,
    ready: Vec<Frame>,
    /// Bytes dropped while resynchronizing.
    pub dropped_bytes: u64,
    /// Frames dropped because the X.25 CRC did not match.
    pub bad_crc_frames: u64,
    /// Frames dropped because the message id is outside the HIL subset.
    pub unknown_msgid_frames: u64,
    /// MAVLink v1 frames skipped (never sent by rustsitsim).
    pub v1_frames: u64,
}

impl FrameParser {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(512),
            ready: Vec::new(),
            dropped_bytes: 0,
            bad_crc_frames: 0,
            unknown_msgid_frames: 0,
            v1_frames: 0,
        }
    }

    /// Feed raw bytes; complete frames are buffered until `take()`.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        loop {
            while !self.buf.is_empty() && self.buf[0] != STX_V2 && self.buf[0] != STX_V1 {
                self.buf.remove(0);
                self.dropped_bytes += 1;
            }
            if self.buf.is_empty() {
                return;
            }
            if self.buf[0] == STX_V1 {
                // v1: STX + LEN + 5 header bytes + payload + 2 CRC.
                if self.buf.len() < 8 {
                    return;
                }
                let total = 8 + self.buf[1] as usize;
                if self.buf.len() < total {
                    return;
                }
                self.v1_frames += 1;
                self.buf.drain(..total);
                continue;
            }
            // v2: 10-byte header + payload + 2 CRC.
            if self.buf.len() < 12 {
                return;
            }
            let len = self.buf[1] as usize;
            let total = 12 + len;
            if self.buf.len() < total {
                return;
            }
            let msgid =
                (self.buf[7] as u32) | ((self.buf[8] as u32) << 8) | ((self.buf[9] as u32) << 16);
            let frame = Frame {
                seq: self.buf[4],
                sysid: self.buf[5],
                compid: self.buf[6],
                msgid,
                payload: self.buf[10..10 + len].to_vec(),
            };
            let wire_crc = u16::from_le_bytes([self.buf[10 + len], self.buf[11 + len]]);
            self.buf.drain(..total);

            if crc_extra_for(msgid).is_none() {
                self.unknown_msgid_frames += 1;
                continue;
            }
            if frame.computed_crc() != wire_crc {
                self.bad_crc_frames += 1;
                continue;
            }
            self.ready.push(frame);
        }
    }

    /// Take all complete, CRC-verified frames.
    pub fn take(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.ready)
    }
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::CommandAck;

    #[test]
    fn frame_roundtrip_command_ack() {
        let msg = Message::CommandAck(CommandAck::new(511, 0));
        let frame = Frame::from_message(&msg, 7, 2, 1);
        let bytes = frame.encode();
        // Fragmented feed: parser must buffer across feed() calls.
        let mut p = FrameParser::new();
        p.feed(&bytes[..3]);
        p.feed(&bytes[3..]);
        let frames = p.take();
        assert_eq!(frames.len(), 1);
        match frames[0].decode().unwrap() {
            Message::CommandAck(a) => {
                assert_eq!(a.command, 511);
                assert_eq!(a.result, 0);
            }
            _ => panic!("wrong message"),
        }
    }

    #[test]
    fn parser_resyncs_on_garbage() {
        let msg = Message::CommandAck(CommandAck::new(511, 0));
        let bytes = Frame::from_message(&msg, 0, 2, 1).encode();
        let mut p = FrameParser::new();
        let mut junk = vec![0x12u8, 0x34, 0x00];
        junk.extend_from_slice(&bytes);
        junk.push(0xFE); // trailing v1 STX without body: must be ignored
        p.feed(&junk);
        let frames = p.take();
        assert_eq!(frames.len(), 1);
        assert_eq!(p.dropped_bytes, 3);
    }

    #[test]
    fn bad_crc_frame_dropped() {
        let msg = Message::CommandAck(CommandAck::new(511, 0));
        let mut bytes = Frame::from_message(&msg, 0, 2, 1).encode();
        let n = bytes.len();
        bytes[n - 1] ^= 0xFF;
        let mut p = FrameParser::new();
        p.feed(&bytes);
        assert!(p.take().is_empty());
        assert_eq!(p.bad_crc_frames, 1);
    }

    #[test]
    fn unknown_msgid_counted() {
        // HEARTBEAT (0) is well-formed v2 but outside the HIL subset.
        let frame = Frame { seq: 0, sysid: 2, compid: 1, msgid: crate::msg_id::HEARTBEAT, payload: vec![0u8; 9] };
        let bytes = frame.encode();
        let mut p = FrameParser::new();
        p.feed(&bytes);
        assert!(p.take().is_empty());
        assert_eq!(p.unknown_msgid_frames, 1);
    }
}
