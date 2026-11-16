//! The shelf card: Records and Playlists, the inside of one of either, and
//! search results. Everything here sits on frosted glass.

use std::sync::Arc;

use eframe::egui::{
    self, epaint::RectShape, pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, Id, Rect, ScrollArea, Sense, Stroke,
    TextureId, UiBuilder,
};

use crate::app::{icon_button, pill, search_id, App, Hit, Tab};
use crate::art::Palette;
use crate::library::{fmt_time, Album};
use crate::paint::{self, Icon};
use crate::stage::CARD_R;

const PAD: f32 = 22.0;

impl App {
    pub fn shelf(&mut self, ui: &mut egui::Ui, rect: Rect, time: f32) {
        let ctx = ui.ctx().clone();
        let p = ui.painter_at(rect.expand(30.0));
        paint::glass(&p, rect, CARD_R, Color32::from_rgba_unmultiplied(20, 21, 28, 205));
        let p = ui.painter_at(rect);
        let left = rect.left() + PAD;
        let top = rect.top() + 16.0;

        // Records | Playlists
        let seg = Rect::from_min_size(pos2(left, top), vec2(212.0, 32.0));
        p.rect_filled(seg, 16.0, Color32::from_white_alpha(14));
        let anim = ctx.animate_value_with_time(Id::unique("wax-tab"), if self.tab == Tab::Records { 0.0 } else { 1.0 }, 0.22);
        let knob = Rect::from_min_size(seg.min + vec2(3.0 + anim * 103.0, 3.0), vec2(103.0, 26.0));
        p.rect_filled(knob, 13.0, Color32::from_white_alpha(38));
        for (i, (tab, label)) in [(Tab::Records, "Records"), (Tab::Playlists, "Playlists")].into_iter().enumerate() {
            let r = Rect::from_min_size(seg.min + vec2(3.0 + i as f32 * 103.0, 3.0), vec2(103.0, 26.0));
            let resp = ui.interact(r, Id::unique(("wax-tab", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            let on = self.tab == tab;
            let col = if on { paint::TEXT } else if resp.hovered() { paint::lerp_color(paint::DIM, paint::TEXT, 0.5) } else { paint::DIM };
            p.text(r.center(), Align2::CENTER_CENTER, label, paint::semibold(12.5), col);
            if resp.clicked() {
                if on {
                    // Clicking the open tab goes back to its top level.
                    self.view = None;
                    self.open_list = None;
                }
                self.tab = tab;
                self.search.clear();
            }
        }

        // Header actions.
        let ay = seg.center().y;
        match self.tab {
            Tab::Records => {
                if icon_button(ui, Id::unique("wax-add"), pos2(rect.right() - PAD - 10.0, ay), 32.0, Icon::Plus, 11.0, paint::DIM)
                    .on_hover_text("Add a music folder")
                    .clicked()
                {
                    self.pick_folder(&ctx);
                }
                if icon_button(ui, Id::unique("wax-rescan"), pos2(rect.right() - PAD - 46.0, ay), 32.0, Icon::Refresh, 12.0, paint::DIM)
                    .on_hover_text("Look for new music")
                    .clicked()
                {
                    self.rescan(&ctx);
                }
            }
            Tab::Playlists => {
                if icon_button(ui, Id::unique("wax-newlist"), pos2(rect.right() - PAD - 10.0, ay), 32.0, Icon::Plus, 11.0, paint::DIM)
                    .on_hover_text("New playlist")
                    .clicked()
                {
                    self.new_playlist(Vec::new(), time);
                }
            }
        }

        // Search field: rounded, with a magnifier.
        let field = Rect::from_min_size(pos2(left, seg.bottom() + 12.0), vec2(rect.width() - PAD * 2.0, 34.0));
        let focused = ctx.memory(|m| m.has_focus(search_id()));
        let focus = ctx.animate_bool_with_time(Id::unique("wax-search-focus"), focused, 0.15);
        p.rect_filled(field, 11.0, Color32::from_white_alpha((12.0 + 10.0 * focus) as u8));
        p.rect_stroke(field, 11.0, Stroke::new(1.0, paint::alpha(self.accent(), focus * 0.8)), egui::StrokeKind::Inside);
        let mc = pos2(field.left() + 16.0, field.center().y);
        p.circle_stroke(mc - vec2(1.0, 1.0), 4.5, Stroke::new(1.4, paint::DIM));
        p.line_segment([mc + vec2(2.3, 2.3), mc + vec2(5.5, 5.5)], Stroke::new(1.4, paint::DIM));
        let edit_rect = Rect::from_min_max(pos2(field.left() + 30.0, field.top() + 7.0), pos2(field.right() - 10.0, field.bottom() - 5.0));
        let edit = egui::TextEdit::singleline(&mut self.search)
            .id(search_id())
            .frame(egui::Frame::NONE)
            .font(paint::body(14.5))
            .hint_text(egui::RichText::new("Search your records, or any song").color(paint::FAINT))
            .desired_width(edit_rect.width());
        ui.put(edit_rect, edit);

        let area = Rect::from_min_max(pos2(left, field.bottom() + 14.0), pos2(rect.right() - 6.0, rect.bottom() - 6.0));
        if !self.search.trim().is_empty() {
            self.results_view(ui, area, time);
            return;
        }
        match self.tab {
            Tab::Records => match self.view.clone() {
                Some(album) => self.album_view(ui, area, &album, time),
                None => self.grid_view(ui, area, time),
            },
            Tab::Playlists => match self.open_list {
                Some(id) if self.playlists.get(id).is_some() => self.playlist_view(ui, area, id, time),
                _ => self.playlists_view(ui, area, time),
            },
        }
    }

    // ------------------------------------------------------------ records

    fn grid_view(&mut self, ui: &mut egui::Ui, area: Rect, time: f32) {
        let p = ui.painter_at(area);
        let count = match self.scanning() {
            Some(n) => format!("Reading tags…  {n} files"),
            None => format!("{} records  ·  {} tracks", self.lib.albums.len(), self.lib.track_count),
        };
        p.text(area.left_top() + vec2(2.0, 4.0), Align2::LEFT_TOP, count, paint::body(12.0), paint::DIM);
        let area = Rect::from_min_max(area.min + vec2(0.0, 28.0), area.max);
        if self.filtered.is_empty() {
            let msg = if self.lib.albums.is_empty() { "Nothing on the shelf yet." } else { "No records match that." };
            p.text(area.left_top() + vec2(2.0, 10.0), Align2::LEFT_TOP, msg, paint::body(14.0), paint::DIM);
            return;
        }

        // Sleeves with the record peeking out; gaps leave room for the disc.
        let gap = 30.0;
        let width = area.width() - 20.0;
        let cols = ((width + gap) / (118.0 + gap)).floor().max(2.0) as usize;
        let size = ((width + gap) / cols as f32 - gap).floor();
        let row_h = size + 62.0;
        let rows = self.filtered.len().div_ceil(cols);

        ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
            ScrollArea::vertical().auto_shrink(false).show_rows(ui, row_h, rows, |ui, range| {
                for row in range {
                    let (row_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                    for col in 0..cols {
                        let Some(&idx) = self.filtered.get(row * cols + col) else { break };
                        let album = self.lib.albums[idx].clone();
                        let min = row_rect.min + vec2(col as f32 * (size + gap), 6.0);
                        self.tile(ui, min, size, &album, time);
                    }
                }
            });
        });
    }

    fn tile(&mut self, ui: &mut egui::Ui, min: egui::Pos2, size: f32, album: &Arc<Album>, time: f32) {
        let ctx = ui.ctx().clone();
        let cover = Rect::from_min_size(min, vec2(size, size));
        let hit = Rect::from_min_size(min, vec2(size, size + 48.0));
        if !ui.is_rect_visible(hit.expand(size * 0.4)) {
            return;
        }
        let id = Id::unique(("wax-tile", album.key));
        let resp = ui.interact(hit, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let is_now = self.now.as_ref().is_some_and(|n| n.track.album_key() == album.key);
        let h = ctx.animate_bool_with_time(id.with("h"), resp.contains_pointer(), 0.2);
        let out = ctx.animate_bool_with_time(id.with("now"), is_now, 0.4);
        let (tex, pal) = self.art.thumb(album.key, &album.tracks[0].path);

        let p = ui.painter();
        let r = size * 0.46;
        let peek = size * (0.13 + 0.18 * h + 0.14 * out);
        paint::vinyl_mini(p, pos2(cover.right() - r + peek, cover.center().y), r, pal);
        let cover = cover.translate(vec2(0.0, -3.0 * h));
        paint::sleeve(p, cover, tex, pal, &album.title, 6.0);

        if is_now {
            let c = cover.left_bottom() + vec2(20.0, -20.0);
            p.circle_filled(c, 13.0, Color32::from_black_alpha(190));
            paint::eq_bars(p, c + vec2(0.0, -2.0), Color32::WHITE, time, self.playing());
        }

        let title_col = if is_now { self.accent() } else { paint::TEXT };
        p.galley(pos2(min.x, min.y + size + 9.0), paint::elided(p, &album.title, paint::semibold(13.0), title_col, size), title_col);
        p.galley(pos2(min.x, min.y + size + 27.0), paint::elided(p, &album.artist, paint::body(12.0), paint::DIM, size), paint::DIM);

        // Play button fades in on hover; it sits on top of the tile's own click.
        let mut played = false;
        if h > 0.01 {
            let c = cover.right_bottom() - vec2(24.0, 24.0);
            let play = ui.interact(Rect::from_center_size(c, vec2(38.0, 38.0)), id.with("play"), Sense::click());
            let ph = ctx.animate_bool_with_time(id.with("ph"), play.hovered(), 0.1);
            let p = ui.painter();
            p.circle_filled(c + vec2(0.0, 2.0), 18.0, Color32::from_black_alpha((90.0 * h) as u8));
            p.circle_filled(c, 17.0 + 2.0 * ph, paint::alpha(Color32::WHITE, h));
            paint::icon(p, c, 9.5, Icon::Play, paint::alpha(paint::INK, h));
            if play.clicked() {
                played = true;
            }
        }
        if played || resp.double_clicked() {
            self.play_album(album, 0);
        } else if resp.clicked() {
            self.view = Some(album.clone());
        }
        resp.context_menu(|ui| self.track_menu(ui, album.tracks.clone(), "record", time));
    }

    fn album_view(&mut self, ui: &mut egui::Ui, area: Rect, album: &Arc<Album>, time: f32) {
        if self.back_link(ui, area, "Records") {
            self.view = None;
            return;
        }
        let (tex, pal) = self.art.thumb(album.key, &album.tracks[0].path);
        let mut meta = String::new();
        if album.year > 0 {
            meta.push_str(&format!("{}  ·  ", album.year));
        }
        let n = album.tracks.len();
        meta.push_str(&format!("{n} track{}  ·  {} min", if n == 1 { "" } else { "s" }, (album.duration_ms / 60_000).max(1)));
        let header = Header { covers: vec![(tex, pal)], title: &album.title, sub: &album.artist, meta: &meta, key: album.key };
        let (list_top, action) = self.header(ui, area, &header, None);
        match action {
            Some(HeaderAction::Play) => {
                self.set_shuffle(false);
                self.play_album(album, 0);
            }
            Some(HeaderAction::Shuffle) => {
                self.set_shuffle(true);
                self.play_album(album, (time * 1000.0) as usize % n.max(1));
            }
            _ => {}
        }

        let list = Rect::from_min_max(pos2(area.left() - 10.0, list_top), area.max);
        let tracks = album.tracks.clone();
        let rows: Vec<Row> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| Row {
                number: if t.track_no > 0 { t.track_no as usize } else { i + 1 },
                title: t.title.to_string(),
                sub: (t.artist != album.artist).then(|| t.artist.to_string()),
                duration_ms: t.duration_ms,
                path: t.path.clone(),
                art: None,
            })
            .collect();
        let out = self.track_rows(ui, list, &rows, None, time);
        if let Some(i) = out.play {
            self.play_album(album, i);
        }
        if let Some(i) = out.menu_for {
            self.pending_menu = Some((vec![tracks[i].clone()], None));
        }
    }

    // ---------------------------------------------------------- playlists

    fn playlists_view(&mut self, ui: &mut egui::Ui, area: Rect, time: f32) {
        let p = ui.painter_at(area);
        let n = self.playlists.lists.len();
        let label = if n == 0 { "No playlists yet".to_string() } else { format!("{n} playlist{}", if n == 1 { "" } else { "s" }) };
        p.text(area.left_top() + vec2(2.0, 4.0), Align2::LEFT_TOP, label, paint::body(12.0), paint::DIM);
        let area = Rect::from_min_max(area.min + vec2(0.0, 28.0), area.max);

        let gap = 22.0;
        let width = area.width() - 20.0;
        let cols = ((width + gap) / (130.0 + gap)).floor().max(2.0) as usize;
        let size = ((width + gap) / cols as f32 - gap).floor();
        let row_h = size + 60.0;
        let total = n + 1; // plus the "New playlist" card
        let rows = total.div_ceil(cols);
        let mut open = None;
        let mut create = false;
        let mut play = None;

        ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
            ScrollArea::vertical().auto_shrink(false).show_rows(ui, row_h, rows, |ui, range| {
                for row in range {
                    let (row_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::hover());
                    for col in 0..cols {
                        let i = row * cols + col;
                        if i >= total {
                            break;
                        }
                        let min = row_rect.min + vec2(col as f32 * (size + gap), 6.0);
                        let card = Rect::from_min_size(min, vec2(size, size));
                        let hit = Rect::from_min_size(min, vec2(size, size + 46.0));
                        if i == n {
                            // New playlist.
                            let resp = ui.interact(hit, Id::unique("wax-newcard"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                            let h = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), 0.15);
                            let p = ui.painter();
                            p.rect_filled(card, 10.0, Color32::from_white_alpha((8.0 + 10.0 * h) as u8));
                            dashed(p, card.shrink(1.0), 10.0, Color32::from_white_alpha((40.0 + 40.0 * h) as u8));
                            paint::icon(p, card.center(), 16.0, Icon::Plus, paint::lerp_color(paint::DIM, paint::TEXT, h));
                            p.text(pos2(min.x, card.bottom() + 9.0), Align2::LEFT_TOP, "New playlist", paint::semibold(13.0), paint::DIM);
                            if resp.clicked() {
                                create = true;
                            }
                            continue;
                        }
                        let list = &self.playlists.lists[i];
                        let id = list.id;
                        let covers = self.covers_of(id, 4);
                        let resp = ui.interact(hit, Id::unique(("wax-listcard", id)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                        let h = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.contains_pointer(), 0.2);
                        let list = &self.playlists.lists[i];
                        let p = ui.painter();
                        let card = card.translate(vec2(0.0, -3.0 * h));
                        mosaic(p, card, &covers, &list.name, 10.0);
                        p.galley(pos2(min.x, min.y + size + 9.0), paint::elided(p, &list.name, paint::semibold(13.0), paint::TEXT, size), paint::TEXT);
                        let k = list.entries.len();
                        let sub = format!("{k} song{}", if k == 1 { "" } else { "s" });
                        p.text(pos2(min.x, min.y + size + 27.0), Align2::LEFT_TOP, sub, paint::body(12.0), paint::DIM);
                        if h > 0.01 && k > 0 {
                            let c = card.right_bottom() - vec2(24.0, 24.0);
                            let pr = ui.interact(Rect::from_center_size(c, vec2(38.0, 38.0)), Id::unique(("wax-listplay", id)), Sense::click());
                            let p = ui.painter();
                            p.circle_filled(c, 17.0, paint::alpha(Color32::WHITE, h));
                            paint::icon(p, c, 9.5, Icon::Play, paint::alpha(paint::INK, h));
                            if pr.clicked() {
                                play = Some(id);
                                continue;
                            }
                        }
                        if resp.clicked() {
                            open = Some(id);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Play").clicked() {
                                play = Some(id);
                                ui.close();
                            }
                            if ui.button("Delete playlist").clicked() {
                                self.playlists.delete(id);
                                self.toast("Playlist deleted", time);
                                ui.close();
                            }
                        });
                    }
                }
            });
        });
        if create {
            self.new_playlist(Vec::new(), time);
        }
        if let Some(id) = open {
            self.open_list = Some(id);
        }
        if let Some(id) = play {
            self.play_playlist(id, 0);
        }
    }

    fn playlist_view(&mut self, ui: &mut egui::Ui, area: Rect, id: u64, time: f32) {
        if self.back_link(ui, area, "Playlists") {
            self.open_list = None;
            self.renaming = None;
            return;
        }
        let Some(list) = self.playlists.get(id) else { return };
        let name = list.name.clone();
        let k = list.entries.len();
        let meta = format!("{k} song{}  ·  {} min", if k == 1 { "" } else { "s" }, (list.duration_ms() / 60_000).max(if k > 0 { 1 } else { 0 }));
        let covers = self.covers_of(id, 4);
        let header = Header { covers, title: &name, sub: "Playlist", meta: &meta, key: id };
        let (list_top, action) = self.header(ui, area, &header, Some(id));
        match action {
            Some(HeaderAction::Play) if k > 0 => {
                self.set_shuffle(false);
                self.play_playlist(id, 0);
            }
            Some(HeaderAction::Shuffle) if k > 0 => {
                self.set_shuffle(true);
                self.play_playlist(id, (time * 1000.0) as usize % k);
            }
            Some(HeaderAction::Delete) => {
                self.playlists.delete(id);
                self.open_list = None;
                self.toast(format!("Deleted “{name}”"), time);
                return;
            }
            _ => {}
        }

        let list_rect = Rect::from_min_max(pos2(area.left() - 10.0, list_top), area.max);
        if k == 0 {
            let p = ui.painter_at(list_rect);
            let msg = "Empty for now. Right-click any song, or use the add button in the dock, to put it here.";
            let g = p.layout(msg.into(), paint::body(13.5), paint::DIM, list_rect.width() - 30.0);
            p.galley(list_rect.left_top() + vec2(12.0, 10.0), g, paint::DIM);
            return;
        }
        let list = self.playlists.get(id).unwrap();
        let tracks = list.tracks(&self.lib);
        let rows: Vec<Row> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| Row {
                number: i + 1,
                title: t.title.to_string(),
                sub: Some(t.artist.to_string()),
                duration_ms: t.duration_ms,
                path: t.path.clone(),
                art: Some((t.album_key(), t.path.clone())),
            })
            .collect();
        let out = self.track_rows(ui, list_rect, &rows, Some(id), time);
        if let Some(i) = out.play {
            self.play_playlist(id, i);
        }
        if let Some(i) = out.menu_for {
            self.pending_menu = Some((vec![tracks[i].clone()], Some((id, i))));
        }
        if let Some((from, to)) = out.moved {
            self.playlists.move_entry(id, from, to);
        }
    }

    /// Cover art for a playlist's mosaic: distinct records, in order.
    fn covers_of(&mut self, id: u64, max: usize) -> Vec<(Option<TextureId>, Palette)> {
        let Some(list) = self.playlists.get(id) else { return Vec::new() };
        let mut keys: Vec<(u64, std::path::PathBuf)> = Vec::new();
        for e in &list.entries {
            let t = e.resolve(&self.lib);
            let key = t.album_key();
            if !keys.iter().any(|(k, _)| *k == key) {
                keys.push((key, t.path.clone()));
                if keys.len() == max {
                    break;
                }
            }
        }
        keys.into_iter().map(|(k, p)| self.art.thumb(k, &p)).collect()
    }

    fn back_link(&mut self, ui: &mut egui::Ui, area: Rect, label: &str) -> bool {
        let back = Rect::from_min_size(area.min - vec2(6.0, 2.0), vec2(120.0, 26.0));
        let resp = ui.interact(back, Id::unique(("wax-back", label)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        let p = ui.painter();
        let col = if resp.hovered() { paint::TEXT } else { paint::DIM };
        paint::icon(p, pos2(area.left() + 3.0, back.center().y), 9.0, Icon::Back, col);
        p.text(pos2(area.left() + 16.0, back.center().y), Align2::LEFT_CENTER, label, paint::semibold(12.5), col);
        resp.clicked()
    }

    /// Cover, title block and Play / Shuffle for a record or playlist.
    /// Returns where the track list starts and which button was pressed.
    fn header(&mut self, ui: &mut egui::Ui, area: Rect, h: &Header, playlist: Option<u64>) -> (f32, Option<HeaderAction>) {
        let p = ui.painter_at(area.expand(20.0));
        let accent = self.accent();
        let size = 124.0;
        let cover = Rect::from_min_size(area.min + vec2(0.0, 36.0), vec2(size, size));
        if playlist.is_none() {
            let pal = h.covers.first().map_or(Palette::from_key(h.key), |c| c.1);
            paint::vinyl_mini(&p, pos2(cover.right() - size * 0.08, cover.center().y), size * 0.46, pal);
            let (tex, pal) = h.covers.first().copied().unwrap_or((None, pal));
            paint::sleeve(&p, cover, tex, pal, h.title, 6.0);
        } else {
            mosaic(&p, cover, &h.covers, h.title, 10.0);
        }

        let tx = cover.right() + if playlist.is_none() { size * 0.38 + 20.0 } else { 18.0 };
        let tw = area.right() - 10.0 - tx;
        let mut y = cover.top() + 2.0;

        // Playlist names are edited in place.
        let editing = playlist.is_some() && self.renaming.is_some();
        if editing {
            let id = playlist.unwrap();
            let rid = Id::unique("wax-rename");
            let r = Rect::from_min_size(pos2(tx - 4.0, y - 2.0), vec2(tw, 34.0));
            p.rect_filled(r, 8.0, Color32::from_white_alpha(18));
            let name = self.renaming.as_mut().unwrap();
            let resp = ui.put(
                r.shrink2(vec2(6.0, 3.0)),
                egui::TextEdit::singleline(name).id(rid).frame(egui::Frame::NONE).font(paint::display(22.0)),
            );
            if !resp.has_focus() && !resp.lost_focus() {
                resp.request_focus();
            }
            let done = resp.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
            if done {
                let name = self.renaming.take().unwrap_or_default();
                self.playlists.rename(id, &name);
            }
            y += 38.0;
        } else {
            let mut job = egui::text::LayoutJob::simple(h.title.to_string(), paint::display(23.0), paint::TEXT, tw);
            job.wrap.max_rows = 2;
            let g = p.layout_job(job);
            let gh = g.size().y;
            let title_rect = Rect::from_min_size(pos2(tx, y), g.size());
            p.galley(pos2(tx, y), g, paint::TEXT);
            if let Some(_) = playlist {
                let r = ui.interact(title_rect, Id::unique("wax-title-edit"), Sense::click()).on_hover_cursor(CursorIcon::Text).on_hover_text("Click to rename");
                if r.clicked() {
                    self.renaming = Some(h.title.to_string());
                }
            }
            y += gh + 6.0;
        }
        p.galley(pos2(tx, y), paint::elided(&p, h.sub, paint::semibold(13.5), accent, tw), accent);
        y += 21.0;
        p.text(pos2(tx, y), Align2::LEFT_TOP, h.meta, paint::body(12.0), paint::DIM);

        let by = cover.bottom() - 32.0;
        let mut action = None;
        let play = pill(ui, Id::unique(("wax-hplay", h.key)), pos2(tx, by), "Play", Some(accent));
        if play.clicked() {
            action = Some(HeaderAction::Play);
        }
        let shuf = pill(ui, Id::unique(("wax-hshuf", h.key)), pos2(play.rect.right() + 8.0, by), "Shuffle", None);
        if shuf.clicked() {
            action = Some(HeaderAction::Shuffle);
        }
        if playlist.is_some() {
            let more = pill(ui, Id::unique(("wax-hmore", h.key)), pos2(shuf.rect.right() + 8.0, by), "···", None);
            egui::Popup::menu(&more).show(|ui| {
                if ui.button("Rename").clicked() {
                    self.renaming = Some(h.title.to_string());
                    ui.close();
                }
                if ui.button("Delete playlist").clicked() {
                    action = Some(HeaderAction::Delete);
                    ui.close();
                }
            });
        }
        (cover.bottom() + 20.0, action)
    }

    /// A track list shared by records and playlists. Playlists get drag to
    /// reorder.
    fn track_rows(&mut self, ui: &mut egui::Ui, area: Rect, rows: &[Row], playlist: Option<u64>, time: f32) -> RowsOut {
        let mut out = RowsOut::default();
        let row_h = if rows.iter().any(|r| r.art.is_some()) { 50.0 } else { 40.0 };
        let accent = self.accent();
        let current = self.now.as_ref().map(|n| n.track.path.clone());
        let playing = self.playing();
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let released = ui.input(|i| i.pointer.any_released());
        let mut drop_line = None;

        ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
            ScrollArea::vertical().auto_shrink(false).show_rows(ui, row_h, rows.len(), |ui, range| {
                for i in range {
                    let row = &rows[i];
                    let sense = if playlist.is_some() { Sense::click_and_drag() } else { Sense::click() };
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width() - 12.0, row_h), sense);
                    let resp = resp.on_hover_cursor(if self.drag.is_some() { CursorIcon::Grabbing } else { CursorIcon::PointingHand });
                    let p = ui.painter();
                    let is_now = current.as_deref() == Some(row.path.as_path());
                    let dragging_this = self.drag.is_some_and(|(from, _)| from == i);
                    if resp.hovered() || dragging_this {
                        p.rect_filled(r.shrink2(vec2(0.0, 1.0)), 8.0, Color32::from_white_alpha(if dragging_this { 24 } else { 14 }));
                    }
                    let mid = r.center().y;
                    let mut x = r.left() + 14.0;
                    // Number / eq bars / play glyph.
                    let nc = pos2(x + 10.0, mid);
                    if is_now {
                        paint::eq_bars(p, nc - vec2(0.0, 2.0), accent, time, playing);
                    } else if resp.hovered() && self.drag.is_none() {
                        paint::icon(p, nc, 7.5, Icon::Play, paint::TEXT);
                    } else {
                        p.text(nc, Align2::CENTER_CENTER, row.number, paint::body(12.0), paint::DIM);
                    }
                    x += 30.0;
                    if let Some((key, path)) = &row.art {
                        let (tex, pal) = self.art.thumb(*key, path);
                        let c = Rect::from_min_size(pos2(x, mid - 18.0), vec2(36.0, 36.0));
                        paint::sleeve(p, c, tex, pal, &row.title, 4.0);
                        x += 48.0;
                    }
                    let dur = if row.duration_ms > 0 {
                        p.text(pos2(r.right() - 12.0, mid), Align2::RIGHT_CENTER, fmt_time(row.duration_ms as u64), paint::body(12.0), paint::DIM)
                    } else {
                        Rect::from_min_size(pos2(r.right() - 12.0, mid), vec2(0.0, 0.0))
                    };
                    let avail = dur.left() - 14.0 - x;
                    let col = if is_now { accent } else { paint::TEXT };
                    if row.art.is_some() {
                        p.galley(pos2(x, mid - 17.0), paint::elided(p, &row.title, paint::semibold(13.5), col, avail), col);
                        if let Some(sub) = &row.sub {
                            p.galley(pos2(x, mid + 2.0), paint::elided(p, sub, paint::body(12.0), paint::DIM, avail), paint::DIM);
                        }
                    } else {
                        let g = paint::elided(p, &row.title, paint::body(14.0), col, avail);
                        let used = g.size().x;
                        p.galley(pos2(x, mid - g.size().y * 0.5), g, col);
                        if let Some(sub) = row.sub.as_ref().filter(|_| avail - used > 70.0) {
                            let g = paint::elided(p, sub, paint::body(12.0), paint::DIM, avail - used - 12.0);
                            p.galley(pos2(x + used + 10.0, mid - g.size().y * 0.5 + 1.0), g, paint::DIM);
                        }
                    }

                    // Drag to reorder (playlists only).
                    if resp.drag_started() {
                        self.drag = Some((i, i));
                    }
                    if let (Some((from, _)), Some(pos)) = (self.drag, pointer) {
                        if pos.y >= r.top() && pos.y < r.bottom() {
                            let to = if pos.y < mid { i } else { i + 1 };
                            self.drag = Some((from, to));
                            drop_line = Some(if to == i { r.top() } else { r.bottom() });
                        }
                    }
                    if resp.clicked() {
                        out.play = Some(i);
                    }
                    if resp.secondary_clicked() {
                        out.menu_for = Some(i);
                    }
                }
            });
        });
        if let (Some(y), Some(_)) = (drop_line, self.drag) {
            ui.painter().line_segment([pos2(area.left() + 10.0, y), pos2(area.right() - 20.0, y)], Stroke::new(2.0, accent));
        }
        if released {
            if let Some((from, to)) = self.drag.take() {
                let to = if to > from { to - 1 } else { to };
                if to != from {
                    out.moved = Some((from, to));
                }
            }
        }
        out
    }

    // ------------------------------------------------------------- search

    /// Search results as one keyboard-navigable list.
    fn results_view(&mut self, ui: &mut egui::Ui, area: Rect, time: f32) {
        let ctx = ui.ctx().clone();
        let rows = self.search_rows();
        self.sel = self.sel.min(rows.len().saturating_sub(1));
        let accent = self.accent();
        let now_path = self.now.as_ref().map(|n| n.track.path.clone());
        let playing = self.playing();
        let mut clicked = None;
        let mut menu = None;

        ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let width = ui.available_width() - 12.0;
                let header = |ui: &mut egui::Ui, text: &str, note: Option<(String, bool)>| {
                    let (r, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
                    let p = ui.painter();
                    let cap = paint::spaced(p, pos2(r.left() + 2.0, r.bottom() - 9.0), Align2::LEFT_BOTTOM, text, paint::semibold(10.5), paint::DIM, 2.5);
                    if let Some((note, busy)) = note {
                        let x = cap.right() + 12.0;
                        if busy {
                            for i in 0..3 {
                                let a = ((time * 4.0 - i as f32 * 0.6).sin() * 0.5 + 0.5).powi(2);
                                p.circle_filled(pos2(x + 4.0 + i as f32 * 7.0, cap.center().y), 2.2, paint::alpha(accent, 0.3 + 0.7 * a));
                            }
                        } else {
                            p.text(pos2(x, cap.center().y), Align2::LEFT_CENTER, note, paint::body(12.0), paint::FAINT);
                        }
                    }
                };

                let mut shown_stream_header = false;
                if rows.iter().any(|h| !matches!(h, Hit::Stream(_))) {
                    header(ui, "IN YOUR COLLECTION", None);
                }
                for (i, &hit) in rows.iter().enumerate() {
                    if matches!(hit, Hit::Stream(_)) && !shown_stream_header {
                        shown_stream_header = true;
                        ui.add_space(6.0);
                        header(ui, "FROM YOUTUBE", None);
                    }
                    let (title, sub, right, art, is_now, saved): (String, String, String, (u64, std::path::PathBuf), bool, bool) = match hit {
                        Hit::Album(a) => {
                            let al = &self.lib.albums[a];
                            let n = al.tracks.len();
                            let sub = format!("Record  ·  {}  ·  {n} track{}", al.artist, if n == 1 { "" } else { "s" });
                            let is_now = now_path.as_ref().is_some_and(|p| al.tracks.iter().any(|t| &t.path == p));
                            (al.title.to_string(), sub, String::new(), (al.key, al.tracks[0].path.clone()), is_now, false)
                        }
                        Hit::Track(a, t) => {
                            let al = &self.lib.albums[a];
                            let tr = &al.tracks[t];
                            let sub = format!("{}  ·  {}", tr.artist, al.title);
                            let is_now = now_path.as_ref() == Some(&tr.path);
                            (tr.title.to_string(), sub, fmt_time(tr.duration_ms as u64), (al.key, tr.path.clone()), is_now, false)
                        }
                        Hit::Stream(s) => {
                            let tr = &self.online.results[s];
                            let right = if tr.duration_ms > 0 { fmt_time(tr.duration_ms as u64) } else { String::new() };
                            let is_now = now_path.as_ref() == Some(&tr.path);
                            let saved = tr.stream_id().is_some_and(|id| self.cfg.saved.contains_key(id));
                            (tr.title.to_string(), tr.artist.to_string(), right, (tr.album_key(), tr.path.clone()), is_now, saved)
                        }
                    };

                    let (row, resp) = ui.allocate_exact_size(vec2(width, 52.0), Sense::click());
                    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                    if resp.hovered() && ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                        self.sel = i;
                    }
                    let selected = self.sel == i;
                    let p = ui.painter();
                    let h = ctx.animate_bool_with_time(Id::unique(("wax-row", i)), selected, 0.1);
                    if h > 0.0 {
                        p.rect_filled(row, 10.0, Color32::from_white_alpha((h * 18.0) as u8));
                    }

                    let (tex, pal) = self.art.thumb(art.0, &art.1);
                    let cover = Rect::from_min_size(row.left_top() + vec2(8.0, 7.0), vec2(38.0, 38.0));
                    paint::sleeve(p, cover, tex, pal, &title, 5.0);
                    if is_now {
                        p.rect_filled(cover, 5.0, Color32::from_black_alpha(140));
                        paint::eq_bars(p, cover.center() + vec2(0.0, -2.0), Color32::WHITE, time, playing);
                    } else if selected && h > 0.5 {
                        p.rect_filled(cover, 5.0, Color32::from_black_alpha(110));
                        paint::icon(p, cover.center(), 8.5, Icon::Play, Color32::WHITE);
                    }

                    let mut rx = row.right() - 12.0;
                    if !right.is_empty() {
                        rx = p.text(pos2(rx, row.center().y), Align2::RIGHT_CENTER, right, paint::body(12.0), paint::DIM).left() - 10.0;
                    }
                    if saved {
                        paint::icon(p, pos2(rx - 6.0, row.center().y), 10.0, Icon::Check, accent);
                        rx -= 22.0;
                    }
                    let tx = cover.right() + 12.0;
                    let tw = rx - 4.0 - tx;
                    let col = if is_now { accent } else { paint::TEXT };
                    p.galley(pos2(tx, row.top() + 9.0), paint::elided(p, &title, paint::semibold(13.5), col, tw), col);
                    p.galley(pos2(tx, row.top() + 28.0), paint::elided(p, &sub, paint::body(12.0), paint::DIM, tw), paint::DIM);

                    if resp.clicked() {
                        clicked = Some(hit);
                    }
                    if resp.secondary_clicked() {
                        menu = Some(hit);
                    }
                }

                // YouTube status when there's nothing (yet) to list from it.
                if !shown_stream_header {
                    let busy = self.online_busy();
                    let note = if busy {
                        Some((String::new(), true))
                    } else if let Some(e) = self.online.error.as_ref().filter(|_| self.online.query == self.search.trim()) {
                        Some((e.clone(), false))
                    } else if self.search.trim().chars().count() < 3 {
                        Some(("keep typing, or press Enter".into(), false))
                    } else {
                        None
                    };
                    if let Some(note) = note {
                        ui.add_space(6.0);
                        header(ui, "FROM YOUTUBE", Some(note));
                    }
                }
                if rows.is_empty() && !self.online_busy() {
                    let (r, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
                    ui.painter().text(r.left_center() + vec2(2.0, 0.0), Align2::LEFT_CENTER, "Press Enter to look it up", paint::body(13.5), paint::DIM);
                }
            });
        });
        if let Some(hit) = clicked {
            self.activate(&ctx, hit);
        }
        if let Some(hit) = menu {
            let tracks = match hit {
                Hit::Album(a) => self.lib.albums[a].tracks.clone(),
                Hit::Track(a, t) => vec![self.lib.albums[a].tracks[t].clone()],
                Hit::Stream(s) => vec![self.online.results[s].clone()],
            };
            self.pending_menu = Some((tracks, None));
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum HeaderAction {
    Play,
    Shuffle,
    Delete,
}

struct Header<'a> {
    covers: Vec<(Option<TextureId>, Palette)>,
    title: &'a str,
    sub: &'a str,
    meta: &'a str,
    key: u64,
}

struct Row {
    number: usize,
    title: String,
    sub: Option<String>,
    duration_ms: u32,
    path: std::path::PathBuf,
    /// Per-row cover (playlists mix records).
    art: Option<(u64, std::path::PathBuf)>,
}

#[derive(Default)]
struct RowsOut {
    play: Option<usize>,
    menu_for: Option<usize>,
    moved: Option<(usize, usize)>,
}

/// A playlist cover: up to four record sleeves in a grid.
pub fn mosaic(p: &egui::Painter, rect: Rect, covers: &[(Option<TextureId>, Palette)], name: &str, radius: f32) {
    let r = radius as u8;
    if covers.len() < 4 {
        match covers.first() {
            Some(&(tex, pal)) => paint::sleeve(p, rect, tex, pal, name, radius),
            None => {
                let pal = Palette::from_key(name.bytes().fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)));
                paint::sleeve(p, rect, None, pal, "", radius);
                paint::icon(p, rect.center(), rect.width() * 0.2, Icon::List, Color32::from_white_alpha(220));
            }
        }
        return;
    }
    for (i, &(tex, pal)) in covers.iter().take(4).enumerate() {
        let (cx, cy) = ((i % 2) as f32, (i / 2) as f32);
        let q = Rect::from_min_size(rect.min + vec2(cx * rect.width() * 0.5, cy * rect.height() * 0.5), rect.size() * 0.5);
        let cr = CornerRadius {
            nw: if i == 0 { r } else { 0 },
            ne: if i == 1 { r } else { 0 },
            sw: if i == 2 { r } else { 0 },
            se: if i == 3 { r } else { 0 },
        };
        match tex {
            Some(t) => {
                let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                p.add(RectShape::filled(q, cr, Color32::WHITE).with_texture(t, uv));
            }
            None => {
                p.rect_filled(q, cr, paint::shade(pal.a, 0.8));
            }
        }
    }
    p.rect_stroke(rect, radius, Stroke::new(1.0, Color32::from_white_alpha(22)), egui::StrokeKind::Inside);
}

fn dashed(p: &egui::Painter, rect: Rect, radius: f32, col: Color32) {
    let _ = radius;
    let stroke = Stroke::new(1.2, col);
    let seg = 6.0;
    let edges = [
        (rect.left_top() + vec2(radius, 0.0), rect.right_top() - vec2(radius, 0.0)),
        (rect.left_bottom() + vec2(radius, 0.0), rect.right_bottom() - vec2(radius, 0.0)),
        (rect.left_top() + vec2(0.0, radius), rect.left_bottom() - vec2(0.0, radius)),
        (rect.right_top() + vec2(0.0, radius), rect.right_bottom() - vec2(0.0, radius)),
    ];
    for (a, b) in edges {
        let len = (b - a).length();
        let dir = (b - a) / len;
        let mut t = 0.0;
        while t < len {
            p.line_segment([a + dir * t, a + dir * (t + seg).min(len)], stroke);
            t += seg * 2.0;
        }
    }
    for (c, a0) in [
        (rect.left_top() + vec2(radius, radius), std::f32::consts::PI),
        (rect.right_top() + vec2(-radius, radius), -std::f32::consts::FRAC_PI_2),
        (rect.right_bottom() + vec2(-radius, -radius), 0.0),
        (rect.left_bottom() + vec2(radius, -radius), std::f32::consts::FRAC_PI_2),
    ] {
        let pts: Vec<egui::Pos2> = (0..=6).map(|i| c + egui::Vec2::angled(a0 + i as f32 / 6.0 * std::f32::consts::FRAC_PI_2) * radius).collect();
        p.add(egui::Shape::line(pts, stroke));
    }
}

