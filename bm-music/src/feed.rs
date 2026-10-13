//! Read-ahead: every stem owns a ring of decoded stereo frames at the engine rate that a background
//! thread (or the caller, in manual mode) keeps filled, so the audio callback never touches the disk.

use crate::error::Result;
use crate::segment::{Frame, StemGen, StemInfo};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Default)]
struct RingInner {
    data: VecDeque<Frame>,
    eof: bool,
    failed: Option<String>,
    info: Option<StemInfo>,
    produced: u64,
}

/// Single producer / single consumer frame queue.
#[derive(Default)]
pub struct Ring {
    inner: Mutex<RingInner>,
}

impl Ring {
    fn lock(&self) -> std::sync::MutexGuard<'_, RingInner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
    pub fn available(&self) -> usize {
        self.lock().data.len()
    }
    /// All data has been produced and consumed (or the stem failed to open).
    pub fn drained(&self) -> bool {
        let g = self.lock();
        g.data.is_empty() && (g.eof || g.failed.is_some())
    }
    pub fn failed(&self) -> Option<String> {
        self.lock().failed.clone()
    }
    pub fn info(&self) -> Option<StemInfo> {
        self.lock().info
    }
    /// At least `frames` buffered, or the stream is complete.
    pub fn eof(&self) -> bool {
        self.lock().eof
    }
    pub fn ready(&self, frames: usize) -> bool {
        let g = self.lock();
        g.data.len() >= frames || g.eof || g.failed.is_some()
    }
    /// Pops up to `out.len()` frames; returns how many were written.
    pub fn pop(&self, out: &mut [Frame]) -> usize {
        let mut g = self.lock();
        let n = out.len().min(g.data.len());
        for (o, f) in out.iter_mut().zip(g.data.drain(..n)) {
            *o = f;
        }
        n
    }
    pub fn produced(&self) -> u64 {
        self.lock().produced
    }
}

type Opener = Box<dyn FnOnce() -> Result<StemGen> + Send>;

/// Producer side of one stem.
pub struct StemFeed {
    ring: Arc<Ring>,
    opener: Option<Opener>,
    gen_: Option<StemGen>,
    target: usize,
    done: bool,
}

const PRODUCE_CHUNK: usize = 2048;

impl StemFeed {
    pub fn new(ring: Arc<Ring>, opener: Opener, target_frames: usize) -> StemFeed {
        StemFeed { ring, opener: Some(opener), gen_: None, target: target_frames, done: false }
    }

    /// Fills the ring up to its target. Returns true if any work was done.
    pub fn pump(&mut self) -> bool {
        if self.done {
            return false;
        }
        if self.gen_.is_none() {
            if let Some(op) = self.opener.take() {
                match op() {
                    Ok(g) => {
                        self.ring.lock().info = Some(g.info);
                        self.gen_ = Some(g);
                    }
                    Err(e) => {
                        self.ring.lock().failed = Some(e.to_string());
                        self.done = true;
                        return true;
                    }
                }
            }
        }
        let mut worked = false;
        let mut chunk: Vec<Frame> = Vec::with_capacity(PRODUCE_CHUNK);
        while let Some(g) = self.gen_.as_mut() {
            if self.ring.available() >= self.target {
                break;
            }
            chunk.clear();
            let more = g.produce(&mut chunk, PRODUCE_CHUNK);
            {
                let mut r = self.ring.lock();
                r.produced += chunk.len() as u64;
                r.data.extend(chunk.drain(..));
                if !more {
                    r.eof = true;
                }
            }
            worked = true;
            if !more {
                self.done = true;
                self.gen_ = None;
                break;
            }
        }
        worked
    }

    pub fn ring(&self) -> &Arc<Ring> {
        &self.ring
    }

    /// The consumer dropped its end of the ring.
    fn orphaned(&self) -> bool {
        Arc::strong_count(&self.ring) <= 1
    }
}

struct Shared {
    feeds: Mutex<Vec<Arc<Mutex<StemFeed>>>>,
    stop: AtomicBool,
    cv: Condvar,
    cv_lock: Mutex<()>,
}

/// Owns the prefetch worker (threaded mode) or pumps synchronously (manual mode).
pub struct Prefetch {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Prefetch {
    pub fn new(threaded: bool) -> Prefetch {
        let shared = Arc::new(Shared { feeds: Mutex::new(Vec::new()), stop: AtomicBool::new(false), cv: Condvar::new(), cv_lock: Mutex::new(()) });
        let thread = if threaded {
            let w: Weak<Shared> = Arc::downgrade(&shared);
            Some(
                std::thread::Builder::new()
                    .name("bm-music-prefetch".into())
                    .spawn(move || Prefetch::worker(w))
                    .expect("spawn prefetch thread"),
            )
        } else {
            None
        };
        Prefetch { shared, thread }
    }

    pub fn is_threaded(&self) -> bool {
        self.thread.is_some()
    }

    fn worker(w: Weak<Shared>) {
        loop {
            let Some(sh) = w.upgrade() else { return };
            if sh.stop.load(Ordering::Relaxed) {
                return;
            }
            let worked = Prefetch::pump_shared(&sh);
            if !worked {
                let g = sh.cv_lock.lock().unwrap_or_else(|p| p.into_inner());
                let _ = sh.cv.wait_timeout(g, Duration::from_millis(5));
            }
        }
    }

    fn pump_shared(sh: &Shared) -> bool {
        let feeds: Vec<Arc<Mutex<StemFeed>>> = {
            let mut f = sh.feeds.lock().unwrap_or_else(|p| p.into_inner());
            f.retain(|x| x.try_lock().map(|g| !g.orphaned()).unwrap_or(true));
            f.clone()
        };
        let mut worked = false;
        for f in feeds {
            if let Ok(mut g) = f.try_lock() {
                worked |= g.pump();
            }
        }
        worked
    }

    pub fn add(&self, feed: StemFeed) {
        self.shared.feeds.lock().unwrap_or_else(|p| p.into_inner()).push(Arc::new(Mutex::new(feed)));
        self.shared.cv.notify_all();
    }

    /// Manual mode: fill all rings now. (No-op work in threaded mode apart from waking the worker.)
    pub fn pump(&self) {
        if self.thread.is_some() {
            self.shared.cv.notify_all();
        } else {
            while Prefetch::pump_shared(&self.shared) {}
        }
    }
}

impl Drop for Prefetch {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.cv.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::TrackDef;
    use crate::source::MemSource;

    fn feed(frames: usize, target: usize) -> (Arc<Ring>, StemFeed) {
        let ring = Arc::new(Ring::default());
        let t = TrackDef { looping: false, ..Default::default() };
        let data: Vec<f32> = (0..frames).map(|i| i as f32).collect();
        let f = StemFeed::new(
            ring.clone(),
            Box::new(move || Ok(StemGen::new(None, (Box::new(MemSource::from_f32(1000, 1, data)), &t), 1000, 0.0))),
            target,
        );
        (ring, f)
    }

    #[test]
    fn manual_pump_fills_to_target_and_ends() {
        let (ring, mut f) = feed(10_000, 3000);
        assert!(f.pump());
        assert!(ring.available() >= 3000 && ring.available() < 3000 + PRODUCE_CHUNK);
        let mut out = vec![[0.0; 2]; 100];
        assert_eq!(ring.pop(&mut out), 100);
        assert_eq!(out[99][0], 99.0);
        for _ in 0..20 {
            let mut sink = vec![[0.0; 2]; 4000];
            ring.pop(&mut sink);
            f.pump();
        }
        assert!(ring.ready(1) || ring.drained());
    }

    #[test]
    fn open_failure_is_reported_not_panicked() {
        let ring = Arc::new(Ring::default());
        let mut f = StemFeed::new(ring.clone(), Box::new(|| Err(crate::error::MusicError::Track("x".into(), "missing".into()))), 100);
        f.pump();
        assert!(ring.failed().is_some());
        assert!(ring.drained());
    }

    #[test]
    fn threaded_prefetch_delivers_and_stops_on_drop() {
        let pf = Prefetch::new(true);
        let (ring, f) = feed(50_000, 4000);
        pf.add(f);
        let t0 = std::time::Instant::now();
        while !ring.ready(4000) && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(ring.available() >= 4000);
        drop(pf);
    }
}
