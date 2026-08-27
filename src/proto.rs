//! TL7218 `WAV_RECORD_EN` PCM trace packet format.
//!
//! The firmware (`bluetooth/utilities/wav/wav.c`) pushes fixed-size packets over the
//! USB CDC virtual com port:
//!
//! ```text
//! offset  size  field
//!   0      2    preamble = 0xAAAA (little endian)
//!   2      1    idx          packet counter & 0x03, used for drop detection
//!   3      1    bits         sample width reported by the firmware (16)
//!   4      4    sample_rate  little endian, 16000
//!   8    4*N    buffer[N]    N = samples per packet (160 => 10 ms at 16 kHz)
//! ```
//!
//! Every `buffer[i]` word packs two 16-bit samples: the high half and the low half.
//! At the `intercom.c` capture point the high half is the signal *before* noise
//! suppression and the low half is *after*; at the `codec_task.c` capture point they
//! are the raw codec left/right channels.

/// Packet preamble emitted by `wav.c` (`WAV_PACKET_PREMBLE`).
pub const PREAMBLE: [u8; 2] = [0xAA, 0xAA];

/// Bytes ahead of the sample words.
pub const HEADER_LEN: usize = 8;

/// Firmware default: `WAV_SAMPLES_PER_PACKET`.
pub const DEFAULT_SAMPLES_PER_PACKET: usize = 160;

/// Plausibility window for the `sample_rate` header field, used to reject false
/// preamble matches while resynchronising.
const SAMPLE_RATE_RANGE: std::ops::RangeInclusive<u32> = 4_000..=192_000;

/// How the two 16-bit halves of each 32-bit word are turned into output channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ChannelMode {
    /// Two-channel output: channel 0 = high half, channel 1 = low half.
    Both,
    /// Mono output from the high half only (NS input / left).
    HighOnly,
    /// Mono output from the low half only (NS output / right).
    LowOnly,
}

impl ChannelMode {
    pub fn channels(self) -> u16 {
        match self {
            ChannelMode::Both => 2,
            ChannelMode::HighOnly | ChannelMode::LowOnly => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChannelMode::Both => "双通道 (高16位 + 低16位)",
            ChannelMode::HighOnly => "仅高16位 (降噪前 / 左)",
            ChannelMode::LowOnly => "仅低16位 (降噪后 / 右)",
        }
    }

    pub fn next(self) -> Self {
        match self {
            ChannelMode::Both => ChannelMode::HighOnly,
            ChannelMode::HighOnly => ChannelMode::LowOnly,
            ChannelMode::LowOnly => ChannelMode::Both,
        }
    }
}

/// One decoded trace packet.
#[derive(Debug, Clone)]
pub struct Packet {
    pub idx: u8,
    pub bits: u8,
    pub sample_rate: u32,
    /// Interleaved output samples, already reduced according to the [`ChannelMode`].
    pub samples: Vec<i16>,
}

/// Incremental packet extractor that tolerates arbitrary chunk boundaries and
/// resynchronises after corruption or a mid-stream connect.
pub struct Deframer {
    buf: Vec<u8>,
    samples_per_packet: usize,
    mode: ChannelMode,
    /// Bytes discarded while hunting for a valid preamble.
    pub resync_bytes: u64,
}

impl Deframer {
    pub fn new(samples_per_packet: usize, mode: ChannelMode) -> Self {
        Self {
            buf: Vec::with_capacity(64 * 1024),
            samples_per_packet,
            mode,
            resync_bytes: 0,
        }
    }

    pub fn packet_len(&self) -> usize {
        HEADER_LEN + self.samples_per_packet * 4
    }

    pub fn extend(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Pops the next valid packet, discarding any leading garbage.
    pub fn next_packet(&mut self) -> Option<Packet> {
        let packet_len = self.packet_len();

        loop {
            if self.buf.len() < packet_len {
                return None;
            }

            if self.header_is_plausible() {
                let packet = self.decode(packet_len);
                self.buf.drain(..packet_len);
                return Some(packet);
            }

            // Skip one byte and try again; a real preamble may start anywhere.
            self.buf.remove(0);
            self.resync_bytes += 1;
        }
    }

    fn header_is_plausible(&self) -> bool {
        if self.buf[0..2] != PREAMBLE {
            return false;
        }
        if self.buf[2] > 3 {
            return false;
        }
        if !matches!(self.buf[3], 16 | 24 | 32) {
            return false;
        }
        let rate = u32::from_le_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]);
        SAMPLE_RATE_RANGE.contains(&rate)
    }

    fn decode(&self, packet_len: usize) -> Packet {
        let idx = self.buf[2];
        let bits = self.buf[3];
        let sample_rate = u32::from_le_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]);

        let mut samples = Vec::with_capacity(self.samples_per_packet * self.mode.channels() as usize);
        for word in self.buf[HEADER_LEN..packet_len].chunks_exact(4) {
            let raw = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
            let high = ((raw >> 16) & 0xffff) as u16 as i16;
            let low = (raw & 0xffff) as u16 as i16;
            match self.mode {
                ChannelMode::Both => {
                    samples.push(high);
                    samples.push(low);
                }
                ChannelMode::HighOnly => samples.push(high),
                ChannelMode::LowOnly => samples.push(low),
            }
        }

        Packet {
            idx,
            bits,
            sample_rate,
            samples,
        }
    }
}

/// `idx` counts modulo 4, so a gap of `n` packets shows up as a jump of `n+1`.
/// Returns the number of packets presumed lost between `previous` and `current`.
pub fn lost_between(previous: u8, current: u8) -> u8 {
    (current.wrapping_sub(previous) & 0x03).wrapping_sub(1) & 0x03
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(idx: u8, samples: &[(i16, i16)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&PREAMBLE);
        out.push(idx);
        out.push(16);
        out.extend_from_slice(&16_000u32.to_le_bytes());
        for (hi, lo) in samples {
            let raw = ((*hi as u16 as u32) << 16) | (*lo as u16 as u32);
            out.extend_from_slice(&raw.to_le_bytes());
        }
        out
    }

    #[test]
    fn decodes_both_halves() {
        let mut d = Deframer::new(2, ChannelMode::Both);
        d.extend(&packet(1, &[(100, -100), (i16::MAX, i16::MIN)]));
        let p = d.next_packet().unwrap();
        assert_eq!(p.idx, 1);
        assert_eq!(p.sample_rate, 16_000);
        assert_eq!(p.samples, vec![100, -100, i16::MAX, i16::MIN]);
        assert!(d.next_packet().is_none());
    }

    #[test]
    fn selects_single_half() {
        let mut d = Deframer::new(2, ChannelMode::LowOnly);
        d.extend(&packet(0, &[(1, 2), (3, 4)]));
        assert_eq!(d.next_packet().unwrap().samples, vec![2, 4]);

        let mut d = Deframer::new(2, ChannelMode::HighOnly);
        d.extend(&packet(0, &[(1, 2), (3, 4)]));
        assert_eq!(d.next_packet().unwrap().samples, vec![1, 3]);
    }

    #[test]
    fn resynchronises_after_leading_garbage() {
        let mut d = Deframer::new(2, ChannelMode::Both);
        d.extend(&[0x01, 0xAA, 0xFF]);
        d.extend(&packet(2, &[(7, 8), (9, 10)]));
        let p = d.next_packet().unwrap();
        assert_eq!(p.idx, 2);
        assert_eq!(d.resync_bytes, 3);
    }

    #[test]
    fn reassembles_across_chunk_boundaries() {
        let bytes = packet(3, &[(11, 12), (13, 14)]);
        let mut d = Deframer::new(2, ChannelMode::Both);
        let (head, tail) = bytes.split_at(bytes.len() - 1);
        for chunk in head.chunks(3) {
            d.extend(chunk);
            assert!(d.next_packet().is_none(), "incomplete packet must not emit");
        }
        d.extend(tail);
        assert_eq!(d.next_packet().unwrap().idx, 3);
    }

    #[test]
    fn counts_losses_from_idx_gaps() {
        assert_eq!(lost_between(0, 1), 0);
        assert_eq!(lost_between(3, 0), 0);
        assert_eq!(lost_between(0, 2), 1);
        assert_eq!(lost_between(0, 3), 2);
        assert_eq!(lost_between(0, 0), 3);
    }
}
