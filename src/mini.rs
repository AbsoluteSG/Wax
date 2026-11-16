//! The mini player: a floating capsule, always on top, like a desktop widget.
//! A spinning record with a progress ring, the song, its waveform, and the
//! transport. Drag it anywhere; double-click (or the corner button) to expand.

use std::sync::atomic::Ordering;

use eframe::egui::{self, pos2, vec2, Color32, CursorIcon, Id, Mesh, Rect, Sense, Shape, Stroke};

use crate::app::{icon_button, App};
use crate::engine::Cmd;
use crate::paint::{self, Icon, Vinyl};

pub const MINI: egui::Vec2 = egui::Vec2::new(470.0, 124.0);

impl App {
    pub fn mini_view(&mut self, ui: &mut egui::Ui, rect: Rect, time: f32) {
        let ctx = ui.ctx().clone();
        let h = rect.height();
        let radius = h * 0.5;

        // Drag anywhere to move; registered first so the controls win.
        let bg = ui.interact(rect, Id::unique("wax-mini-bg"), Sense::click_and_drag());
        if bg.drag_started() {
            self.motion.begin_drag();
        }
        if bg.double_clicked() {
            self.set_mini(&ctx, false);
            return;
        }
        let hovered = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
        let hv = ctx.animate_bool_with_time(Id::unique("wax-mini-hover"), hovered, 0.2);

        let p = ui.painter_at(rect);
        let pal = self.palette();
        paint::bands(&p, rect, radius, self.flow, pal, self.engine.shared.level());
        p.rect_filled(rect, radius, Color32::from_black_alpha(120));
        p.rect_stroke(rect.shrink(0.5), radius, Stroke::new(1.0, Color32::from_white_alpha(28)), egui::StrokeKind::Inside);

        // The record, floating very slightly.
        let bob = vec2(0.0, (self.bob_t * 1.2).sin() * 1.2);
        let dc = pos2(rect.left() + radius, rect.center().y) + bob;
        let dr = radius - 12.0;
        let pos_ms = self.engine.shared.pos_ms.load(Ordering::Relaxed);
        let listed = self.now.as_ref().map_or(0, |n| n.track.duration_ms as u64);
        let duration = if listed > 0 { listed } else { self.measured_ms as u64 };
        let progress = if duration > 0 { (pos_ms as f32 / duration as f32).min(1.0) } else { 0.0 };
        let label = self.now.as_ref().and_then(|n| {
            let key = n.track.album_key();
            self.art.large(key).or_else(|| self.art.thumb(key, &n.track.path).0)
        });
        let disc_tex = self.disc.get(&ctx, dr * ctx.pixels_per_point(), &[]);
        paint::vinyl(&p, &Vinyl { center: dc, radius: dr, angle: self.angle, label, palette: pal, gaps: &[], progress: None, disc: disc_tex });
        // Progress ring.
        let ring = dr + 5.0;
        p.circle_stroke(dc, ring, Stroke::new(2.5, Color32::from_white_alpha(40)));
        if progress > 0.0 {
            let n = (64.0 * progress).ceil().max(2.0) as usize;
            let a0 = -std::f32::consts::FRAC_PI_2;
            let pts: Vec<_> = (0..=n).map(|i| dc + egui::Vec2::angled(a0 + std::f32::consts::TAU * progress * i as f32 / n as f32) * ring).collect();
            p.add(Shape::line(pts, Stroke::new(2.5, Color32::WHITE)));
        }

        // Song.
        let x0 = rect.left() + h + 6.0;
        let ctrl_w = 128.0;
        let text_w = rect.right() - ctrl_w - x0 - 8.0;
        match &self.now {
            Some(n) => {
                p.galley(pos2(x0, rect.top() + 20.0), paint::elided(&p, &n.track.title, paint::semibold(15.5), Color32::WHITE, text_w), Color32::WHITE);
                p.galley(pos2(x0, rect.top() + 42.0), paint::elided(&p, &n.track.artist, paint::body(12.5), Color32::from_white_alpha(200), text_w), Color32::WHITE);
            }
            None => {
                p.galley(pos2(x0, rect.top() + 30.0), paint::elided(&p, "Nothing playing", paint::semibold(15.0), Color32::WHITE, text_w), Color32::WHITE);
            }
        }

        // Transport, top right.
        let cy = rect.top() + 38.0;
        let soft = Color32::from_white_alpha(225);
        let id = |s: &str| Id::unique(("wax-mini", s));
        let bx = rect.right() - 30.0;
        if icon_button(ui, id("next"), pos2(bx - 2.0, cy), 32.0, Icon::Next, 12.0, soft).clicked() {
            self.engine.send(Cmd::Next);
        }
        let pc = pos2(bx - 42.0, cy);
        let play = ui.interact(Rect::from_center_size(pc, vec2(38.0, 38.0)), id("play"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let ph = ctx.animate_bool_with_time(id("ph"), play.hovered(), 0.1);
        p.circle_filled(pc, 16.0 + ph, Color32::WHITE);
        if self.engine.shared.buffering.load(Ordering::Relaxed) {
            let a = time * 5.0;
            p.circle_filled(pc + egui::Vec2::angled(a) * 6.0, 2.2, paint::INK);
            ctx.request_repaint();
        } else {
            paint::icon(&p, pc, 10.0, if self.playing() { Icon::Pause } else { Icon::Play }, paint::INK);
        }
        if play.clicked() {
            self.engine.send(Cmd::Toggle);
        }
        if icon_button(ui, id("prev"), pos2(bx - 82.0, cy), 32.0, Icon::Prev, 12.0, soft).clicked() {
            self.engine.send(Cmd::Prev);
        }

        // Waveform along the bottom, scrolling past a fixed playhead.
        let wave = Rect::from_min_max(pos2(x0, rect.bottom() - 48.0), pos2(rect.right() - 58.0, rect.bottom() - 18.0));
        let playing = self.playing();
        self.spectrum.update(&self.engine.shared.scope, ctx.input(|i| i.stable_dt.min(0.05)), playing);
        let resp = ui.interact(wave.expand2(vec2(4.0, 6.0)), id("wave"), Sense::click());
        let head_x = wave.left() + wave.width() * 0.42;
        let now_s = self.smooth_ms as f32 / 1000.0;
        if resp.clicked() && duration > 0 {
            if let Some(pos) = resp.interact_pointer_pos() {
                let t = (now_s + (pos.x - head_x) / (PITCH * BPS)).clamp(0.0, duration as f32 / 1000.0);
                let ms = (t * 1000.0) as u64;
                self.engine.shared.pos_ms.store(ms, Ordering::Relaxed);
                self.smooth_ms = ms as f64;
                self.engine.send(Cmd::Seek(ms));
            }
        }
        if resp.hovered() {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
        }
        self.live_wave(&p, wave, head_x, now_s, duration as f32 / 1000.0, playing);

        // Expand, bottom right; brighter on hover.
        let ec = pos2(rect.right() - 34.0, wave.center().y);
        let col = paint::alpha(Color32::WHITE, 0.55 + 0.45 * hv);
        if icon_button(ui, id("expand"), ec, 30.0, Icon::Expand, 11.0, col).on_hover_text("Back to the full player").clicked() {
            self.set_mini(&ctx, false);
        }
    }
}

/// Scrolling speed of the mini waveform: bars per second and px per bar.
const BPS: f32 = 6.0;
const PITCH: f32 = 4.0;

impl App {
    /// Bars scroll right-to-left past the playhead. Behind it: the loudness
    /// actually heard, recorded as it played. Ahead: the track's overview,
    /// dimmed. Around it: bars that move with the live spectrum.
    fn live_wave(&mut self, p: &egui::Painter, rect: Rect, head_x: f32, now_s: f32, dur_s: f32, playing: bool) {
        let key = self.now.as_ref().map_or(0, |n| n.track.key());
        if self.live.0 != key {
            self.live = (key, Vec::new());
        }
        let slot = (now_s * BPS) as usize;
        if playing && dur_s > 0.0 {
            if self.live.1.len() <= slot {
                self.live.1.resize(slot + 1, -1.0);
            }
            // Loudness actually heard, with headroom so quiet and loud passages differ.
            let v = (0.08 + 1.5 * self.spectrum.energy).min(1.0);
            let cell = &mut self.live.1[slot];
            *cell = cell.max(v);
        }

        let mid = rect.center().y;
        let half = rect.height() * 0.5;
        let peaks = self.peaks.as_ref().map(|(_, pk)| pk.clone());
        let bands = self.spectrum.bands;
        let pos = now_s * BPS;
        let first = (pos - (head_x - rect.left()) / PITCH).floor() as i64 - 1;
        let last = (pos + (rect.right() - head_x) / PITCH).ceil() as i64 + 1;
        let mut mesh = Mesh::default();
        let mut bar = |x: f32, h: f32, col: Color32| {
            if x < rect.left() - 1.0 || x > rect.right() + 1.0 {
                return;
            }
            let tip = paint::alpha(col, 0.25);
            let k = mesh.vertices.len() as u32;
            let hw = 1.1;
            for (y, c) in [(mid - h, tip), (mid, col), (mid + h, tip)] {
                mesh.colored_vertex(pos2(x - hw, y), c);
                mesh.colored_vertex(pos2(x + hw, y), c);
            }
            for q in [0, 2] {
                mesh.add_triangle(k + q, k + q + 1, k + q + 2);
                mesh.add_triangle(k + q + 1, k + q + 2, k + q + 3);
            }
        };
        for i in first..=last {
            let x = head_x + (i as f32 - pos) * PITCH;
            if i < 0 || (dur_s > 0.0 && i as f32 / BPS > dur_s) {
                bar(x, 0.8, Color32::from_white_alpha(40));
                continue;
            }
            let overview = peaks.as_ref().map_or(0.25, |pk| {
                let f = (i as f32 / BPS / dur_s.max(0.001)).clamp(0.0, 1.0);
                pk[((f * (pk.len() - 1) as f32) as usize).min(pk.len() - 1)] as f32 / 255.0
            });
            let heard = self.live.1.get(i as usize).copied().filter(|v| *v >= 0.0);
            let past = x <= head_x;
            let mut amp = match (past, heard) {
                (true, Some(v)) => v * 0.75 + overview * 0.25,
                _ => overview,
            };
            // Bars near the playhead dance: low notes at the centre, highs further out.
            let d = (x - head_x).abs();
            if playing && d < 34.0 {
                let band = ((d / 34.0) * (bands.len() - 1) as f32) as usize;
                let fall = 1.0 - d / 40.0;
                amp *= 0.85 + (bands[band] * 0.95 + self.spectrum.energy * 0.35) * fall;
            }
            let h = (amp.clamp(0.04, 1.0) * half).max(1.0);
            let col = if past { Color32::from_white_alpha(235) } else { Color32::from_white_alpha(80) };
            bar(x, h, col);
        }
        p.add(Shape::mesh(mesh));
        // Playhead with a soft glow that breathes with the music.
        let glow = 0.12 + 0.3 * self.spectrum.energy;
        p.rect_filled(Rect::from_center_size(pos2(head_x, mid), vec2(18.0, rect.height())), 9.0, Color32::from_white_alpha((glow * 60.0) as u8));
        p.line_segment([pos2(head_x, rect.top() - 2.0), pos2(head_x, rect.bottom() + 2.0)], Stroke::new(1.5, Color32::WHITE));
    }
}
