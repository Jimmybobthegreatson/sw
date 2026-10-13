//! Decoded audio buffers and WAV export helpers.

use std::sync::Arc;

/// Interleaved signed 16-bit PCM. Cheap to clone (the samples are shared).
#[derive(Clone, Debug)]
pub struct Pcm {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Arc<[i16]>,
}

impl Pcm {
    pub fn new(channels: u16, sample_rate: u32, samples: Vec<i16>) -> Pcm {
        Pcm { channels: channels.max(1), sample_rate: sample_rate.max(1), samples: samples.into() }
    }

    /// Converts float samples in `[-1, 1]` (clamped) to 16 bit.
    pub fn from_f32(channels: u16, sample_rate: u32, samples: &[f32]) -> Pcm {
        Pcm::new(channels, sample_rate, samples.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect())
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn duration_secs(&self) -> f32 {
        self.frames() as f32 / self.sample_rate as f32
    }

    pub fn bytes(&self) -> usize {
        self.samples.len() * 2
    }

    /// Frame `i` of channel `c` as float.
    #[inline]
    pub fn sample_f32(&self, frame: usize, ch: usize) -> f32 {
        self.samples[frame * self.channels as usize + ch.min(self.channels as usize - 1)] as f32 * (1.0 / 32768.0)
    }

    /// Largest absolute sample as a fraction of full scale.
    pub fn peak(&self) -> f32 {
        self.samples.iter().map(|&s| (s as i32).unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0
    }

    /// Root mean square over all samples (0..1).
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.samples.iter().map(|&s| (s as f64 / 32768.0).powi(2)).sum();
        (sum / self.samples.len() as f64).sqrt() as f32
    }

    /// Zero crossings per sample of the first channel (white noise ~0.5, speech 0.02..0.2). Useful to
    /// detect a wrong decoder: garbage ADPCM sounds like noise.
    pub fn zero_crossing_rate(&self) -> f32 {
        let ch = self.channels as usize;
        let n = self.frames();
        if n < 2 {
            return 0.0;
        }
        let mut z = 0usize;
        let mut prev = self.samples[0];
        for f in 1..n {
            let s = self.samples[f * ch];
            if (s >= 0) != (prev >= 0) {
                z += 1;
            }
            prev = s;
        }
        z as f32 / n as f32
    }

    /// Canonical 44-byte-header PCM WAV file.
    pub fn to_wav(&self) -> Vec<u8> {
        wav_i16(&self.samples, self.channels, self.sample_rate)
    }
}

/// Serialises 16-bit PCM as a RIFF/WAVE file.
pub fn wav_i16(samples: &[i16], channels: u16, rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut v = Vec::with_capacity(44 + data_len);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    v.extend_from_slice(&(channels * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

/// Serialises float samples (clamped to `[-1, 1]`) as 16-bit WAV.
pub fn wav_f32(samples: &[f32], channels: u16, rate: u32) -> Vec<u8> {
    let i: Vec<i16> = samples.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16).collect();
    wav_i16(&i, channels, rate)
}

/// Statistics used by tests / examples to sanity check audio without listening to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioStats {
    pub frames: usize,
    pub peak: f32,
    pub rms: f32,
    /// Fraction of samples at (or within 1 LSB of) full scale.
    pub clipped_fraction: f32,
    /// Fraction of 10 ms windows whose RMS is above -80 dBFS.
    pub active_fraction: f32,
    pub dc_offset: f32,
}

impl AudioStats {
    pub fn of_f32(samples: &[f32], channels: usize, rate: u32) -> AudioStats {
        let n = samples.len().max(1);
        let peak = samples.iter().fold(0f32, |a, &s| a.max(s.abs()));
        let rms = (samples.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / n as f64).sqrt() as f32;
        let clipped = samples.iter().filter(|&&s| s.abs() >= 0.9999).count() as f32 / n as f32;
        let dc = (samples.iter().map(|&s| s as f64).sum::<f64>() / n as f64) as f32;
        let win = (rate as usize / 100).max(1) * channels.max(1);
        let (mut active, mut total) = (0usize, 0usize);
        for w in samples.chunks(win) {
            total += 1;
            let r = (w.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / w.len() as f64).sqrt();
            if r > 1e-4 {
                active += 1;
            }
        }
        AudioStats {
            frames: samples.len() / channels.max(1),
            peak,
            rms,
            clipped_fraction: clipped,
            active_fraction: active as f32 / total.max(1) as f32,
            dc_offset: dc,
        }
    }

    pub fn of_pcm(p: &Pcm) -> AudioStats {
        let f: Vec<f32> = p.samples.iter().map(|&s| s as f32 / 32768.0).collect();
        AudioStats::of_f32(&f, p.channels as usize, p.sample_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_is_canonical() {
        let w = wav_i16(&[0, 1, -1, 2], 2, 22050);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 36 + 8);
        assert_eq!(w.len(), 44 + 8);
    }

    #[test]
    fn stats_detect_silence_and_clipping() {
        let s = AudioStats::of_f32(&vec![0.0; 4410], 1, 44100);
        assert_eq!(s.active_fraction, 0.0);
        let s = AudioStats::of_f32(&vec![1.0; 4410], 1, 44100);
        assert!(s.clipped_fraction > 0.99);
    }
}
