//! The record's grooves, sheen, rim and drop shadow, baked into one texture.
//!
//! None of these move while a song plays (only the label spins), so drawing
//! them as dozens of anti-aliased circles every frame was the single biggest
//! per-frame cost. Now they're one textured quad, rendered off-thread and
//! redone only when the size tier or the record's track gaps change.

use std::f32::consts::PI;
use std::sync::mpsc::{self, Receiver};

use eframe::egui::{self, Color32, ColorImage, TextureFilter, TextureHandle, TextureId, TextureOptions, TextureWrapMode};

/// Radii as fractions of the disc radius (shared with the vector fallback).
pub const GROOVE_OUT: f32 = 0.955;
pub const GROOVE_IN: f32 = 0.43;
pub const LABEL: f32 = 0.36;
/// The texture covers the disc plus room for its offset shadow.
pub const EXTENT: f32 = 1.18;

#[derive(Default)]
pub struct DiscCache {
    tex: Option<(u64, TextureHandle)>,
    pending: Option<(u64, Receiver<ColorImage>)>,
}

impl DiscCache {
    /// Texture for a disc of `radius_px` physical pixels with these gaps.
    /// Returns the best texture available now; a better one may follow.
    pub fn get(&mut self, ctx: &egui::Context, radius_px: f32, gaps: &[f32]) -> Option<TextureId> {
        let size = ((radius_px * 2.0 * EXTENT).max(1.0) as u32).next_power_of_two().clamp(256, 2048) as usize;
        let key = key(size, gaps);
        if let Some((k, rx)) = &self.pending {
            if let Ok(img) = rx.try_recv() {
                let opts = TextureOptions {
                    magnification: TextureFilter::Linear,
                    minification: TextureFilter::Linear,
                    wrap_mode: TextureWrapMode::ClampToEdge,
                    mipmap_mode: Some(TextureFilter::Linear),
                };
                self.tex = Some((*k, ctx.load_texture("disc", img, opts)));
                self.pending = None;
            }
        }
        let have = self.tex.as_ref().map(|(k, _)| *k);
        let coming = self.pending.as_ref().map(|(k, _)| *k);
        if have != Some(key) && coming != Some(key) {
            let (tx, rx) = mpsc::channel();
            let gaps = gaps.to_vec();
            let ctx2 = ctx.clone();
            std::thread::Builder::new()
                .name("wax-disc".into())
                .spawn(move || {
                    let _ = tx.send(render(size, &gaps));
                    ctx2.request_repaint();
                })
                .ok();
            self.pending = Some((key, rx));
        }
        self.tex.as_ref().map(|(_, t)| t.id())
    }
}

fn key(size: usize, gaps: &[f32]) -> u64 {
    let mut h = crate::library::Fnv::new();
    h.write(&(size as u64).to_le_bytes());
    for g in gaps {
        h.write(&((g * 4096.0) as u32).to_le_bytes());
    }
    h.finish()
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Render the disc into a `size`² premultiplied RGBA image.
fn render(size: usize, gaps: &[f32]) -> ColorImage {
    let r_px = size as f32 / (2.0 * EXTENT); // disc radius in texture pixels
    let px = 1.0 / r_px; // one pixel, in disc units
    let span = GROOVE_OUT - GROOVE_IN;
    let rings = (r_px * span / 2.6).clamp(12.0, 64.0);
    let gap_r: Vec<f32> = gaps.iter().filter(|g| **g > 0.01 && **g < 0.99).map(|g| GROOVE_OUT - span * g).collect();
    let gap_w = (0.012f32).max(1.5 * px);
    let sheen_in = LABEL + 0.03;
    let half = size as f32 * 0.5;

    let mut out = vec![Color32::TRANSPARENT; size * size];
    for py in 0..size {
        let y = (py as f32 + 0.5 - half) * px;
        for pxi in 0..size {
            let x = (pxi as f32 + 0.5 - half) * px;
            let rho = (x * x + y * y).sqrt();

            // Soft drop shadow, offset down-right.
            let rs = ((x - 0.03).powi(2) + (y - 0.05).powi(2)).sqrt();
            let shadow = if rs < 1.12 { (110.0 / 255.0) * ((1.12 - rs) / 0.22).clamp(0.0, 1.0) } else { 0.0 };

            let cover = ((1.0 - rho) / px + 0.5).clamp(0.0, 1.0);
            if cover <= 0.0 {
                if shadow > 0.0 {
                    out[py * size + pxi] = Color32::from_rgba_premultiplied(0, 0, 0, (shadow * 255.0) as u8);
                }
                continue;
            }

            let mut rgb = [9.0 / 255.0, 9.0 / 255.0, 11.0 / 255.0];
            let mut add = 0.0f32; // white overlay alpha

            if rho >= GROOVE_IN && rho <= GROOVE_OUT {
                // Thin grooves whose brightness drifts with radius.
                let f = (GROOVE_OUT - rho) / span;
                let a = (9.0 + 9.0 * ((f * 37.0).sin() * 0.5 + 0.5) + 6.0 * ((f * 11.0 + 1.3).sin() * 0.5 + 0.5)) / 255.0;
                let t = (f * rings).fract();
                let dist = t.min(1.0 - t) * (span / rings) / px;
                add += a * (1.0 - dist).clamp(0.0, 1.0);
            }
            // The silent bands between tracks.
            for &g in &gap_r {
                let d = (rho - g).abs();
                let k = (gap_w * 0.5 - d) / px + 0.5;
                if k > 0.0 {
                    let k = k.clamp(0.0, 1.0);
                    for c in &mut rgb {
                        *c += (5.0 / 255.0 - *c) * k;
                    }
                    add *= 1.0 - k;
                }
            }
            // Light catching the grooves: two fixed wedges.
            if rho >= sheen_in && rho <= 0.985 {
                let a = y.atan2(x);
                let k = (a - PI * 0.28).cos().powi(2).powi(9);
                let k2 = (a - PI * 0.78).cos().powi(2).powi(40) * 0.35;
                let t = (rho - sheen_in) / (0.985 - sheen_in);
                add += (k * 0.16 + k2 * 0.1) * (0.55 + 0.45 * t) * smooth(0.0, 0.02, 0.985 - rho);
            }
            // A thin bright rim.
            let rim = (1.0 - (rho - (1.0 - 0.5 * px)).abs() / px).clamp(0.0, 1.0);
            add += rim * 26.0 / 255.0;

            let add = add.clamp(0.0, 1.0);
            for c in &mut rgb {
                *c = *c * (1.0 - add) + add;
            }
            // Disc over shadow (premultiplied).
            let a = cover + shadow * (1.0 - cover);
            let to = |c: f32| ((c * cover).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            out[py * size + pxi] = Color32::from_rgba_premultiplied(to(rgb[0]), to(rgb[1]), to(rgb[2]), (a * 255.0 + 0.5) as u8);
        }
    }
    ColorImage::new([size, size], out)
}
