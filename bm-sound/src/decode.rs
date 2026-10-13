//! Decoding of mod-supplied audio assets (WAV incl. IMA/MS ADPCM + float, Ogg Vorbis, FLAC) via symphonia.

use crate::pcm::Pcm;
use std::io::Cursor;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("unsupported or corrupt audio: {0}")]
    Format(String),
    #[error("no audio track found")]
    NoTrack,
    #[error("decoded stream is empty")]
    Empty,
}

/// Decodes a whole file. `ext` (e.g. `"ogg"`) is only a hint; the container is probed.
pub fn decode_asset(bytes: Vec<u8>, ext: Option<&str>) -> Result<Pcm, DecodeError> {
    let mss = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    if let Some(e) = ext {
        hint.with_extension(e);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| DecodeError::Format(e.to_string()))?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or(DecodeError::NoTrack)?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| DecodeError::Format(e.to_string()))?;
    let mut channels = track.codec_params.channels.map(|c| c.count() as u16).unwrap_or(0);
    let mut rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut out: Vec<i16> = Vec::new();
    let mut sbuf: Option<SampleBuffer<i16>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(DecodeError::Format(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let spec = *buf.spec();
                channels = spec.channels.count() as u16;
                rate = spec.rate;
                if sbuf.as_ref().map_or(true, |s| s.capacity() < buf.capacity() * spec.channels.count()) {
                    sbuf = Some(SampleBuffer::<i16>::new(buf.capacity() as u64, spec));
                }
                let sb = sbuf.as_mut().unwrap();
                sb.copy_interleaved_ref(buf);
                out.extend_from_slice(sb.samples());
            }
            Err(SymError::DecodeError(_)) => continue, // skip corrupt packet
            Err(e) => return Err(DecodeError::Format(e.to_string())),
        }
    }
    if out.is_empty() || channels == 0 || rate == 0 {
        return Err(DecodeError::Empty);
    }
    Ok(Pcm::new(channels, rate, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcm::wav_i16;

    #[test]
    fn wav_round_trip() {
        let src: Vec<i16> = (0..2000).map(|i| ((i as f32 * 0.05).sin() * 12000.0) as i16).collect();
        let wav = wav_i16(&src, 1, 22050);
        let p = decode_asset(wav, Some("wav")).unwrap();
        assert_eq!(p.channels, 1);
        assert_eq!(p.sample_rate, 22050);
        assert_eq!(&p.samples[..], &src[..]);
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(decode_asset(vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10], Some("ogg")).is_err());
    }
}
