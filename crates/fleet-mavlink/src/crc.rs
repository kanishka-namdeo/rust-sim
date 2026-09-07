//! CRC-16/X.25 ("MCRF4XX") as used by MAVLink — port of pymavlink's
//! `x25crc` accumulate loop, bit-exact.

#![forbid(unsafe_code)]

#[derive(Clone, Copy)]
pub struct X25Crc {
    pub crc: u16,
}

impl X25Crc {
    pub fn new() -> Self {
        X25Crc { crc: 0xffff }
    }

    pub fn accumulate(&mut self, data: u8) {
        let mut tmp = (data as u16) ^ (self.crc & 0xff);
        tmp = (tmp & 0xff) ^ ((tmp << 4) & 0xff);
        self.crc = (self.crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4);
    }

    pub fn accumulate_slice(&mut self, data: &[u8]) {
        for &b in data {
            self.accumulate(b);
        }
    }

    pub fn finalize_with_extra(self, crc_extra: u8) -> u16 {
        let mut c = self;
        c.accumulate(crc_extra);
        c.crc
    }
}

impl Default for X25Crc {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_byte_changes_crc() {
        // Bit-exact CRC correctness against pymavlink is enforced by the
        // golden-vector integration test (tests/golden.rs); here we only
        // pin structural properties.
        let mut c = X25Crc::new();
        c.accumulate_slice(b"123456789");
        let plain = c.crc;
        let with_extra = c.finalize_with_extra(50);
        assert_ne!(plain, with_extra);
    }

    #[test]
    fn crc_is_deterministic() {
        let mut a = X25Crc::new();
        a.accumulate_slice(&[0xfd, 9, 0, 0]);
        let mut b = X25Crc::new();
        b.accumulate_slice(&[0xfd, 9, 0, 0]);
        assert_eq!(a.crc, b.crc);
    }

    #[test]
    fn crc_is_order_sensitive() {
        let mut a = X25Crc::new();
        a.accumulate_slice(&[1, 2, 3]);
        let mut b = X25Crc::new();
        b.accumulate_slice(&[3, 2, 1]);
        assert_ne!(a.crc, b.crc);
    }
}
