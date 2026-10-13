//! The [`AudioEngine`] trait (what game code talks to), the mixer-backed implementation with a headless
//! offline renderer, and a null engine for tests that do not care about audio.

use crate::mixer::{Bus, Ducking, Listener, Mixer, MixerStats, PcmSource, VoiceHandle, VoiceParams, VoiceUpdate};
use crate::pcm::{wav_f32, Pcm};
use crate::reverb::ReverbParams;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

/// Everything game code needs from an audio backend. Object safe; implementations are `Send` so the
/// engine can live in the game thread while a device callback drives the mixer.
pub trait AudioEngine: Send {
    fn sample_rate(&self) -> u32;
    /// Starts a buffer voice. `None` when the voice limit was hit and no lower-priority voice could be stolen.
    fn play(&mut self, pcm: Arc<Pcm>, params: VoiceParams) -> Option<VoiceHandle>;
    /// Starts a streaming voice (music etc.).
    fn play_stream(&mut self, src: Box<dyn PcmSource>, params: VoiceParams) -> Option<VoiceHandle>;
    fn stop(&mut self, h: VoiceHandle, fade_secs: f32);
    fn stop_all(&mut self, bus: Option<Bus>);
    fn is_playing(&self, h: VoiceHandle) -> bool;
    fn update_voice(&mut self, h: VoiceHandle, u: &VoiceUpdate);
    fn set_listener(&mut self, l: Listener);
    fn set_bus_volume(&mut self, bus: Bus, v: f32);
    fn set_bus_muted(&mut self, bus: Bus, muted: bool);
    fn set_master_volume(&mut self, v: f32);
    fn set_ducking(&mut self, d: Option<Ducking>);
    fn set_reverb(&mut self, r: ReverbParams);
    fn set_max_voices(&mut self, n: usize);
    fn stats(&self) -> MixerStats;
}

/// Mixer engine. The mixer is shared (`Arc<Mutex<_>>`) so an output device callback can pull from it.
#[derive(Clone)]
pub struct MixerEngine {
    mixer: Arc<Mutex<Mixer>>,
}

impl MixerEngine {
    /// Engine without a device: audio is produced only by [`MixerEngine::render`] /
    /// [`MixerEngine::render_offline`].
    pub fn headless(sample_rate: u32) -> MixerEngine {
        MixerEngine { mixer: Arc::new(Mutex::new(Mixer::new(sample_rate))) }
    }

    /// The shared mixer (hand it to an output backend such as [`crate::device::DeviceOutput`]).
    pub fn mixer(&self) -> Arc<Mutex<Mixer>> {
        self.mixer.clone()
    }

    fn lock(&self) -> MutexGuard<'_, Mixer> {
        self.mixer.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Renders `frames` stereo frames and returns them interleaved.
    pub fn render(&self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        self.lock().render(&mut out);
        out
    }

    /// Renders `seconds` of audio in blocks of `block_frames`; `tick(time_secs, engine)` runs before every
    /// block so a script can start sounds / move the listener at exact times. Deterministic.
    pub fn render_offline(&mut self, seconds: f32, block_frames: usize, mut tick: impl FnMut(f64, &mut MixerEngine)) -> Vec<f32> {
        let rate = self.sample_rate();
        let total = (seconds * rate as f32) as usize;
        let block = block_frames.max(1);
        let mut out = Vec::with_capacity(total * 2);
        let mut done = 0usize;
        while done < total {
            let n = block.min(total - done);
            tick(done as f64 / rate as f64, self);
            out.extend_from_slice(&self.render(n));
            done += n;
        }
        out
    }

    /// Writes interleaved stereo float samples as a 16-bit WAV file.
    pub fn write_wav(path: impl AsRef<Path>, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
        std::fs::write(path, wav_f32(samples, 2, sample_rate))
    }
}

impl AudioEngine for MixerEngine {
    fn sample_rate(&self) -> u32 {
        self.lock().sample_rate()
    }
    fn play(&mut self, pcm: Arc<Pcm>, params: VoiceParams) -> Option<VoiceHandle> {
        self.lock().play(pcm, params)
    }
    fn play_stream(&mut self, src: Box<dyn PcmSource>, params: VoiceParams) -> Option<VoiceHandle> {
        self.lock().play_stream(src, params)
    }
    fn stop(&mut self, h: VoiceHandle, fade_secs: f32) {
        self.lock().stop(h, fade_secs)
    }
    fn stop_all(&mut self, bus: Option<Bus>) {
        self.lock().stop_all(bus)
    }
    fn is_playing(&self, h: VoiceHandle) -> bool {
        self.lock().is_playing(h)
    }
    fn update_voice(&mut self, h: VoiceHandle, u: &VoiceUpdate) {
        self.lock().update_voice(h, u)
    }
    fn set_listener(&mut self, l: Listener) {
        self.lock().set_listener(l)
    }
    fn set_bus_volume(&mut self, bus: Bus, v: f32) {
        self.lock().set_bus_volume(bus, v)
    }
    fn set_bus_muted(&mut self, bus: Bus, muted: bool) {
        self.lock().set_bus_muted(bus, muted)
    }
    fn set_master_volume(&mut self, v: f32) {
        self.lock().set_master_volume(v)
    }
    fn set_ducking(&mut self, d: Option<Ducking>) {
        self.lock().set_ducking(d)
    }
    fn set_reverb(&mut self, r: ReverbParams) {
        self.lock().set_reverb(r)
    }
    fn set_max_voices(&mut self, n: usize) {
        self.lock().set_max_voices(n)
    }
    fn stats(&self) -> MixerStats {
        self.lock().stats()
    }
}

/// Engine that discards everything (unit tests of game logic, servers).
#[derive(Default)]
pub struct NullEngine {
    next: u64,
    pub played: Vec<(VoiceHandle, VoiceParams)>,
}

impl AudioEngine for NullEngine {
    fn sample_rate(&self) -> u32 {
        48000
    }
    fn play(&mut self, _pcm: Arc<Pcm>, params: VoiceParams) -> Option<VoiceHandle> {
        self.next += 1;
        let h = VoiceHandle(self.next);
        self.played.push((h, params));
        Some(h)
    }
    fn play_stream(&mut self, _src: Box<dyn PcmSource>, params: VoiceParams) -> Option<VoiceHandle> {
        self.next += 1;
        let h = VoiceHandle(self.next);
        self.played.push((h, params));
        Some(h)
    }
    fn stop(&mut self, _h: VoiceHandle, _fade_secs: f32) {}
    fn stop_all(&mut self, _bus: Option<Bus>) {}
    fn is_playing(&self, _h: VoiceHandle) -> bool {
        false
    }
    fn update_voice(&mut self, _h: VoiceHandle, _u: &VoiceUpdate) {}
    fn set_listener(&mut self, _l: Listener) {}
    fn set_bus_volume(&mut self, _bus: Bus, _v: f32) {}
    fn set_bus_muted(&mut self, _bus: Bus, _muted: bool) {}
    fn set_master_volume(&mut self, _v: f32) {}
    fn set_ducking(&mut self, _d: Option<Ducking>) {}
    fn set_reverb(&mut self, _r: ReverbParams) {}
    fn set_max_voices(&mut self, _n: usize) {}
    fn stats(&self) -> MixerStats {
        MixerStats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_render_is_deterministic_and_scripted() {
        let pcm = Arc::new(Pcm::new(1, 22050, (0..22050).map(|i| ((i as f32 * 0.1).sin() * 8000.0) as i16).collect()));
        let run = || {
            let mut e = MixerEngine::headless(44100);
            let p = pcm.clone();
            let mut started = false;
            e.render_offline(2.0, 512, move |t, eng| {
                if t >= 0.5 && !started {
                    started = true;
                    eng.play(p.clone(), VoiceParams::default());
                }
            })
        };
        let a = run();
        let b = run();
        assert_eq!(a.len(), 2 * 88200);
        assert_eq!(a, b);
        // silent before 0.5 s, active after
        let first: f32 = a[..2 * 20000].iter().map(|s| s.abs()).sum();
        let later: f32 = a[2 * 22050..2 * 44100].iter().map(|s| s.abs()).sum();
        assert_eq!(first, 0.0);
        assert!(later > 100.0);
    }

    #[test]
    fn trait_object_works() {
        let mut e: Box<dyn AudioEngine> = Box::new(NullEngine::default());
        let pcm = Arc::new(Pcm::new(1, 8000, vec![1; 100]));
        assert!(e.play(pcm, VoiceParams::default()).is_some());
    }
}
