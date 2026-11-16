//! The mini player: a floating capsule, always on top, like a desktop widget.
//! A spinning record with a progress ring, the song, its waveform, and the
//! transport. Drag it anywhere; double-click (or the corner button) to expand.

use std::sync::atomic::Ordering;

use eframe::egui::{self, pos2, vec2, Color32, CursorIcon, Id, Rect, Sense, Shape, Stroke, ViewportCommand};

use crate::app::{icon_button, App};
use crate::engine::Cmd;
use crate::paint::{self, Icon, Vinyl, Wave};
use crate::stage::ease_out;

pub const MINI: egui::Vec2 = egui::Vec2::new(470.0, 124.0);

impl App {
    pub fn mini_view(&mut self, ui: &mut egui::Ui, rect: Rect, time: f32) {
        let ctx = ui.ctx().clone();
        let h = rect.height();
        let radius = h * 0.5;

        // Drag anywhere to move; registered first so the controls win.
        let bg = ui.interact(rect, Id::unique("wax-mini-bg"), Sense::click_and_drag());
        if bg.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
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

        // The record.
        let dc = pos2(rect.left() + radius, rect.center().y);
        let dr = radius - 12.0;
        let pos_ms = self.engine.shared.pos_ms.load(Ordering::Relaxed);
        let listed = self.now.as_ref().map_or(0, |n| n.track.duration_ms as u64);
        let duration = if listed > 0 { listed } else { self.measured_ms as u64 };
        let progress = if duration > 0 { (pos_ms as f32 / duration as f32).min(1.0) } else { 0.0 };
        let label = self.now.as_ref().and_then(|n| {
            let key = n.track.album_key();
            self.art.large(key).or_else(|| self.art.thumb(key, &n.track.path).0)
        });
        paint::vinyl(&p, &Vinyl { center: dc, radius: dr, angle: self.angle, label, palette: pal, gaps: &[], progress: None });
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

        // Waveform along the bottom; click to seek.
        let wave = Rect::from_min_max(pos2(x0, rect.bottom() - 46.0), pos2(rect.right() - 58.0, rect.bottom() - 20.0));
        let resp = ui.interact(wave.expand2(vec2(4.0, 6.0)), id("wave"), Sense::click());
        let hover = resp.hover_pos().map(|pos| ((pos.x - wave.left()) / wave.width()).clamp(0.0, 1.0));
        if resp.clicked() && duration > 0 {
            if let Some(f) = hover {
                let ms = (f * duration as f32) as u64;
                self.engine.shared.pos_ms.store(ms, Ordering::Relaxed);
                self.engine.send(Cmd::Seek(ms));
            }
        }
        if hover.is_some() {
            ctx.set_cursor_icon(CursorIcon::PointingHand);
        }
        paint::waveform(
            &p,
            wave,
            &Wave {
                peaks: self.peaks.as_ref().map(|(_, pk)| &pk[..]),
                progress,
                hover,
                accent: Color32::WHITE,
                level: if self.playing() { self.engine.shared.level() } else { 0.0 },
                time: self.flow * 4.0,
                appear: ease_out(((time - self.peaks_at) / 0.7).clamp(0.0, 1.0)),
            },
        );

        // Expand, bottom right; brighter on hover.
        let ec = pos2(rect.right() - 34.0, wave.center().y);
        let col = paint::alpha(Color32::WHITE, 0.55 + 0.45 * hv);
        if icon_button(ui, id("expand"), ec, 30.0, Icon::Expand, 11.0, col).on_hover_text("Back to the full player").clicked() {
            self.set_mini(&ctx, false);
        }
    }
}
