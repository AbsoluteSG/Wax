//! Cover art: extracted and downscaled off the UI thread, thumbnails cached
//! on disk, textures evicted when they scroll out of use.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use egui::ecolor::Hsva;
use lofty::picture::PictureType;
use lofty::prelude::*;

use crate::library::data_dir;

const THUMB: u32 = 192;
const LARGE: u32 = 512;
/// Thumbnails kept on the GPU before the least recently drawn are dropped.
const MAX_THUMBS: usize = 160;
/// Pending decode requests; older ones are dropped when scrolling fast.
const MAX_PENDING: usize = 48;

#[derive(Clone, Copy, PartialEq)]
pub struct Palette {
    pub a: Color32,
    pub b: Color32,
}

impl Palette {
    /// Deterministic colours for records with no artwork.
    pub fn from_key(key: u64) -> Self {
        let h = (key % 360) as f32 / 360.0;
        let shift = 0.08 + ((key >> 12) % 100) as f32 / 100.0 * 0.22;
        Self { a: hsv(h, 0.62, 0.85), b: hsv((h + shift).fract(), 0.7, 0.72) }
    }
}

fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    let [r, g, b] = Hsva::new(h, s, v, 1.0).to_srgb();
    Color32::from_rgb(r, g, b)
}

struct Job {
    key: u64,
    track: PathBuf,
    large: bool,
}

enum Done {
    Image { key: u64, large: bool, image: Option<ColorImage>, palette: Option<Palette> },
    Dropped { key: u64 },
}

struct Stack {
    jobs: Mutex<Vec<Job>>,
    ready: Condvar,
}

enum Slot {
    Pending,
    Missing,
    Ready(TextureHandle),
}

struct Entry {
    slot: Slot,
    palette: Palette,
    last_used: u64,
}

pub struct ArtCache {
    stack: Arc<Stack>,
    rx: Receiver<Done>,
    thumbs: HashMap<u64, Entry>,
    /// Full-size cover for the record on the platter.
    large: Option<(u64, Option<TextureHandle>)>,
    frame: u64,
}

impl ArtCache {
    pub fn spawn(ctx: &egui::Context) -> Self {
        let stack = Arc::new(Stack { jobs: Mutex::new(Vec::new()), ready: Condvar::new() });
        let (tx, rx) = mpsc::channel();
        for i in 0..2 {
            let (stack, tx, ctx) = (stack.clone(), tx.clone(), ctx.clone());
            std::thread::Builder::new()
                .name(format!("wax-art-{i}"))
                .spawn(move || worker(stack, tx, ctx))
                .expect("spawn art thread");
        }
        Self { stack, rx, thumbs: HashMap::new(), large: None, frame: 0 }
    }

    /// Call once per frame: uploads finished images and evicts stale ones.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        while let Ok(done) = self.rx.try_recv() {
            match done {
                Done::Dropped { key } => {
                    self.thumbs.remove(&key);
                }
                Done::Image { key, large: true, image, .. } => {
                    if let Some((k, slot)) = &mut self.large {
                        if *k == key {
                            *slot = image.map(|img| ctx.load_texture("cover", img, TextureOptions::LINEAR));
                        }
                    }
                }
                Done::Image { key, large: false, image, palette } => {
                    if let Some(e) = self.thumbs.get_mut(&key) {
                        if let Some(p) = palette {
                            e.palette = p;
                        }
                        e.slot = match image {
                            Some(img) => Slot::Ready(ctx.load_texture("thumb", img, TextureOptions::LINEAR)),
                            None => Slot::Missing,
                        };
                    }
                }
            }
        }
        if self.thumbs.len() > MAX_THUMBS && self.frame % 120 == 0 {
            let cutoff = self.frame.saturating_sub(240);
            self.thumbs.retain(|_, e| e.last_used >= cutoff || matches!(e.slot, Slot::Pending));
        }
    }

    /// Thumbnail + palette for an album; kicks off loading on first use.
    pub fn thumb(&mut self, key: u64, track: &Path) -> (Option<egui::TextureId>, Palette) {
        let frame = self.frame;
        let entry = self.thumbs.entry(key).or_insert_with(|| {
            push(&self.stack, Job { key, track: track.to_owned(), large: false });
            Entry { slot: Slot::Pending, palette: Palette::from_key(key), last_used: frame }
        });
        entry.last_used = frame;
        let tex = match &entry.slot {
            Slot::Ready(t) => Some(t.id()),
            _ => None,
        };
        (tex, entry.palette)
    }

    /// Ask for the full-size cover of the album now playing.
    pub fn set_large(&mut self, key: u64, track: &Path) {
        if self.large.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        self.large = Some((key, None));
        push(&self.stack, Job { key, track: track.to_owned(), large: true });
    }

    pub fn large(&self, key: u64) -> Option<egui::TextureId> {
        match &self.large {
            Some((k, Some(t))) if *k == key => Some(t.id()),
            _ => None,
        }
    }
}

fn push(stack: &Stack, job: Job) {
    stack.jobs.lock().unwrap().push(job);
    stack.ready.notify_one();
}

fn worker(stack: Arc<Stack>, tx: Sender<Done>, ctx: egui::Context) {
    let dir = data_dir().join("art");
    loop {
        let job = {
            let mut jobs = stack.jobs.lock().unwrap();
            loop {
                // Newest first: whatever just scrolled into view matters most.
                if let Some(job) = jobs.pop() {
                    if jobs.len() > MAX_PENDING {
                        let extra = jobs.len() - MAX_PENDING;
                        let stale: Vec<Job> = jobs.drain(..extra).collect();
                        for j in stale.into_iter().filter(|j| !j.large) {
                            let _ = tx.send(Done::Dropped { key: j.key });
                        }
                    }
                    break job;
                }
                jobs = stack.ready.wait(jobs).unwrap();
            }
        };

        let stream = job.track.to_str().and_then(|p| p.strip_prefix(crate::stream::PREFIX)).map(str::to_owned);
        let done = if let Some(id) = stream {
            // Streamed songs: YouTube's thumbnail, kept in memory only.
            let size = if job.large { LARGE } else { THUMB };
            let image = crate::stream::thumbnail(&id).map(|img| {
                let img = img.resize_to_fill(size, size, image::imageops::FilterType::Triangle).to_rgb8();
                ColorImage::from_rgb([size as usize, size as usize], img.as_raw())
            });
            let palette = image.as_ref().filter(|_| !job.large).map(extract_palette);
            Done::Image { key: job.key, large: job.large, image, palette }
        } else if job.large {
            let image = find_cover(&job.track).and_then(|b| decode(&b, LARGE));
            Done::Image { key: job.key, large: true, image, palette: None }
        } else {
            let image = load_thumb(&dir.join(format!("{:016x}.jpg", job.key)), &job.track);
            let palette = image.as_ref().map(extract_palette);
            Done::Image { key: job.key, large: false, image, palette }
        };
        if tx.send(done).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Read the cached thumbnail, or build and cache it. An empty cache file
/// records "this album has no artwork" so we don't probe again.
fn load_thumb(cache: &Path, track: &Path) -> Option<ColorImage> {
    if let Ok(bytes) = std::fs::read(cache) {
        return if bytes.is_empty() { None } else { decode(&bytes, THUMB) };
    }
    let Some(img) = find_cover(track).and_then(|b| image::load_from_memory(&b).ok()) else {
        let _ = std::fs::write(cache, []);
        return None;
    };
    let thumb = img.resize_to_fill(THUMB, THUMB, image::imageops::FilterType::Triangle).to_rgb8();
    let _ = thumb.save_with_format(cache, image::ImageFormat::Jpeg);
    Some(ColorImage::from_rgb([THUMB as usize, THUMB as usize], thumb.as_raw()))
}

fn decode(bytes: &[u8], size: u32) -> Option<ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() != size || img.height() != size {
        img.resize_to_fill(size, size, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let rgb = img.to_rgb8();
    Some(ColorImage::from_rgb([size as usize, size as usize], rgb.as_raw()))
}

/// Embedded picture first, then the usual image files next to the track.
fn find_cover(track: &Path) -> Option<Vec<u8>> {
    if let Ok(file) = lofty::read_from_path(track) {
        let pics = file.tags().iter().flat_map(|t| t.pictures());
        let mut fallback = None;
        for p in pics {
            if p.pic_type() == PictureType::CoverFront {
                return Some(p.data().to_vec());
            }
            fallback.get_or_insert(p);
        }
        if let Some(p) = fallback {
            return Some(p.data().to_vec());
        }
    }
    let dir = track.parent()?;
    let mut best: Option<(u8, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "bmp") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        let rank = match stem.as_str() {
            "cover" | "folder" | "front" => 0,
            s if s.contains("cover") || s.contains("front") => 1,
            _ => 2,
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, path));
        }
    }
    std::fs::read(best?.1).ok()
}

/// Two dominant, well-separated hues, tuned to stay vivid behind white text.
fn extract_palette(img: &ColorImage) -> Palette {
    const BUCKETS: usize = 12;
    let mut weight = [0.0f32; BUCKETS];
    let mut sum = [[0.0f32; 3]; BUCKETS];
    for px in img.pixels.iter().step_by(7) {
        let c = Hsva::from_srgb([px.r(), px.g(), px.b()]);
        let w = c.s * c.s * c.v;
        if w < 0.02 {
            continue;
        }
        let b = ((c.h * BUCKETS as f32) as usize).min(BUCKETS - 1);
        weight[b] += w;
        sum[b][0] += c.h * w;
        sum[b][1] += c.s * w;
        sum[b][2] += c.v * w;
    }
    let total: f32 = weight.iter().sum();
    let samples = (img.pixels.len() / 7).max(1) as f32;
    if total / samples < 0.01 {
        // Monochrome cover: a neutral red accent on charcoal.
        return Palette { a: Color32::from_rgb(214, 58, 66), b: Color32::from_rgb(88, 34, 44) };
    }
    let color = |b: usize, v_min: f32| {
        let w = weight[b];
        hsv(sum[b][0] / w, (sum[b][1] / w).clamp(0.45, 0.85), (sum[b][2] / w).clamp(v_min, 0.9))
    };
    let first = (0..BUCKETS).max_by(|&x, &y| weight[x].total_cmp(&weight[y])).unwrap();
    let second = (0..BUCKETS)
        .filter(|&b| {
            let d = (b as i32 - first as i32).rem_euclid(BUCKETS as i32);
            d.min(BUCKETS as i32 - d) >= 2 && weight[b] > weight[first] * 0.12
        })
        .max_by(|&x, &y| weight[x].total_cmp(&weight[y]));
    let a = color(first, 0.7);
    let b = match second {
        Some(s) => color(s, 0.55),
        None => {
            let c = Hsva::from_srgb([a.r(), a.g(), a.b()]);
            // One-hue cover: pair it with a deeper neighbour on the warm side.
            hsv((c.h + 0.93).fract(), (c.s + 0.12).min(0.9), c.v * 0.6)
        }
    };
    Palette { a, b }
}
