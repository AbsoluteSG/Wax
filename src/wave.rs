//! Waveform overviews: one background decode per track, cached as a few
//! hundred bytes on disk so each track is only ever analysed once.

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use rodio::{Decoder, Source};

use crate::library::{data_dir, Track};
use crate::stream::{Ready, StreamBuf};

type Job = (Arc<Track>, Option<Arc<StreamBuf>>);

/// Number of bins stored per track.
pub const BINS: usize = 720;

pub struct WaveWorker {
    tx: Sender<Job>,
    /// (track key, peaks, measured length in ms or 0).
    pub rx: Receiver<(u64, Arc<[u8]>, u32)>,
    /// Key of the track the UI currently cares about; stale jobs bail out.
    wanted: Arc<AtomicU64>,
}

impl WaveWorker {
    pub fn spawn(wake: impl Fn() + Send + 'static) -> Self {
        let (tx, job_rx) = mpsc::channel::<Job>();
        let (out_tx, rx) = mpsc::channel();
        let wanted = Arc::new(AtomicU64::new(0));
        let w = wanted.clone();
        std::thread::Builder::new()
            .name("wax-wave".into())
            .spawn(move || {
                let dir = data_dir().join("wave");
                while let Ok((track, stream)) = job_rx.recv() {
                    let key = track.key();
                    if w.load(Ordering::Relaxed) != key {
                        continue;
                    }
                    // Streams are analysed from memory once fully arrived, never cached.
                    if let Some(buf) = stream {
                        let peaks = wait_done(&buf, key, &w).and_then(|()| analyse_stream(&buf, key, &w));
                        if let Some((p, ms)) = peaks {
                            if out_tx.send((key, p.into(), ms)).is_err() {
                                return;
                            }
                            wake();
                        }
                        continue;
                    }
                    let cache = dir.join(format!("{key:016x}.bin"));
                    let peaks = match std::fs::read(&cache) {
                        Ok(bytes) if bytes.len() == BINS => Some(bytes),
                        _ => analyse(&track.path, key, &w).map(|(p, _)| p).inspect(|p| {
                            let _ = std::fs::write(&cache, p);
                        }),
                    };
                    if let Some(p) = peaks {
                        if out_tx.send((key, p.into(), 0)).is_err() {
                            return;
                        }
                        wake();
                    }
                }
            })
            .expect("spawn wave thread");
        Self { tx, rx, wanted }
    }

    pub fn request(&self, track: &Arc<Track>, stream: Option<Arc<StreamBuf>>) {
        self.wanted.store(track.key(), Ordering::Relaxed);
        let _ = self.tx.send((track.clone(), stream));
    }
}

fn wait_done(buf: &StreamBuf, key: u64, wanted: &AtomicU64) -> Option<()> {
    loop {
        match buf.status() {
            Ready::Done => return Some(()),
            Ready::Failed(_) => return None,
            Ready::Partial => {}
        }
        if wanted.load(Ordering::Relaxed) != key {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

fn analyse_stream(buf: &std::sync::Arc<StreamBuf>, key: u64, wanted: &AtomicU64) -> Option<(Vec<u8>, u32)> {
    let decoder = Decoder::builder()
        .with_data(buf.reader())
        .with_byte_len(buf.len() as u64)
        .with_hint("m4a")
        .build()
        .ok()?;
    peaks(decoder, key, wanted)
}

fn analyse(path: &PathBuf, key: u64, wanted: &AtomicU64) -> Option<(Vec<u8>, u32)> {
    let file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut builder = Decoder::builder().with_data(BufReader::with_capacity(256 * 1024, file)).with_byte_len(len);
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        builder = builder.with_hint(ext);
    }
    let decoder = builder.build().ok()?;
    peaks(decoder, key, wanted)
}

fn peaks<S: Source>(decoder: S, key: u64, wanted: &AtomicU64) -> Option<(Vec<u8>, u32)> {
    let per_sec = decoder.channels().get() as u64 * decoder.sample_rate().get() as u64;
    let mut total = 0u64;
    // RMS over fixed chunks; resampled to BINS afterwards so we don't need
    // to trust the container's duration.
    let chunk = decoder.channels().get() as usize * 2048;
    let mut rms = Vec::with_capacity(4096);
    let (mut sum, mut n) = (0.0f32, 0usize);
    for s in decoder {
        total += 1;
        sum += s * s;
        n += 1;
        if n == chunk {
            rms.push((sum / n as f32).sqrt());
            sum = 0.0;
            n = 0;
            if rms.len() % 64 == 0 && wanted.load(Ordering::Relaxed) != key {
                return None;
            }
        }
    }
    if n > chunk / 4 {
        rms.push((sum / n as f32).sqrt());
    }
    if rms.is_empty() {
        return None;
    }

    let max = rms.iter().copied().fold(1e-6f32, f32::max);
    let mut out = Vec::with_capacity(BINS);
    for i in 0..BINS {
        let a = i * rms.len() / BINS;
        let b = ((i + 1) * rms.len() / BINS).max(a + 1).min(rms.len());
        let v = rms[a..b].iter().copied().fold(0.0f32, f32::max) / max;
        out.push((v.powf(0.85) * 255.0) as u8);
    }
    Some((out, (total * 1000 / per_sec.max(1)) as u32))
}
