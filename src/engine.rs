//! Playback engine. Owns the audio device and the queue on its own thread so
//! tracks keep advancing while the window is hidden and the UI isn't running.

use std::fs::File;
use std::io::BufReader;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::source::SeekError;
use rodio::{ChannelCount, Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Sample, SampleRate, Source};

use crate::library::Track;
use crate::stream::{self, Ready, StreamBuf};

pub enum Cmd {
    Play { queue: Vec<Arc<Track>>, index: usize, paused: bool, seek_ms: u64 },
    PlayNext(Vec<Arc<Track>>),
    Enqueue(Vec<Arc<Track>>),
    Toggle,
    Next,
    Prev,
    Seek(u64),
    SeekBy(i64),
    Volume(f32),
    Shuffle(bool),
    Repeat(u8),
    /// A streamed track has enough audio to start (sent by its waiter thread).
    Ready(u64),
}

/// Repeat modes, stored as u8 in the config.
pub const REPEAT_OFF: u8 = 0;
pub const REPEAT_ALL: u8 = 1;
pub const REPEAT_ONE: u8 = 2;

#[derive(Default)]
pub struct Shared {
    pub current: Mutex<Option<Arc<Track>>>,
    /// Bumped whenever `current` changes.
    pub generation: AtomicU64,
    pub playing: AtomicBool,
    pub pos_ms: AtomicU64,
    /// Waiting for a streamed track's first audio to arrive.
    pub buffering: AtomicBool,
    /// Smoothed output level 0..1 as f32 bits, written from the audio thread.
    pub level: AtomicU32,
    pub error: Mutex<Option<String>>,
}

impl Shared {
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

#[derive(Clone)]
pub struct Engine {
    tx: Sender<Cmd>,
    pub shared: Arc<Shared>,
}

impl Engine {
    pub fn spawn(wake: impl Fn() + Send + 'static, volume: f32, shuffle: bool, repeat: u8) -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let s = shared.clone();
        std::thread::Builder::new()
            .name("wax-audio".into())
            .spawn({
                let tx = tx.clone();
                move || Worker::new(rx, tx, s, Box::new(wake), volume, shuffle, repeat).run()
            })
            .expect("spawn audio thread");
        Self { tx, shared }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

struct Worker {
    rx: Receiver<Cmd>,
    tx: Sender<Cmd>,
    /// Bumped on every track start, so late `Ready`s for old tracks are ignored.
    token: u64,
    /// Start options held while a stream buffers: (paused, seek_ms).
    pending: Option<(bool, u64)>,
    /// The current track's stream, if it is one.
    stream: Option<Arc<StreamBuf>>,
    /// Playing from a still-downloading stream: seeking needs a rebuild.
    unseekable: bool,
    shared: Arc<Shared>,
    wake: Box<dyn Fn() + Send>,
    device: Option<MixerDeviceSink>,
    player: Option<Player>,
    queue: Vec<Arc<Track>>,
    /// Play order: indices into `queue`. Identity unless shuffling.
    order: Vec<usize>,
    cursor: usize,
    volume: f32,
    shuffle: bool,
    repeat: u8,
    rng: u64,
}

impl Worker {
    fn new(
        rx: Receiver<Cmd>,
        tx: Sender<Cmd>,
        shared: Arc<Shared>,
        wake: Box<dyn Fn() + Send>,
        volume: f32,
        shuffle: bool,
        repeat: u8,
    ) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9E37_79B9, |d| d.as_nanos() as u64)
            | 1;
        Self {
            rx,
            tx,
            token: 0,
            pending: None,
            stream: None,
            unseekable: false,
            shared,
            wake,
            device: None,
            player: None,
            queue: Vec::new(),
            order: Vec::new(),
            cursor: 0,
            volume,
            shuffle,
            repeat,
            rng: seed,
        }
    }

    fn run(mut self) {
        loop {
            let playing = self.shared.playing.load(Ordering::Relaxed);
            let cmd = if playing {
                match self.rx.recv_timeout(Duration::from_millis(30)) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                match self.rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            };
            if let Some(cmd) = cmd {
                self.handle(cmd);
            }
            self.tick();
        }
    }

    fn tick(&mut self) {
        let Some(player) = &self.player else { return };
        self.shared.pos_ms.store(player.get_pos().as_millis() as u64, Ordering::Relaxed);
        if player.empty() && self.shared.playing.load(Ordering::Relaxed) {
            self.track_ended();
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Play { queue, index, paused, seek_ms } => {
                if queue.is_empty() {
                    return;
                }
                self.queue = queue;
                let index = index.min(self.queue.len() - 1);
                self.rebuild_order(index);
                self.start_current(paused, seek_ms);
            }
            Cmd::PlayNext(tracks) => self.insert(tracks, true),
            Cmd::Enqueue(tracks) => self.insert(tracks, false),
            Cmd::Toggle => {
                if let Some((paused, _)) = &mut self.pending {
                    *paused = !*paused;
                } else if let Some(p) = &self.player {
                    if p.empty() {
                        self.start_current(false, 0);
                    } else if p.is_paused() {
                        p.play();
                        self.shared.playing.store(true, Ordering::Relaxed);
                    } else {
                        p.pause();
                        self.shared.playing.store(false, Ordering::Relaxed);
                        self.shared.level.store(0, Ordering::Relaxed);
                    }
                } else if !self.queue.is_empty() {
                    self.start_current(false, 0);
                }
            }
            Cmd::Next => self.step(1, true),
            Cmd::Prev => {
                let pos = self.player.as_ref().map_or(0, |p| p.get_pos().as_millis());
                if pos > 3000 || self.cursor == 0 {
                    self.seek(0);
                } else {
                    self.step(-1, true);
                }
            }
            Cmd::Seek(ms) => self.seek(ms),
            Cmd::SeekBy(delta) => {
                let pos = self.player.as_ref().map_or(0, |p| p.get_pos().as_millis() as i64);
                self.seek((pos + delta).max(0) as u64);
            }
            Cmd::Volume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                if let Some(p) = &self.player {
                    p.set_volume(gain(self.volume));
                }
            }
            Cmd::Shuffle(on) => {
                if self.shuffle != on {
                    self.shuffle = on;
                    if !self.order.is_empty() {
                        let current = self.order[self.cursor];
                        self.rebuild_order(current);
                    }
                }
            }
            Cmd::Repeat(mode) => self.repeat = mode,
            Cmd::Ready(token) => {
                if token == self.token {
                    self.stream_ready();
                }
            }
        }
    }

    fn insert(&mut self, tracks: Vec<Arc<Track>>, next: bool) {
        if self.queue.is_empty() {
            self.handle(Cmd::Play { queue: tracks, index: 0, paused: false, seek_ms: 0 });
            return;
        }
        let at = if next { self.cursor + 1 } else { self.order.len() };
        for (i, t) in tracks.into_iter().enumerate() {
            self.queue.push(t);
            self.order.insert(at + i, self.queue.len() - 1);
        }
    }

    fn rebuild_order(&mut self, current: usize) {
        self.order = (0..self.queue.len()).collect();
        if self.shuffle {
            // Fisher–Yates, then pull the current track to the front.
            for i in (1..self.order.len()).rev() {
                let j = (self.next_rand() % (i as u64 + 1)) as usize;
                self.order.swap(i, j);
            }
            if let Some(p) = self.order.iter().position(|&i| i == current) {
                self.order.swap(0, p);
            }
            self.cursor = 0;
        } else {
            self.cursor = current;
        }
    }

    fn next_rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn track_ended(&mut self) {
        if self.repeat == REPEAT_ONE {
            self.start_current(false, 0);
        } else {
            self.step(1, false);
        }
    }

    /// Move through the queue. `manual` steps always wrap; natural endings
    /// only wrap when repeating the whole queue.
    fn step(&mut self, dir: i32, manual: bool) {
        if self.order.is_empty() {
            return;
        }
        let len = self.order.len();
        let next = self.cursor as i64 + dir as i64;
        if next >= len as i64 {
            if !manual && self.repeat == REPEAT_OFF {
                self.cursor = 0;
                self.start_current(true, 0);
                return;
            }
            let last = self.order[self.cursor];
            self.rebuild_order(0);
            // Avoid replaying the same track right after a reshuffle.
            if self.shuffle && len > 1 && self.order[0] == last {
                self.order.swap(0, 1);
            }
            self.cursor = 0;
        } else {
            self.cursor = next.max(0) as usize;
        }
        self.start_current(false, 0);
    }

    fn seek(&mut self, ms: u64) {
        if let Some((_, seek)) = &mut self.pending {
            *seek = ms;
            return;
        }
        if self.unseekable {
            // Rebuild from the finished download, which can seek.
            let done = self.stream.as_ref().is_some_and(|b| matches!(b.status(), Ready::Done));
            if done {
                let paused = self.player.as_ref().is_none_or(|p| p.is_paused());
                self.restart_stream(paused, ms);
            } else {
                *self.shared.error.lock().unwrap() = Some("Still streaming in, seek again in a moment".into());
                (self.wake)();
            }
            return;
        }
        if let Some(p) = &self.player {
            if p.empty() {
                self.start_current(!self.shared.playing.load(Ordering::Relaxed), ms);
                return;
            }
            let _ = p.try_seek(Duration::from_millis(ms));
            self.shared.pos_ms.store(ms, Ordering::Relaxed);
        }
    }

    /// Load the track under the cursor, skipping over files that fail to open.
    fn start_current(&mut self, paused: bool, seek_ms: u64) {
        self.token += 1;
        self.pending = None;
        self.stream = None;
        self.unseekable = false;
        for _ in 0..self.order.len().max(1) {
            let Some(track) = self.order.get(self.cursor).map(|&i| self.queue[i].clone()) else { return };
            if let Some(id) = track.stream_id() {
                self.begin_stream(&track, id, paused, seek_ms);
                return;
            }
            match self.load(&track, paused, seek_ms) {
                Ok(()) => {
                    self.shared.buffering.store(false, Ordering::Relaxed);
                    self.prefetch_next();
                    *self.shared.current.lock().unwrap() = Some(track);
                    self.shared.pos_ms.store(seek_ms, Ordering::Relaxed);
                    self.shared.playing.store(!paused, Ordering::Relaxed);
                    self.shared.generation.fetch_add(1, Ordering::Relaxed);
                    (self.wake)();
                    return;
                }
                Err(e) => {
                    *self.shared.error.lock().unwrap() = Some(format!("{}: {e}", track.title));
                    self.cursor = (self.cursor + 1) % self.order.len();
                }
            }
        }
        self.player = None;
        self.shared.buffering.store(false, Ordering::Relaxed);
        self.shared.playing.store(false, Ordering::Relaxed);
        (self.wake)();
    }

    fn load(&mut self, track: &Track, paused: bool, seek_ms: u64) -> Result<(), String> {
        if self.device.is_none() {
            let mut device = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
            device.log_on_drop(false);
            self.device = Some(device);
        }
        let file = File::open(&track.path).map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        let mut builder = Decoder::builder()
            .with_data(BufReader::with_capacity(64 * 1024, file))
            .with_byte_len(len)
            .with_seekable(true)
            .with_gapless(true);
        if let Some(ext) = track.path.extension().and_then(|e| e.to_str()) {
            builder = builder.with_hint(ext);
        }
        let decoder = builder.build().map_err(|e| e.to_string())?;
        self.attach(decoder, paused, seek_ms);
        Ok(())
    }

    /// Show the streamed track right away, then start it once audio arrives.
    fn begin_stream(&mut self, track: &Arc<Track>, id: &str, paused: bool, seek_ms: u64) {
        self.player = None; // silence the previous track immediately
        let buf = stream::fetch(id);
        self.stream = Some(buf.clone());
        self.pending = Some((paused, seek_ms));
        *self.shared.current.lock().unwrap() = Some(track.clone());
        self.shared.pos_ms.store(seek_ms, Ordering::Relaxed);
        self.shared.playing.store(false, Ordering::Relaxed);
        self.shared.level.store(0, Ordering::Relaxed);
        self.shared.buffering.store(true, Ordering::Relaxed);
        self.shared.generation.fetch_add(1, Ordering::Relaxed);
        (self.wake)();
        let (tx, token) = (self.tx.clone(), self.token);
        std::thread::spawn(move || {
            buf.wait_for(stream::START_BYTES, Duration::from_secs(90));
            let _ = tx.send(Cmd::Ready(token));
        });
    }

    fn stream_ready(&mut self) {
        let (Some(buf), Some((paused, seek_ms))) = (self.stream.clone(), self.pending.take()) else { return };
        let failed = match buf.status() {
            Ready::Failed(e) => Some(e),
            _ if buf.len() == 0 => Some("no audio arrived".to_string()),
            _ => None,
        };
        let title = self.shared.current.lock().unwrap().as_ref().map(|t| t.title.clone()).unwrap_or_default();
        let result = match failed {
            Some(e) => Err(e),
            None => self.build_stream(&buf, paused, seek_ms),
        };
        match result {
            Ok(()) => {
                self.shared.buffering.store(false, Ordering::Relaxed);
                self.shared.playing.store(!paused, Ordering::Relaxed);
                self.shared.generation.fetch_add(1, Ordering::Relaxed);
                self.prefetch_next();
                (self.wake)();
            }
            Err(e) => {
                *self.shared.error.lock().unwrap() = Some(format!("{title}: {e}"));
                self.shared.buffering.store(false, Ordering::Relaxed);
                if self.cursor + 1 < self.order.len() {
                    self.cursor += 1;
                    self.start_current(false, 0);
                } else {
                    self.stream = None;
                    (self.wake)();
                }
            }
        }
    }

    fn restart_stream(&mut self, paused: bool, seek_ms: u64) {
        if let Some(buf) = self.stream.clone() {
            if let Err(e) = self.build_stream(&buf, paused, seek_ms) {
                *self.shared.error.lock().unwrap() = Some(e);
            }
            self.shared.pos_ms.store(seek_ms, Ordering::Relaxed);
            (self.wake)();
        }
    }

    fn build_stream(&mut self, buf: &Arc<StreamBuf>, paused: bool, seek_ms: u64) -> Result<(), String> {
        if self.device.is_none() {
            let mut device = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
            device.log_on_drop(false);
            self.device = Some(device);
        }
        // Until the download completes the decoder must read straight through.
        let complete = matches!(buf.status(), Ready::Done);
        let mut builder = Decoder::builder().with_data(buf.reader()).with_hint("m4a").with_seekable(complete);
        if complete {
            builder = builder.with_byte_len(buf.len() as u64);
        }
        let decoder = builder.build().map_err(|e| e.to_string())?;
        self.unseekable = !complete;
        self.attach(decoder, paused, if complete { seek_ms } else { 0 });
        Ok(())
    }

    /// Warm up the next streamed track so it starts without a wait.
    fn prefetch_next(&self) {
        if let Some(&i) = self.order.get(self.cursor + 1) {
            if let Some(id) = self.queue[i].stream_id() {
                stream::fetch(id);
            }
        }
    }

    fn attach<S: Source + Send + 'static>(&mut self, source: S, paused: bool, seek_ms: u64) {
        // A fresh Player per track: dropping the old one stops it instantly.
        let player = Player::connect_new(self.device.as_ref().unwrap().mixer());
        player.set_volume(gain(self.volume));
        if paused {
            player.pause();
        }
        player.append(LevelTap::new(source, self.shared.clone()));
        if seek_ms > 0 {
            let _ = player.try_seek(Duration::from_millis(seek_ms));
        }
        self.player = Some(player);
    }
}

/// Perceptual volume curve.
fn gain(v: f32) -> f32 {
    v * v
}

/// Pass-through source that publishes a smoothed peak level for the visuals.
struct LevelTap<S> {
    inner: S,
    shared: Arc<Shared>,
    peak: f32,
    count: u32,
    smooth: f32,
}

impl<S> LevelTap<S> {
    fn new(inner: S, shared: Arc<Shared>) -> Self {
        Self { inner, shared, peak: 0.0, count: 0, smooth: 0.0 }
    }
}

impl<S: Source> Iterator for LevelTap<S> {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        let s = self.inner.next()?;
        self.peak = self.peak.max(s.abs());
        self.count += 1;
        if self.count >= 2048 {
            // Fast attack, slow release.
            let k = if self.peak > self.smooth { 0.6 } else { 0.12 };
            self.smooth += (self.peak - self.smooth) * k;
            self.shared.level.store(self.smooth.min(1.0).to_bits(), Ordering::Relaxed);
            self.peak = 0.0;
            self.count = 0;
        }
        Some(s)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for LevelTap<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)
    }
}
