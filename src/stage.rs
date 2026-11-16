//! The stage card (the record on the platter and what's playing) and the
//! floating dock along the bottom (transport, waveform, save, volume).

use std::sync::atomic::Ordering;

use eframe::egui::{self, pos2, vec2, Align2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Shape, Stroke};

use crate::app::{icon_button, pill, App, SaveState};
use crate::engine::{Cmd, REPEAT_ALL, REPEAT_OFF, REPEAT_ONE};
use crate::library::fmt_time;
use crate::paint::{self, Icon, Vinyl, Wave};

pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

pub const CARD_R: f32 = 18.0;

impl App {
    pub fn stage(&mut self, ui: &mut egui::Ui, rect: Rect, _time: f32) {
        let ctx = ui.ctx().clone();
        let p = ui.painter_at(rect.expand(30.0));
        let pal = self.palette();
        let level = self.engine.shared.level();
        paint::glass(&p, rect, CARD_R, Color32::TRANSPARENT);
        paint::bands(&p, rect, CARD_R, self.flow, pal, level);
        let p = ui.painter_at(rect);

        // Sleeve on the left, record sliding out to the right; the whole
        // composition is centred in the card.
        let s = (rect.height() * 0.46).clamp(170.0, 420.0).min(rect.width() * 0.4);
        let r = s * 0.48;
        let comp_w = s * 1.54;
        let text_h = 190.0;
        let left = rect.center().x - comp_w * 0.5;
        let top = rect.top() + ((rect.height() - s - text_h) * 0.45).max(28.0);
        let sleeve = Rect::from_min_size(pos2(left, top), vec2(s, s));
        let slide = ease_out(self.slide);
        let disc = pos2(sleeve.center().x + s * 0.56 * slide, sleeve.center().y);

        let pos_ms = self.engine.shared.pos_ms.load(Ordering::Relaxed);
        let (label, cover, gaps, progress, title) = match &self.now {
            Some(n) => {
                let key = n.track.album_key();
                let large = self.art.large(key);
                let thumb = self.art.thumb(key, &n.track.path).0;
                let progress = (n.offset_ms + pos_ms) as f32 / n.album_ms.max(1) as f32;
                let title = if n.track.album.is_empty() { &*n.track.title } else { &*n.track.album };
                (large.or(thumb), large.or(thumb), n.gaps.as_slice(), Some(progress), title)
            }
            None => (None, None, &[][..], None, ""),
        };
        paint::vinyl(&p, &Vinyl { center: disc, radius: r, angle: self.angle, label, palette: pal, gaps, progress });
        paint::sleeve(&p, sleeve, cover, pal, title, 6.0);

        // Scroll over the stage to change volume.
        let hover = ui.interact(rect, Id::unique("wax-stage"), Sense::hover());
        if hover.hovered() {
            let dy = ctx.input(|i| i.smooth_scroll_delta.y);
            if dy != 0.0 {
                self.set_volume(self.cfg.volume + dy * 0.0012);
            }
        }

        // Typography.
        let text_w = (rect.right() - left - 32.0).min(comp_w + 80.0);
        let mut y = sleeve.bottom() + (s * 0.11).max(18.0);
        let playing = self.playing();
        match &self.now {
            Some(n) => {
                let buffering = self.engine.shared.buffering.load(Ordering::Relaxed);
                let status = match (buffering, playing, n.track.stream_id().is_some()) {
                    (true, ..) => "FINDING THE GROOVE…",
                    (_, true, true) => "STREAMING",
                    (_, true, false) => "NOW SPINNING",
                    _ => "PAUSED",
                };
                paint::spaced(&p, pos2(left, y), Align2::LEFT_TOP, status, paint::semibold(10.5), Color32::from_white_alpha(170), 3.0);
                y += 22.0;

                let size = (s * 0.155).clamp(28.0, 54.0);
                let mut job = egui::text::LayoutJob::simple(n.track.title.to_string(), paint::display(size), Color32::WHITE, text_w);
                job.wrap.max_rows = 2;
                // Tighter leading for the big two-line titles.
                job.sections[0].format.line_height = Some(size * 1.02);
                let g = p.layout_job(job);
                let h = g.size().y;
                p.galley(pos2(left, y), g, Color32::WHITE);
                y += h + 12.0;

                squiggle(&p, pos2(left + 2.0, y + 5.0), self.flow * 6.0, self.spin, Color32::WHITE);
                y += 22.0;

                p.galley(pos2(left, y), paint::elided(&p, &n.track.artist, paint::semibold(18.0), Color32::WHITE, text_w), Color32::WHITE);
                y += 27.0;
                let mut sub = if n.track.stream_id().is_some() { "From YouTube".to_string() } else { n.track.album.to_string() };
                if n.track.year > 0 {
                    sub.push_str(&format!("  ·  {}", n.track.year));
                }
                p.galley(pos2(left, y), paint::elided(&p, &sub, paint::body(14.0), Color32::from_white_alpha(190), text_w), Color32::WHITE);
            }
            None => {
                let empty = self.lib.albums.is_empty();
                let (head, sub) = if self.scanning().is_some() && empty {
                    ("Reading your records…", "Tags are cached, so this only takes a while the first time.")
                } else if empty {
                    ("Drop your music here", "Drag a folder onto the window, or just start typing a song name.")
                } else {
                    ("Pick a record", "Choose something from the collection, or just start typing to search.")
                };
                let g = p.layout(head.to_owned(), paint::display((s * 0.15).clamp(28.0, 48.0)), Color32::WHITE, text_w);
                let h = g.size().y;
                p.galley(pos2(left, y), g, Color32::WHITE);
                y += h + 10.0;
                let g = p.layout(sub.to_owned(), paint::body(15.0), Color32::from_white_alpha(200), text_w.min(430.0));
                let h = g.size().y;
                p.galley(pos2(left, y), g, Color32::WHITE);
                y += h + 20.0;
                if empty && self.scanning().is_none() && pill(ui, Id::unique("wax-choose"), pos2(left, y), "Choose folder", Some(Color32::WHITE)).clicked() {
                    self.pick_folder(&ctx);
                }
            }
        }
    }

    /// The floating dock: transport on the left, the waveform across the
    /// middle, keep/add/volume on the right.
    pub fn dock(&mut self, ui: &mut egui::Ui, rect: Rect, time: f32) {
        let ctx = ui.ctx().clone();
        let p = ui.painter_at(rect.expand(30.0));
        paint::glass(&p, rect, 22.0, Color32::from_rgba_unmultiplied(22, 23, 30, 228));
        let p = ui.painter_at(rect);
        let accent = self.accent();
        let mid = rect.center().y;
        let playing = self.playing();

        // Transport.
        let soft = Color32::from_white_alpha(220);
        let off = Color32::from_white_alpha(105);
        let id = |s: &str| Id::unique(("wax-transport", s));
        let x0 = rect.left() + 22.0;

        let shuffle = self.cfg.shuffle;
        let c = pos2(x0 + 14.0, mid);
        if icon_button(ui, id("shuffle"), c, 34.0, Icon::Shuffle, 13.0, if shuffle { Color32::WHITE } else { off })
            .on_hover_text(if shuffle { "Shuffle is on" } else { "Shuffle is off" })
            .clicked()
        {
            self.set_shuffle(!shuffle);
        }
        if shuffle {
            p.circle_filled(c + vec2(0.0, 14.0), 1.6, accent);
        }
        if icon_button(ui, id("prev"), pos2(x0 + 56.0, mid), 38.0, Icon::Prev, 14.0, soft).clicked() {
            self.engine.send(Cmd::Prev);
        }
        let pc = pos2(x0 + 104.0, mid);
        let play = ui.interact(Rect::from_center_size(pc, vec2(48.0, 48.0)), id("play"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let h = ctx.animate_bool_with_time(id("play-h"), play.hovered(), 0.12);
        p.circle_filled(pc + vec2(0.0, 2.0), 22.0 + h * 1.5, Color32::from_black_alpha(60));
        p.circle_filled(pc, 21.0 + h * 1.5, Color32::WHITE);
        let buffering = self.engine.shared.buffering.load(Ordering::Relaxed);
        if buffering {
            // A small orbiting dot while a stream warms up.
            let a = time * 5.0;
            p.circle_filled(pc + vec2(a.cos(), a.sin()) * 8.0, 2.6, paint::INK);
            p.circle_stroke(pc, 8.0, Stroke::new(1.0, Color32::from_black_alpha(60)));
            ctx.request_repaint();
        } else {
            paint::icon(&p, pc, 13.0, if playing { Icon::Pause } else { Icon::Play }, paint::INK);
        }
        if play.clicked() {
            self.engine.send(Cmd::Toggle);
        }
        if icon_button(ui, id("next"), pos2(x0 + 152.0, mid), 38.0, Icon::Next, 14.0, soft).clicked() {
            self.engine.send(Cmd::Next);
        }
        let (icon, col, tip) = match self.cfg.repeat {
            REPEAT_ALL => (Icon::Repeat, Color32::WHITE, "Repeating the queue"),
            REPEAT_ONE => (Icon::RepeatOne, Color32::WHITE, "Repeating this track"),
            _ => (Icon::Repeat, off, "Repeat is off"),
        };
        let rc = pos2(x0 + 194.0, mid);
        if icon_button(ui, id("repeat"), rc, 34.0, icon, 13.0, col).on_hover_text(tip).clicked() {
            self.cfg.repeat = match self.cfg.repeat {
                REPEAT_OFF => REPEAT_ALL,
                REPEAT_ALL => REPEAT_ONE,
                _ => REPEAT_OFF,
            };
            self.engine.send(Cmd::Repeat(self.cfg.repeat));
            self.cfg.save();
        }
        if self.cfg.repeat != REPEAT_OFF {
            p.circle_filled(rc + vec2(0.0, 14.0), 1.6, accent);
        }

        // Right group: keep, add to playlist, volume.
        let vol_track = Rect::from_min_max(pos2(rect.right() - 112.0, mid - 1.5), pos2(rect.right() - 26.0, mid + 1.5));
        self.volume_slider(ui, &p, vol_track, accent);
        paint::icon(&p, pos2(vol_track.left() - 18.0, mid), 12.0, Icon::Volume, paint::DIM);
        let add_c = pos2(vol_track.left() - 54.0, mid);
        let save_c = pos2(add_c.x - 38.0, mid);
        let current = self.now.as_ref().map(|n| n.track.clone());
        if let Some(track) = &current {
            let add = icon_button(ui, id("add"), add_c, 34.0, Icon::AddList, 13.0, soft).on_hover_text("Add to a playlist");
            egui::Popup::menu(&add).show(|ui| self.playlist_menu(ui, vec![track.clone()], time));
            match self.save_state(track) {
                SaveState::Unsaved => {
                    if icon_button(ui, id("save"), save_c, 34.0, Icon::Save, 13.0, soft).on_hover_text("Save to your collection").clicked() {
                        self.save_track(&ctx, track.clone(), time);
                    }
                }
                SaveState::Saving => {
                    let a = time * 6.0;
                    p.circle_stroke(save_c, 7.0, Stroke::new(1.5, Color32::from_white_alpha(40)));
                    let arc: Vec<Pos2> = (0..=10).map(|i| save_c + vec2((a + i as f32 * 0.16).cos(), (a + i as f32 * 0.16).sin()) * 7.0).collect();
                    p.add(Shape::line(arc, Stroke::new(1.5, accent)));
                    ctx.request_repaint();
                }
                SaveState::Saved => {
                    paint::icon(&p, save_c, 12.0, Icon::Check, accent);
                }
                SaveState::Local => {}
            }
        }

        // Waveform between the groups, times either side.
        let wl = rect.left() + 262.0;
        let wr = save_c.x - 70.0;
        let wave = Rect::from_min_max(pos2(wl + 46.0, rect.top() + 16.0), pos2(wr - 46.0, rect.bottom() - 16.0));
        if wave.width() < 60.0 {
            return;
        }
        let listed = self.now.as_ref().map_or(0, |n| n.track.duration_ms as u64);
        let duration = if listed > 0 { listed } else { self.measured_ms as u64 };
        let raw_pos = self.engine.shared.pos_ms.load(Ordering::Relaxed);
        let pos_ms = if duration > 0 { raw_pos.min(duration) } else { raw_pos };

        // Click or drag anywhere on the wave to seek.
        let resp = ui.interact(wave.expand2(vec2(6.0, 10.0)), Id::unique("wax-wave"), Sense::click_and_drag());
        let frac_at = |pos: Pos2| ((pos.x - wave.left()) / wave.width()).clamp(0.0, 1.0);
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        let mut hover = None;
        if duration > 0 {
            if let Some(pos) = pointer {
                hover = Some(frac_at(pos));
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
            if resp.dragged() {
                self.scrub = hover;
            }
            if resp.clicked() || resp.drag_stopped() {
                if let Some(f) = self.scrub.take().or(hover) {
                    let ms = (f * duration as f32) as u64;
                    // Show the new position right away, before the engine confirms it.
                    self.engine.shared.pos_ms.store(ms, Ordering::Relaxed);
                    self.engine.send(Cmd::Seek(ms));
                }
            }
        }
        let progress = self.scrub.unwrap_or(if duration > 0 { pos_ms as f32 / duration as f32 } else { 0.0 });
        let shown_ms = self.scrub.map_or(pos_ms, |f| (f * duration as f32) as u64);

        paint::waveform(
            &p,
            wave,
            &Wave {
                peaks: self.peaks.as_ref().map(|(_, pk)| &pk[..]),
                progress,
                hover: if self.scrub.is_some() { None } else { hover },
                accent,
                level: if playing { self.engine.shared.level() } else { 0.0 },
                time: self.flow * 4.0,
                appear: ease_out(((time - self.peaks_at) / 0.7).clamp(0.0, 1.0)),
            },
        );
        p.text(pos2(wave.left() - 12.0, mid), Align2::RIGHT_CENTER, fmt_time(shown_ms), paint::semibold(12.0), paint::TEXT);
        p.text(pos2(wave.right() + 12.0, mid), Align2::LEFT_CENTER, fmt_time(duration), paint::body(12.0), paint::DIM);
        if let (Some(f), false) = (hover, self.scrub.is_some()) {
            let x = wave.left() + wave.width() * f;
            let tag = fmt_time((f * duration as f32) as u64);
            let tp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("wax-wave-tip")));
            let g = tp.layout_no_wrap(tag, paint::semibold(11.5), Color32::WHITE);
            let r = Rect::from_center_size(pos2(x, rect.top() - 14.0), g.size() + vec2(14.0, 8.0));
            tp.rect_filled(r, 8.0, Color32::from_rgba_unmultiplied(30, 32, 40, 235));
            tp.galley(r.min + vec2(7.0, 4.0), g, Color32::WHITE);
        }
    }

    pub fn volume_slider(&mut self, ui: &egui::Ui, p: &egui::Painter, track: Rect, accent: Color32) {
        let ctx = ui.ctx();
        let vol = ui
            .interact(track.expand2(vec2(8.0, 14.0)), Id::unique("wax-volume"), Sense::click_and_drag())
            .on_hover_cursor(CursorIcon::PointingHand);
        if let Some(pos) = vol.interact_pointer_pos() {
            self.set_volume((pos.x - track.left()) / track.width());
        }
        if vol.drag_stopped() || vol.clicked() {
            self.cfg.save();
        }
        let h = ctx.animate_bool_with_time(vol.id.with("h"), vol.hovered() || vol.dragged(), 0.12);
        let x = track.left() + track.width() * self.cfg.volume;
        let mid = track.center().y;
        p.rect_filled(track, 2.0, Color32::from_white_alpha(40));
        p.rect_filled(Rect::from_min_max(track.min, pos2(x, track.bottom())), 2.0, paint::lerp_color(paint::TEXT, accent, h));
        p.circle_filled(pos2(x, mid), 3.0 + 3.0 * h, Color32::WHITE);
    }
}

/// The little hand-drawn wiggle under the title; it only moves with the music.
fn squiggle(p: &egui::Painter, start: Pos2, phase: f32, spin: f32, color: Color32) {
    let amp = 2.0 + 3.0 * (spin / 3.49).clamp(0.0, 1.0);
    let pts: Vec<Pos2> = (0..=28)
        .map(|i| {
            let f = i as f32 / 28.0;
            // Flat at both ends so it reads as a pen stroke.
            let env = (f * std::f32::consts::PI).sin();
            start + vec2(f * 54.0, (f * 15.0 - phase).sin() * amp * env)
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(2.0, color)));
}
