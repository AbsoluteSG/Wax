//! What the menus and buttons do: saving streamed songs, playlist edits,
//! and the right-click menu shared by every list.

use std::sync::Arc;

use eframe::egui::{self, Id, Pos2};

use crate::app::{App, SaveState, Tab};
use crate::engine::Cmd;
use crate::library::{self, Library, Track};
use crate::save;

impl App {
    pub fn play_tracks(&mut self, queue: Vec<Arc<Track>>, index: usize) {
        if !queue.is_empty() {
            self.resumed = true;
            self.engine.send(Cmd::Play { queue, index, paused: false, seek_ms: 0 });
        }
    }

    pub fn play_playlist(&mut self, id: u64, index: usize) {
        if let Some(list) = self.playlists.get(id) {
            let tracks = list.tracks(&self.lib);
            self.play_tracks(tracks, index);
        }
    }

    /// Create a playlist (optionally with songs in it), open it, and let the
    /// name be typed straight away.
    pub fn new_playlist(&mut self, tracks: Vec<Arc<Track>>, time: f32) {
        let tracks: Vec<Arc<Track>> = tracks.iter().map(|t| self.prefer_saved(t)).collect();
        let id = self.playlists.create();
        if !tracks.is_empty() {
            self.playlists.add(id, &tracks);
        }
        let name = self.playlists.get(id).map(|p| p.name.clone()).unwrap_or_default();
        self.tab = Tab::Playlists;
        self.open_list = Some(id);
        self.search.clear();
        self.renaming = Some(name.clone());
        if !tracks.is_empty() {
            self.toast(format!("Added to “{name}”"), time);
        }
    }

    pub fn add_to_playlist(&mut self, id: u64, tracks: &[Arc<Track>], time: f32) {
        // Songs already saved go in as their files, not their streams.
        let tracks: Vec<Arc<Track>> = tracks.iter().map(|t| self.prefer_saved(t)).collect();
        let added = self.playlists.add(id, &tracks);
        let name = self.playlists.get(id).map(|p| p.name.clone()).unwrap_or_default();
        let msg = match (added, tracks.len()) {
            (0, 1) => format!("Already in “{name}”"),
            (0, _) => format!("All already in “{name}”"),
            (1, _) => format!("Added to “{name}”"),
            (n, _) => format!("Added {n} songs to “{name}”"),
        };
        self.toast(msg, time);
    }

    /// Playlist choices for "Add to playlist".
    pub fn playlist_menu(&mut self, ui: &mut egui::Ui, tracks: Vec<Arc<Track>>, time: f32) {
        ui.set_min_width(180.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        let lists: Vec<(u64, String)> = self.playlists.lists.iter().map(|p| (p.id, p.name.clone())).collect();
        for (id, name) in lists {
            if ui.button(name).clicked() {
                self.add_to_playlist(id, &tracks, time);
                ui.close();
            }
        }
        if !self.playlists.lists.is_empty() {
            ui.separator();
        }
        if ui.button("New playlist…").clicked() {
            self.new_playlist(tracks, time);
            ui.close();
        }
    }

    /// The right-click menu for songs and records. `entry` is set when the
    /// songs are a playlist row, enabling "Remove". Returns true once an
    /// action was taken.
    pub fn track_menu_items(&mut self, ui: &mut egui::Ui, tracks: Vec<Arc<Track>>, entry: Option<(u64, usize)>, time: f32) -> bool {
        ui.set_min_width(200.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        let mut done = false;
        let what = if tracks.len() == 1 { "song" } else { "songs" };
        if ui.button("Play").clicked() {
            self.play_tracks(tracks.clone(), 0);
            done = true;
        }
        if ui.button("Play next").clicked() {
            self.engine.send(Cmd::PlayNext(tracks.clone()));
            self.toast(format!("Playing {} next", if tracks.len() == 1 { "this song" } else { "these songs" }), time);
            done = true;
        }
        if ui.button("Add to queue").clicked() {
            self.engine.send(Cmd::Enqueue(tracks.clone()));
            self.toast(format!("Added {what} to the queue"), time);
            done = true;
        }
        ui.separator();
        ui.menu_button("Add to playlist", |ui| self.playlist_menu(ui, tracks.clone(), time));
        let unsaved: Vec<Arc<Track>> = tracks.iter().filter(|t| self.save_state(t) == SaveState::Unsaved).cloned().collect();
        if !unsaved.is_empty() && ui.button("Save to collection").clicked() {
            let ctx = ui.ctx().clone();
            for t in unsaved {
                self.save_track(&ctx, t, time);
            }
            done = true;
        }
        if let Some((list, index)) = entry {
            ui.separator();
            if ui.button("Remove from playlist").clicked() {
                self.playlists.remove_entry(list, index);
                done = true;
            }
        }
        if done {
            ui.close();
        }
        done
    }

    /// For egui's own right-click menus (`Response::context_menu`).
    pub fn track_menu(&mut self, ui: &mut egui::Ui, tracks: Vec<Arc<Track>>, _what: &str, time: f32) {
        self.track_menu_items(ui, tracks, None, time);
    }

    /// Menus requested from inside scrolling lists, shown where the
    /// pointer was when the row was right-clicked.
    pub fn show_pending_menu(&mut self, ctx: &egui::Context, time: f32) {
        let Some((tracks, entry)) = self.pending_menu.clone() else {
            self.menu_at = None;
            return;
        };
        let (pointer, secondary) = ctx.input(|i| (i.pointer.interact_pos(), i.pointer.secondary_clicked()));
        if self.menu_at.is_none() || secondary {
            self.menu_at = Some((pointer.unwrap_or(Pos2::ZERO), self.frames));
        }
        let (pos, opened) = self.menu_at.unwrap();
        let mut closed = false;
        let area = egui::Area::new(Id::unique("wax-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .constrain(true)
            .show(ctx, |ui| {
                egui::Frame::menu(ui.style()).show(ui, |ui| {
                    ui.set_max_width(230.0);
                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                        closed = self.track_menu_items(ui, tracks, entry, time);
                    });
                });
            });
        // A press on the main UI (not on this menu or its submenu) closes it.
        let press_at = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None });
        let clicked_away = opened != self.frames
            && press_at.is_some_and(|p| ctx.layer_id_at(p).is_none_or(|l| l.order == egui::Order::Background));
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let _ = area;
        if closed || esc || clicked_away {
            self.pending_menu = None;
            self.menu_at = None;
        }
    }

    // --------------------------------------------------------------- save

    pub fn save_state(&self, track: &Track) -> SaveState {
        let Some(id) = track.stream_id() else { return SaveState::Local };
        if self.saving_ids.contains(id) {
            SaveState::Saving
        } else if self.cfg.saved.get(id).is_some_and(|p| p.exists()) {
            SaveState::Saved
        } else {
            SaveState::Unsaved
        }
    }

    pub fn save_track(&mut self, ctx: &egui::Context, track: Arc<Track>, time: f32) {
        if self.save_state(&track) != SaveState::Unsaved {
            return;
        }
        let Some(id) = track.stream_id() else { return };
        self.saving_ids.insert(id.to_owned());
        let wake = ctx.clone();
        self.saving.push(save::spawn(track.clone(), move || wake.request_repaint()));
        self.toast(format!("Saving “{}”…", track.title), time);
    }

    pub fn poll_saves(&mut self, ctx: &egui::Context, time: f32) {
        let mut finished = Vec::new();
        self.saving.retain(|rx| match rx.try_recv() {
            Ok(done) => {
                finished.push(done);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        for (stream_track, result) in finished {
            let id = stream_track.stream_id().unwrap_or_default().to_owned();
            self.saving_ids.remove(&id);
            match result {
                Ok(local) => {
                    let dir = save::save_dir();
                    if !self.cfg.folders.iter().any(|f| local.path.starts_with(f)) {
                        self.cfg.folders.push(dir);
                    }
                    self.cfg.saved.insert(id, local.path.clone());
                    self.cfg.save();
                    self.playlists.relink(&stream_track.path, &local);
                    let title = local.title.clone();
                    let mut tracks = self.lib.all_tracks();
                    tracks.retain(|t| t.path != local.path);
                    tracks.push(local);
                    library::save_cached_tracks(&tracks);
                    self.lib = Library::build(tracks);
                    self.library_changed(ctx);
                    self.toast(format!("Saved “{title}” to your collection"), time);
                }
                Err(e) => self.toast(format!("Couldn't save “{}”: {e}", stream_track.title), time),
            }
        }
    }

    /// A streamed result that's already been saved plays from the file.
    pub fn prefer_saved(&self, track: &Arc<Track>) -> Arc<Track> {
        track
            .stream_id()
            .and_then(|id| self.cfg.saved.get(id))
            .and_then(|p| self.lib.track(p))
            .cloned()
            .unwrap_or_else(|| track.clone())
    }
}
