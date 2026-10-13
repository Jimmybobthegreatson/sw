//! Real-time output through cpal (feature `device`). The returned [`DeviceOutput`] must be kept alive (and
//! stays on the thread that created it); it pulls audio from a shared [`Mixer`].

use crate::mixer::Mixer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("no default output device")]
    NoDevice,
    #[error("audio backend: {0}")]
    Backend(String),
}

pub struct DeviceOutput {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub channels: u16,
}

impl DeviceOutput {
    /// Opens the default output device. Because the device chooses the sample rate, create the mixer with
    /// [`DeviceOutput::default_rate`] (or use [`DeviceOutput::start_default`]).
    pub fn default_rate() -> Result<u32, DeviceError> {
        let host = cpal::default_host();
        let dev = host.default_output_device().ok_or(DeviceError::NoDevice)?;
        let cfg = dev.default_output_config().map_err(|e| DeviceError::Backend(e.to_string()))?;
        Ok(cfg.sample_rate())
    }

    /// Starts streaming `mixer` to the default device. The mixer's sample rate should equal
    /// [`DeviceOutput::default_rate`]; otherwise audio plays at the wrong pitch.
    pub fn start(mixer: Arc<Mutex<Mixer>>) -> Result<DeviceOutput, DeviceError> {
        let host = cpal::default_host();
        let dev = host.default_output_device().ok_or(DeviceError::NoDevice)?;
        let cfg = dev.default_output_config().map_err(|e| DeviceError::Backend(e.to_string()))?;
        let fmt = cfg.sample_format();
        let sample_rate = cfg.sample_rate();
        let config: cpal::StreamConfig = cfg.into();
        let channels = config.channels;
        let stream = match fmt {
            SampleFormat::F32 => build::<f32>(&dev, config, mixer)?,
            SampleFormat::I16 => build::<i16>(&dev, config, mixer)?,
            SampleFormat::U16 => build::<u16>(&dev, config, mixer)?,
            SampleFormat::I32 => build::<i32>(&dev, config, mixer)?,
            other => return Err(DeviceError::Backend(format!("unsupported sample format {other}"))),
        };
        stream.play().map_err(|e| DeviceError::Backend(e.to_string()))?;
        Ok(DeviceOutput { _stream: stream, sample_rate, channels })
    }

    /// Convenience: mixer with the device rate, already streaming.
    pub fn start_default() -> Result<(crate::engine::MixerEngine, DeviceOutput), DeviceError> {
        let rate = DeviceOutput::default_rate()?;
        let engine = crate::engine::MixerEngine::headless(rate);
        let out = DeviceOutput::start(engine.mixer())?;
        Ok((engine, out))
    }
}

fn build<T>(dev: &cpal::Device, config: cpal::StreamConfig, mixer: Arc<Mutex<Mixer>>) -> Result<cpal::Stream, DeviceError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut scratch: Vec<f32> = Vec::new();
    dev.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let frames = data.len() / channels.max(1);
            scratch.resize(frames * 2, 0.0);
            match mixer.lock() {
                Ok(mut m) => m.render(&mut scratch),
                Err(p) => p.into_inner().render(&mut scratch),
            }
            for (f, frame) in data.chunks_mut(channels.max(1)).enumerate() {
                let (l, r) = (scratch[2 * f], scratch[2 * f + 1]);
                for (c, s) in frame.iter_mut().enumerate() {
                    let v = match c {
                        0 => l,
                        1 => r,
                        _ => 0.0,
                    };
                    *s = T::from_sample(v);
                }
                if channels == 1 {
                    frame[0] = T::from_sample((l + r) * 0.5);
                }
            }
        },
        |e| log::error!("audio stream error: {e}"),
        None,
    )
    .map_err(|e| DeviceError::Backend(e.to_string()))
}
