//! Lip-sync (`"LIP "`) chunks that precede streamed dialogue in `pc_eng.str`.
//!
//! Layout (decoded from the file, confidence **M**: structure verified on every chunk of the shipped
//! level banks, the meaning of the channel ids and the time unit are inferred):
//!
//! ```text
//! 0x00 "LIP "
//! 0x04 u32 N               payload size related (= payload bytes - channel_count)
//! 0x08 u16 channel_count   8..15 (12 typical)
//! 0x0a ?                   6 bytes (an f32 / u16 pair that is 0 in most chunks)
//! 0x10 channels, each:  u8 id, u8 id (repeated), u16 key_count, key_count x { u16 time, u8 weight }
//! ```
//!
//! `weight` is a 0..=255 mouth-shape weight, `time` is in [`LipSync::TIME_UNIT_SECONDS`] units.
//! Channel ids (0, 14..24, 40..43 ...) are the viseme/phoneme slots of the facial animation
//! (`.ANM` facial poses such as `A_Mouth_Widen`, `D_OO:Q:W`); the mapping is not resolved yet.
//! The chunk occupies `0x1000` bytes; ADPCM audio follows.

/// Size of a LIP block in `pc_eng.str`.
pub const LIP_BLOCK_SIZE: usize = 0x1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LipKey {
    pub time: u16,
    pub weight: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LipChannel {
    pub id: u8,
    pub keys: Vec<LipKey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LipSync {
    /// `N` header word (`?`).
    pub header_n: u32,
    pub declared_channels: u16,
    pub channels: Vec<LipChannel>,
}

impl LipSync {
    /// Seconds per `time` tick: 1/24 s. Calibrated on 939 lines: last key time / clip duration = 24.0..24.8.
    pub const TIME_UNIT_SECONDS: f32 = 1.0 / 24.0;

    pub fn parse(d: &[u8]) -> Option<LipSync> {
        if d.len() < 16 || &d[0..4] != b"LIP " {
            return None;
        }
        let header_n = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
        let declared_channels = u16::from_le_bytes([d[8], d[9]]);
        let end = d.len().min(LIP_BLOCK_SIZE);
        let mut p = 16;
        let mut channels = Vec::new();
        for _ in 0..declared_channels {
            if p + 4 > end {
                return None;
            }
            let id = d[p];
            if d[p + 1] != id {
                return None;
            }
            let n = u16::from_le_bytes([d[p + 2], d[p + 3]]) as usize;
            p += 4;
            if p + n * 3 > end {
                return None;
            }
            let keys = (0..n)
                .map(|i| {
                    let q = p + i * 3;
                    LipKey { time: u16::from_le_bytes([d[q], d[q + 1]]), weight: d[q + 2] }
                })
                .collect();
            p += n * 3;
            channels.push(LipChannel { id, keys });
        }
        Some(LipSync { header_n, declared_channels, channels })
    }

    /// Largest key time in ticks.
    pub fn last_time(&self) -> u16 {
        self.channels.iter().flat_map(|c| c.keys.last()).map(|k| k.time).max().unwrap_or(0)
    }

    pub fn duration_secs(&self) -> f32 {
        self.last_time() as f32 * Self::TIME_UNIT_SECONDS
    }

    /// Linearly interpolated weight (0..=1) of channel `id` at `t` seconds.
    pub fn weight_at(&self, id: u8, t: f32) -> f32 {
        let Some(c) = self.channels.iter().find(|c| c.id == id) else { return 0.0 };
        let ticks = t / Self::TIME_UNIT_SECONDS;
        let idx = c.keys.partition_point(|k| (k.time as f32) <= ticks);
        let w = |k: &LipKey| k.weight as f32 / 255.0;
        match (idx.checked_sub(1).map(|i| &c.keys[i]), c.keys.get(idx)) {
            (None, None) => 0.0,
            (None, Some(n)) => w(n) * (ticks / (n.time.max(1) as f32)).clamp(0.0, 1.0),
            (Some(p), None) => w(p),
            (Some(p), Some(n)) => {
                let span = (n.time - p.time).max(1) as f32;
                let f = ((ticks - p.time as f32) / span).clamp(0.0, 1.0);
                w(p) + (w(n) - w(p)) * f
            }
        }
    }

    /// Sampled weights of all channels at `t`: `(channel id, weight)`.
    pub fn frame_at(&self, t: f32) -> Vec<(u8, f32)> {
        self.channels.iter().map(|c| (c.id, self.weight_at(c.id, t))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synthetic_chunk() {
        let mut d = vec![0u8; LIP_BLOCK_SIZE];
        d[0..4].copy_from_slice(b"LIP ");
        d[4..8].copy_from_slice(&3u32.to_le_bytes());
        d[8..10].copy_from_slice(&2u16.to_le_bytes());
        let body: &[u8] = &[7, 7, 2, 0, 0, 0, 0, 10, 0, 255, 9, 9, 1, 0, 5, 0, 128];
        d[16..16 + body.len()].copy_from_slice(body);
        let l = LipSync::parse(&d).unwrap();
        assert_eq!(l.channels.len(), 2);
        assert_eq!(l.channels[0].keys[1], LipKey { time: 10, weight: 255 });
        assert!((l.weight_at(7, 5.0 * LipSync::TIME_UNIT_SECONDS) - 0.5).abs() < 0.01);
        assert_eq!(l.last_time(), 10);
    }

    #[test]
    fn rejects_bad_tag() {
        assert!(LipSync::parse(b"NOPE0000000000000000").is_none());
    }
}
