//! Replay recording (SPEC §8.2): an append-only binary file of fixed-size
//! tick records behind a 64-byte header.
//!
//! Header (64 bytes):
//! ```text
//! [ 0.. 8] magic "RSITSIM1"
//! [ 8..10] format version (u16 = 1)
//! [10..12] header length (u16 = 64)
//! [12..16] tick rate in millihertz (u32; 200_000 = 200 Hz)
//! [16..24] RNG seed (u64)
//! [24..32] start virtual time, us (u64; always 0 in v0.1)
//! [32..64] scenario SHA-256 (32 bytes)
//! ```
//!
//! Tick record (96 bytes):
//! ```text
//! [ 0.. 8] virtual time, us (u64)
//! [ 8..24] 16 motor inputs, u8 = round(u_i * 255) of the RAW commanded
//!          controls (pre-fault; faults are visible via the flags + state)
//! [24..92] 17 state floats (f32 LE): pos(3), vel(3), q(4), omega(3), rotors(4)
//! [92..94] fault-active bitmap (u16; bit i = spec i of the first 16)
//! [94..96] CRC-16/X.25 over bytes [0..94]
//! ```
//!
//! The record is exactly 96 bytes per SPEC §8.2; the spec's "4 fault-active
//! bits" is realized as a 16-slot u16 bitmap so the arithmetic closes
//! (ADR-012). No compression: append-only + trivial reader.
//!
//! Header and records carry no wall-clock data — two runs of the same
//! scenario produce byte-identical files (I-5).

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use sitsim_mavlink::x25_crc;

pub const MAGIC: &[u8; 8] = b"RSITSIM1";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 64;
pub const RECORD_LEN: usize = 96;
/// Max fault slots per record bitmap.
pub const FAULT_SLOTS: usize = 16;

/// Parsed replay header.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayHeader {
    pub version: u16,
    /// Tick rate, Hz.
    pub rate_hz: f64,
    pub seed: u64,
    pub start_t_us: u64,
    pub scenario_hash: [u8; 32],
}

/// One decoded tick record.
#[derive(Debug, Clone, PartialEq)]
pub struct TickRecord {
    pub t_us: u64,
    /// Raw commanded motor inputs, u in [0, 1] (dequantized).
    pub motors: [f64; 16],
    /// pos(3), vel(3), q(4), omega(3), rotors(4).
    pub state: [f32; 17],
    /// Fault-active bitmap.
    pub fault_flags: u16,
}

impl TickRecord {
    /// Encode into the fixed 96-byte wire form.
    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut buf = [0u8; RECORD_LEN];
        buf[0..8].copy_from_slice(&self.t_us.to_le_bytes());
        for (i, &m) in self.motors.iter().enumerate() {
            buf[8 + i] = (m.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        for (i, &v) in self.state.iter().enumerate() {
            buf[24 + 4 * i..28 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        buf[92..94].copy_from_slice(&self.fault_flags.to_le_bytes());
        let crc = x25_crc(&buf[..94]);
        buf[94..96].copy_from_slice(&crc.to_le_bytes());
        buf
    }

    /// Decode and verify the CRC.
    pub fn decode(buf: &[u8]) -> Result<TickRecord, String> {
        if buf.len() != RECORD_LEN {
            return Err(format!("record length {} != {RECORD_LEN}", buf.len()));
        }
        let crc = u16::from_le_bytes([buf[94], buf[95]]);
        let expect = x25_crc(&buf[..94]);
        if crc != expect {
            return Err(format!("CRC mismatch: stored {crc:#06x}, computed {expect:#06x}"));
        }
        let t_us = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let mut motors = [0.0f64; 16];
        for (i, m) in motors.iter_mut().enumerate() {
            *m = buf[8 + i] as f64 / 255.0;
        }
        let mut state = [0f32; 17];
        for (i, v) in state.iter_mut().enumerate() {
            *v = f32::from_le_bytes(buf[24 + 4 * i..28 + 4 * i].try_into().unwrap());
        }
        let fault_flags = u16::from_le_bytes([buf[92], buf[93]]);
        Ok(TickRecord { t_us, motors, state, fault_flags })
    }
}

/// Encode a 64-byte header.
pub fn encode_header(rate_hz: f64, seed: u64, start_t_us: u64, scenario_hash: &[u8; 32]) -> [u8; HEADER_LEN] {
    let mut buf = [0u8; HEADER_LEN];
    buf[0..8].copy_from_slice(MAGIC);
    buf[8..10].copy_from_slice(&VERSION.to_le_bytes());
    buf[10..12].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
    buf[12..16].copy_from_slice(&((rate_hz * 1000.0).round() as u32).to_le_bytes());
    buf[16..24].copy_from_slice(&seed.to_le_bytes());
    buf[24..32].copy_from_slice(&start_t_us.to_le_bytes());
    buf[32..64].copy_from_slice(scenario_hash);
    buf
}

/// Decode + validate a 64-byte header.
pub fn decode_header(buf: &[u8]) -> Result<ReplayHeader, String> {
    if buf.len() != HEADER_LEN {
        return Err(format!("header length {} != {HEADER_LEN}", buf.len()));
    }
    if &buf[0..8] != MAGIC {
        return Err("bad magic (not a rustsitsim replay)".into());
    }
    let version = u16::from_le_bytes(buf[8..10].try_into().unwrap());
    if version != VERSION {
        return Err(format!("unsupported replay version {version}"));
    }
    let millihz = u32::from_le_bytes(buf[12..16].try_into().unwrap());
    let seed = u64::from_le_bytes(buf[16..24].try_into().unwrap());
    let start_t_us = u64::from_le_bytes(buf[24..32].try_into().unwrap());
    let mut scenario_hash = [0u8; 32];
    scenario_hash.copy_from_slice(&buf[32..64]);
    Ok(ReplayHeader {
        version,
        rate_hz: millihz as f64 / 1000.0,
        seed,
        start_t_us,
        scenario_hash,
    })
}

/// Append-only replay writer. Flushing every `flush_every` records keeps
/// `GET /api/replay` meaningful mid-run without per-tick syscalls.
pub struct ReplayWriter {
    file: BufWriter<File>,
    pub records: u64,
    flush_every: u64,
}

impl ReplayWriter {
    pub fn create(path: &Path, rate_hz: f64, seed: u64, scenario_hash: &[u8; 32]) -> io::Result<ReplayWriter> {
        let mut file = BufWriter::new(File::create(path)?);
        file.write_all(&encode_header(rate_hz, seed, 0, scenario_hash))?;
        Ok(ReplayWriter { file, records: 0, flush_every: 200 })
    }

    /// Append one record.
    pub fn push(&mut self, rec: &TickRecord) -> io::Result<()> {
        self.file.write_all(&rec.encode())?;
        self.records += 1;
        if self.records % self.flush_every == 0 {
            self.file.flush()?;
        }
        Ok(())
    }

    /// Final flush + fsync-ish close.
    pub fn finish(mut self) -> io::Result<u64> {
        self.file.flush()?;
        Ok(self.records)
    }
}

/// Replay reader: header + iterator over records (CRC-validated).
pub struct ReplayReader {
    header: ReplayHeader,
    reader: BufReader<File>,
    pub records_read: u64,
}

impl ReplayReader {
    pub fn open(path: &Path) -> Result<ReplayReader, String> {
        let mut reader = BufReader::new(
            File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?,
        );
        let mut hbuf = [0u8; HEADER_LEN];
        reader.read_exact(&mut hbuf).map_err(|e| format!("short header: {e}"))?;
        let header = decode_header(&hbuf)?;
        Ok(ReplayReader { header, reader, records_read: 0 })
    }

    pub fn header(&self) -> &ReplayHeader {
        &self.header
    }

    /// Next record, or None at EOF. Malformed records are errors.
    pub fn next_record(&mut self) -> Result<Option<TickRecord>, String> {
        let mut buf = [0u8; RECORD_LEN];
        match self.reader.read_exact(&mut buf) {
            Ok(()) => {
                self.records_read += 1;
                Ok(Some(TickRecord::decode(&buf)?))
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(format!("read error: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round trip: write two records, read them back byte-exact, and the
    /// file size is 64 + 2*96.
    #[test]
    fn replay_round_trip() {
        let dir = std::env::temp_dir().join("sitsim-replay-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rt.rsreplay");
        let hash = [7u8; 32];

        let mut w = ReplayWriter::create(&path, 200.0, 42, &hash).unwrap();
        let rec1 = TickRecord {
            t_us: 5_000,
            motors: [0.0, 0.25, 0.5, 0.75, 1.0, 0.1, 0.2, 0.3, 0.4, 0.6, 0.7, 0.8, 0.9, 0.33, 0.44, 0.55],
            state: {
                let mut s = [0f32; 17];
                s[0] = 1.5;
                s[6] = 1.0;
                s[16] = 0.99;
                s
            },
            fault_flags: 0b101,
        };
        let rec2 = TickRecord { t_us: 10_000, motors: [0.5; 16], state: [0f32; 17], fault_flags: 0 };
        w.push(&rec1).unwrap();
        w.push(&rec2).unwrap();
        let n = w.finish().unwrap();
        assert_eq!(n, 2);

        assert_eq!(std::fs::metadata(&path).unwrap().len() as usize, HEADER_LEN + 2 * RECORD_LEN);

        let mut r = ReplayReader::open(&path).unwrap();
        assert_eq!(r.header().rate_hz, 200.0);
        assert_eq!(r.header().seed, 42);
        assert_eq!(r.header().scenario_hash, hash);
        let got1 = r.next_record().unwrap().unwrap();
        assert_eq!(got1.t_us, 5_000);
        assert!((got1.motors[1] - 0.25).abs() < 0.01, "quantized motor {}", got1.motors[1]);
        assert_eq!(got1.state[0], 1.5);
        assert_eq!(got1.state[6], 1.0);
        assert_eq!(got1.fault_flags, 0b101);
        let got2 = r.next_record().unwrap().unwrap();
        assert_eq!(got2.t_us, 10_000);
        assert!(r.next_record().unwrap().is_none(), "EOF after 2 records");
    }

    /// Bit-identical files for identical runs (I-5 property, header + body
    /// carry no wall-clock).
    #[test]
    fn identical_runs_identical_files() {
        let dir = std::env::temp_dir().join("sitsim-replay-test");
        std::fs::create_dir_all(&dir).unwrap();
        let hash = [9u8; 32];
        let write = |name: &str| {
            let path = dir.join(name);
            let mut w = ReplayWriter::create(&path, 200.0, 42, &hash).unwrap();
            for t in 1..=10 {
                w.push(&TickRecord {
                    t_us: t * 5_000,
                    motors: [0.5; 16],
                    state: [t as f32; 17],
                    fault_flags: (t % 3) as u16,
                })
                .unwrap();
            }
            w.finish().unwrap();
            std::fs::read(&path).unwrap()
        };
        assert_eq!(write("a.rsreplay"), write("b.rsreplay"));
    }

    /// CRC corruption is detected.
    #[test]
    fn corrupted_record_detected() {
        let rec = TickRecord { t_us: 1, motors: [0.5; 16], state: [0f32; 17], fault_flags: 0 };
        let mut bytes = rec.encode();
        bytes[3] ^= 0xFF; // corrupt t_us
        assert!(TickRecord::decode(&bytes).is_err());

        // Truncation errors.
        assert!(TickRecord::decode(&bytes[..95]).is_err());
        let mut h = encode_header(200.0, 1, 0, &[0u8; 32]);
        h[0] = b'X';
        assert!(decode_header(&h).is_err());
    }
}
