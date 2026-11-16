//! Application state and the per-frame loop. The two big views live in
//! `stage.rs` (the record) and `shelf.rs` (the collection).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CursorIcon, FontData, FontDefinitions, FontFamily, Id, Key, Rect, Sense,
    Stroke, ViewportCommand,
};

use crate::art::{ArtCache, Palette};
use crate::engine::{Cmd, Engine};
use crate::library::{self, Album, Config, Library, Track};
use crate::paint::{self, Icon};
use crate::mini::MINI;
use crate::playlist::Playlists;
use crate::stream;
use crate::wave::WaveWorker;

/// Height of the title band along the top edge.
pub const TOP: f32 = 44.0;
/// Gap around and between the floating cards.
const GUTTER: f32 = 10.0;
const DOCK_H: f32 = 76.0;
const FULL_MIN: egui::Vec2 = egui::Vec2::new(920.0, 600.0);
/// 33⅓ rpm in radians per second.
const RPM_33: f32 = 3.49;

pub struct Launch {
    pub shot: Option<PathBuf>,
    pub autoplay: bool,
    pub open_album: bool,
    pub search: Option<String>,
    /// Seconds after launch to take the `--shot` screenshot.
    pub shot_delay: f32,
}

pub struct Now {
    pub track: Arc<Track>,
    /// Track boundaries across the album, 0..1.
    pub gaps: Vec<f32>,
    /// Where this track starts within the album.
    pub offset_ms: u64,
    pub album_ms: u64,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Records,
    Playlists,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SaveState {
    /// A file in the collection already.
    Local,
    Unsaved,
    Saving,
    Saved,
}

/// One row of the search results, top to bottom.
#[derive(Clone, Copy, PartialEq)]
pub enum Hit {
    Album(usize),
    Track(usize, usize),
    Stream(usize),
}

/// Songs found on YouTube for the current search.
#[derive(Default)]
pub struct Online {
    /// The query `results` belong to.
    pub query: String,
    pub results: Vec<Arc<Track>>,
    pub error: Option<String>,
    pub pending: Option<(String, Receiver<Result<Vec<Track>, String>>)>,
    /// Play the first result as soon as it arrives (Enter before results).
    pub autoplay: bool,
    pub edited_at: f32,
}

pub struct ScanJob {
    rx: Receiver<Vec<Track>>,
    progress: Arc<AtomicUsize>,
}

pub struct App {
    pub cfg: Config,
    pub lib: Library,
    pub engine: Engine,
    pub art: ArtCache,
    pub wave: WaveWorker,
    pub scan: Option<ScanJob>,

    pub generation: u64,
    pub now: Option<Now>,
    pub peaks: Option<(u64, Arc<[u8]>)>,
    pub peaks_at: f32,
    /// Length measured by decoding, for streams whose length wasn't listed.
    pub measured_ms: u32,
    /// Displayed palette, eased towards the playing album's colours.
    pub pal: [[f32; 3]; 2],
    pub angle: f32,
    pub spin: f32,
    pub slide: f32,
    /// Time base for the background drift; only advances while music moves.
    pub flow: f32,
    pub scrub: Option<f32>,

    pub search: String,
    pub applied_search: Option<String>,
    pub filtered: Vec<usize>,
    /// Local tracks whose title matches: (album, track).
    pub track_hits: Vec<(usize, usize)>,
    pub online: Online,
    /// Highlighted search row (Up/Down, Enter).
    pub sel: usize,
    /// After playing from search, the next keystroke starts a new query.
    pub fresh_search: bool,
    pub view: Option<Arc<Album>>,
    pub toast: Option<(String, f32)>,
    pub extra_fonts: (bool, bool),

    pub launch: Launch,
    pub frames: u64,
    pub shot_taken: bool,
    pub glass: crate::platform::Glass,

    pub tab: Tab,
    pub playlists: Playlists,
    pub open_list: Option<u64>,
    /// Name being typed for the open playlist.
    pub renaming: Option<String>,
    /// Playlist row being dragged: (from, drop position).
    pub drag: Option<(usize, usize)>,
    /// Right-click menu requested from a list: songs, and playlist row if any.
    pub pending_menu: Option<(Vec<Arc<Track>>, Option<(u64, usize)>)>,
    pub menu_at: Option<(egui::Pos2, u64)>,
    pub saving: Vec<Receiver<(Arc<Track>, Result<Track, String>)>>,
    pub saving_ids: HashSet<String>,
    pub mini: bool,
    /// Last session's track was restored, or something else was played first.
    pub resumed: bool,
}

pub fn search_id() -> Id {
    Id::unique("wax-search")
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, launch: Launch) -> Self {
        let ctx = &cc.egui_ctx;
        install_fonts(ctx);
        install_style(ctx);

        let cfg = Config::load();
        let lib = Library::build(library::load_cached_tracks());
        let wake = ctx.clone();
        let volume = if launch.shot.is_some() { 0.0 } else { cfg.volume };
        let engine = Engine::spawn(move || wake.request_repaint(), volume, cfg.shuffle, cfg.repeat);
        let wake = ctx.clone();
        let default = default_palette();

        let mut app = Self {
            art: ArtCache::spawn(ctx),
            wave: WaveWorker::spawn(move || wake.request_repaint()),
            scan: None,
            generation: 0,
            now: None,
            peaks: None,
            peaks_at: 0.0,
            measured_ms: 0,
            pal: [rgb_f(default.a), rgb_f(default.b)],
            angle: 0.0,
            spin: 0.0,
            slide: 1.0,
            flow: 0.0,
            scrub: None,
            search: String::new(),
            applied_search: None,
            filtered: Vec::new(),
            track_hits: Vec::new(),
            online: Online::default(),
            sel: 0,
            fresh_search: false,
            view: None,
            toast: None,
            extra_fonts: (false, false),
            launch,
            frames: 0,
            shot_taken: false,
            glass: Default::default(),
            tab: Tab::Records,
            playlists: Playlists::load(),
            open_list: None,
            renaming: None,
            drag: None,
            pending_menu: None,
            menu_at: None,
            saving: Vec::new(),
            saving_ids: HashSet::new(),
            mini: false,
            resumed: false,
            cfg,
            lib,
            engine,
        };
        app.library_changed(ctx);
        if let Some(q) = app.launch.search.take() {
            app.search = q.clone();
            app.refresh_filter(0.0);
            app.submit_search(ctx);
        } else {
            app.resume();
        }
        if !app.cfg.folders.is_empty() {
            app.rescan(ctx);
        }
        app
    }

    pub fn palette(&self) -> Palette {
        let c = |v: [f32; 3]| Color32::from_rgb(v[0] as u8, v[1] as u8, v[2] as u8);
        Palette { a: c(self.pal[0]), b: c(self.pal[1]) }
    }

    /// Accent colour that stays readable on the dark panels.
    pub fn accent(&self) -> Color32 {
        let a = self.palette().a;
        let lum = a.r() as f32 * 0.3 + a.g() as f32 * 0.59 + a.b() as f32 * 0.11;
        if lum < 120.0 { paint::lerp_color(a, Color32::WHITE, 0.35) } else { a }
    }

    pub fn playing(&self) -> bool {
        self.engine.shared.playing.load(Ordering::Relaxed)
    }

    pub fn toast(&mut self, text: impl Into<String>, time: f32) {
        self.toast = Some((text.into(), time));
    }

    // ---------------------------------------------------------- actions

    pub fn play_album(&mut self, album: &Arc<Album>, index: usize) {
        self.resumed = true;
        self.engine.send(Cmd::Play { queue: album.tracks.clone(), index, paused: false, seek_ms: 0 });
    }

    pub fn set_shuffle(&mut self, on: bool) {
        self.cfg.shuffle = on;
        self.engine.send(Cmd::Shuffle(on));
        self.cfg.save();
    }

    pub fn set_volume(&mut self, v: f32) {
        self.cfg.volume = v.clamp(0.0, 1.0);
        self.engine.send(Cmd::Volume(self.cfg.volume));
    }

    pub fn add_folder(&mut self, ctx: &egui::Context, folder: PathBuf) {
        if !self.cfg.folders.contains(&folder) {
            self.cfg.folders.push(folder);
            self.cfg.save();
        }
        self.rescan(ctx);
    }

    pub fn pick_folder(&mut self, ctx: &egui::Context) {
        if let Some(folder) = rfd::FileDialog::new().set_title("Add a music folder").pick_folder() {
            self.add_folder(ctx, folder);
        }
    }

    pub fn rescan(&mut self, ctx: &egui::Context) {
        if self.scan.is_some() {
            return;
        }
        let folders = self.cfg.folders.clone();
        let known: Vec<Track> =
            self.lib.albums.iter().flat_map(|a| a.tracks.iter().map(|t| (**t).clone())).collect();
        let progress = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();
        let (p, ctx) = (progress.clone(), ctx.clone());
        std::thread::spawn(move || {
            let tracks = library::scan(&folders, known, &p);
            library::save_cached_tracks(&tracks);
            let _ = tx.send(tracks);
            ctx.request_repaint();
        });
        self.scan = Some(ScanJob { rx, progress });
    }

    pub fn scanning(&self) -> Option<usize> {
        self.scan.as_ref().map(|s| s.progress.load(Ordering::Relaxed))
    }

    /// Put last session's record back on the platter, paused where it stopped.
    fn resume(&mut self) {
        // Only ever once, and never over something already asked for.
        if self.resumed || self.now.is_some() || self.generation != 0 {
            return;
        }
        let found = self.cfg.last_track.as_deref().and_then(|p| self.lib.find_track(p));
        let paused = !self.launch.autoplay;
        if let Some((album, index)) = found {
            let seek_ms = self.cfg.last_pos_ms;
            self.engine.send(Cmd::Play { queue: album.tracks.clone(), index, paused, seek_ms });
            self.resumed = true;
        } else if self.launch.autoplay {
            if let Some(album) = self.lib.albums.first().cloned() {
                self.play_album(&album, 0);
            }
        }
    }

    pub fn library_changed(&mut self, ctx: &egui::Context) {
        self.applied_search = None;
        if let Some(v) = &self.view {
            self.view = self.lib.albums.iter().find(|a| a.key == v.key).cloned();
        }
        if self.launch.open_album && self.view.is_none() {
            self.view = self.lib.albums.first().cloned();
        }
        // CJK / Hangul fonts are big; only load them if the tags need them.
        let needs = self.lib.needs_scripts();
        self.ensure_fonts(ctx, needs);
    }

    /// CJK / Hangul fonts are big, so they load only once something needs them.
    fn ensure_fonts(&mut self, ctx: &egui::Context, (cjk, hangul): (bool, bool)) {
        if cjk && !self.extra_fonts.0 {
            self.extra_fonts.0 = true;
            add_fallback_font(ctx, "cjk", "msyh.ttc");
        }
        if hangul && !self.extra_fonts.1 {
            self.extra_fonts.1 = true;
            add_fallback_font(ctx, "hangul", "malgun.ttf");
        }
    }

    // ------------------------------------------------------------ frame

    fn poll(&mut self, ctx: &egui::Context, time: f32, dt: f32) {
        if let Some(job) = &self.scan {
            if let Ok(tracks) = job.rx.try_recv() {
                self.scan = None;
                self.lib = Library::build(tracks);
                self.library_changed(ctx);
                if let Some(now) = &self.now {
                    let track = now.track.clone();
                    self.now = Some(self.describe(track));
                } else {
                    self.resume();
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(150));
            }
        }

        while let Ok((key, peaks, ms)) = self.wave.rx.try_recv() {
            if self.now.as_ref().is_some_and(|n| n.track.key() == key) {
                self.measured_ms = ms;
                self.peaks = Some((key, peaks));
                self.peaks_at = time;
            }
        }

        let generation = self.engine.shared.generation.load(Ordering::Relaxed);
        if generation != self.generation {
            self.generation = generation;
            let current = self.engine.shared.current.lock().unwrap().clone();
            if let Some(track) = current {
                let previous = self.now.as_ref().map(|n| n.track.album_key());
                if previous != Some(track.album_key()) {
                    self.slide = 0.0;
                }
                if self.now.as_ref().is_none_or(|n| n.track.path != track.path) {
                    self.measured_ms = 0;
                }
                self.art.set_large(track.album_key(), &track.path);
                self.peaks = None;
                // A stream is analysed once it starts (and its audio is arriving).
                let buffering = self.engine.shared.buffering.load(Ordering::Relaxed);
                match track.stream_id() {
                    Some(id) if !buffering => self.wave.request(&track, Some(stream::fetch(id))),
                    Some(_) => {}
                    None => self.wave.request(&track, None),
                }
                let local = track.stream_id().is_none();
                if local && self.cfg.last_track.as_deref() != Some(track.path.as_path()) {
                    self.cfg.last_track = Some(track.path.clone());
                    self.cfg.last_pos_ms = 0;
                    self.cfg.save();
                }
                self.now = Some(self.describe(track));
            }
        }

        self.poll_online(ctx, time);
        self.poll_saves(ctx, time);

        let err = self.engine.shared.error.lock().unwrap().take();
        if let Some(err) = err {
            self.toast(format!("Couldn't play {err}"), time);
        }

        // Ease colours, platter speed and the record sliding out of its sleeve.
        let target = match &self.now {
            Some(n) => self.art.thumb(n.track.album_key(), &n.track.path).1,
            None => default_palette(),
        };
        let k = 1.0 - (-dt * 3.5).exp();
        for (cur, tgt) in self.pal.iter_mut().zip([rgb_f(target.a), rgb_f(target.b)]) {
            for i in 0..3 {
                cur[i] += (tgt[i] - cur[i]) * k;
            }
        }
        let target_spin = if self.playing() { RPM_33 } else { 0.0 };
        self.spin += (target_spin - self.spin) * (1.0 - (-dt * 2.5).exp());
        self.angle = (self.angle + self.spin * dt) % std::f32::consts::TAU;
        self.flow += dt * (0.15 + 0.85 * self.spin / RPM_33);
        self.slide = (self.slide + dt * 1.6).min(1.0);
    }

    fn describe(&self, track: Arc<Track>) -> Now {
        let album = self.lib.album_of(&track).cloned();
        let (mut gaps, mut offset_ms, mut album_ms) = (Vec::new(), 0, track.duration_ms as u64);
        if let Some(a) = &album {
            album_ms = a.duration_ms.max(1);
            let mut acc = 0u64;
            for t in &a.tracks {
                if t.path == track.path {
                    offset_ms = acc;
                }
                acc += t.duration_ms as u64;
                gaps.push(acc as f32 / album_ms as f32);
            }
            gaps.pop();
        }
        Now { track, gaps, offset_ms, album_ms }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.memory(|m| m.focused().is_some());
        let (mut typed, mut esc, mut enter) = (String::new(), false, false);
        ctx.input(|i| {
            esc = i.key_pressed(Key::Escape);
            enter = i.key_pressed(Key::Enter);
            if typing {
                return;
            }
            for e in &i.events {
                if let egui::Event::Paste(t) = e {
                    typed.push_str(t.trim());
                }
            }
            let cmd = i.modifiers.command;
            if i.key_pressed(Key::Space) {
                self.engine.send(Cmd::Toggle);
            }
            if i.key_pressed(Key::ArrowRight) {
                self.engine.send(if cmd { Cmd::Next } else { Cmd::SeekBy(5000) });
            }
            if i.key_pressed(Key::ArrowLeft) {
                self.engine.send(if cmd { Cmd::Prev } else { Cmd::SeekBy(-5000) });
            }
            if !cmd && !i.modifiers.alt {
                for e in &i.events {
                    if let egui::Event::Text(t) = e {
                        if t != " " {
                            typed.push_str(t);
                        }
                    }
                }
            }
        });
        let (save_key, mini_key) = ctx.input(|i| (i.modifiers.command && i.key_pressed(Key::S), i.modifiers.command && i.key_pressed(Key::M)));
        if save_key {
            if let Some(track) = self.now.as_ref().map(|n| n.track.clone()) {
                let time = ctx.input(|i| i.time as f32);
                self.save_track(ctx, track, time);
            }
        }
        if mini_key {
            self.set_mini(ctx, !self.mini);
        }
        if !typing {
            let dv = ctx.input(|i| i.key_pressed(Key::ArrowUp) as i8 - i.key_pressed(Key::ArrowDown) as i8);
            if dv != 0 {
                self.set_volume(self.cfg.volume + dv as f32 * 0.05);
                self.cfg.save();
            }
        }

        // Just start typing anywhere to search the collection.
        if !typed.is_empty() && !self.mini {
            if std::mem::take(&mut self.fresh_search) {
                self.search.clear();
            }
            self.search.push_str(&typed);
            self.view = None;
            let id = search_id();
            let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
            let end = egui::text::CCursor::new(self.search.chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ctx, id);
            ctx.memory_mut(|m| m.request_focus(id));
        }
        if esc && self.pending_menu.is_none() {
            if self.renaming.is_some() {
                self.renaming = None;
            } else if typing || !self.search.is_empty() {
                self.search.clear();
                ctx.memory_mut(|m| m.surrender_focus(search_id()));
            } else if self.open_list.is_some() {
                self.open_list = None;
            } else {
                self.view = None;
            }
        }
        if typing && !self.search.trim().is_empty() {
            let rows = self.search_rows().len();
            let dv = ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) as i32 - i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) as i32
            });
            if dv != 0 && rows > 0 {
                self.sel = (self.sel as i32 + dv).clamp(0, rows as i32 - 1) as usize;
            }
            if enter {
                self.submit_search(ctx);
            }
        }

        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for file in dropped {
            {
                let path = file.path().to_path_buf();
                let folder = if path.is_dir() { Some(path) } else { path.parent().map(Into::into) };
                if let Some(folder) = folder {
                    self.add_folder(ctx, folder);
                }
            }
        }
    }

    pub fn refresh_filter(&mut self, time: f32) {
        if self.applied_search.as_deref() == Some(self.search.as_str()) {
            return;
        }
        let q = self.search.to_lowercase();
        let terms: Vec<&str> = q.split_whitespace().collect();
        self.filtered = (0..self.lib.albums.len())
            .filter(|&i| terms.iter().all(|t| self.lib.albums[i].haystack.contains(t)))
            .collect();
        self.track_hits.clear();
        if !terms.is_empty() {
            'scan: for &a in &self.filtered {
                for (t, track) in self.lib.albums[a].tracks.iter().enumerate() {
                    let hay = format!("{} {}", track.title, track.artist).to_lowercase();
                    if terms.iter().all(|w| hay.contains(w)) {
                        self.track_hits.push((a, t));
                        if self.track_hits.len() >= 8 {
                            break 'scan;
                        }
                    }
                }
            }
        }
        self.applied_search = Some(self.search.clone());
        self.online.edited_at = time;
        self.online.autoplay = false;
        self.sel = 0;
    }

    /// Everything the search shows, in order: records, tracks, then YouTube.
    pub fn search_rows(&self) -> Vec<Hit> {
        let mut rows: Vec<Hit> = self.filtered.iter().take(4).map(|&a| Hit::Album(a)).collect();
        rows.extend(self.track_hits.iter().map(|&(a, t)| Hit::Track(a, t)));
        if self.online.query == self.search.trim() {
            rows.extend((0..self.online.results.len()).map(Hit::Stream));
        }
        rows
    }

    pub fn activate(&mut self, ctx: &egui::Context, hit: Hit) {
        match hit {
            Hit::Album(a) => {
                let album = self.lib.albums[a].clone();
                self.play_album(&album, 0);
            }
            Hit::Track(a, t) => {
                let album = self.lib.albums[a].clone();
                self.play_album(&album, t);
            }
            Hit::Stream(i) => {
                self.resumed = true;
                // Like the widget: the whole result list becomes the queue.
                let queue: Vec<Arc<Track>> = self.online.results.iter().map(|t| self.prefer_saved(t)).collect();
                self.engine.send(Cmd::Play { queue, index: i, paused: false, seek_ms: 0 });
            }
        }
        self.fresh_search = true;
        ctx.memory_mut(|m| m.surrender_focus(search_id()));
    }

    /// Enter in the search box: play the highlighted row, or look it up.
    fn submit_search(&mut self, ctx: &egui::Context) {
        let q = self.search.trim().trim_matches('"').to_string();
        // A pasted folder or audio file adds it to the collection.
        let path = PathBuf::from(&q);
        if q.len() > 2 && q.as_bytes()[1] == b':' && path.exists() {
            let folder = if path.is_dir() { path } else { path.parent().map(Into::into).unwrap_or(path) };
            self.add_folder(ctx, folder);
            self.search.clear();
            return;
        }
        let rows = self.search_rows();
        if let Some(&hit) = rows.get(self.sel.min(rows.len().saturating_sub(1))) {
            self.activate(ctx, hit);
        } else {
            self.online.autoplay = true;
            self.resumed = true;
            self.start_online(ctx, q);
        }
    }

    fn start_online(&mut self, ctx: &egui::Context, q: String) {
        if q.is_empty() || self.online.pending.as_ref().is_some_and(|(p, _)| *p == q) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (query, ctx) = (q.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(stream::search(&query));
            ctx.request_repaint();
        });
        self.online.pending = Some((q, rx));
        self.online.error = None;
    }

    pub fn online_busy(&self) -> bool {
        self.online.pending.as_ref().is_some_and(|(p, _)| p == self.search.trim())
    }

    fn poll_online(&mut self, ctx: &egui::Context, time: f32) {
        let q = self.search.trim().to_string();
        // Look songs up once typing pauses.
        let wants = q.chars().count() >= 3 && self.online.query != q && !(q.len() > 2 && q.as_bytes()[1] == b':');
        let waiting = self.online.pending.as_ref().is_some_and(|(p, _)| *p == q);
        if wants && !waiting {
            let idle = time - self.online.edited_at;
            if idle >= 0.6 {
                self.start_online(ctx, q.clone());
            } else {
                ctx.request_repaint_after(Duration::from_secs_f32(0.6 - idle + 0.01));
            }
        }
        let Some((pq, rx)) = &self.online.pending else { return };
        let Ok(result) = rx.try_recv() else { return };
        let pq = pq.clone();
        self.online.pending = None;
        self.online.query = pq.clone();
        match result {
            Ok(tracks) => {
                let text: String = tracks.iter().map(|t| format!("{} {} ", t.title, t.artist)).collect();
                self.ensure_fonts(ctx, library::needs_scripts(&text));
                self.online.results = tracks.into_iter().map(Arc::new).collect();
                self.online.error = None;
                if self.online.autoplay && pq == q && !self.online.results.is_empty() {
                    self.online.autoplay = false;
                    let index = self.search_rows().iter().position(|h| matches!(h, Hit::Stream(_)));
                    if let Some(i) = index {
                        self.sel = i;
                    }
                    self.activate(ctx, Hit::Stream(0));
                } else if let Some(id) = self.online.results.first().and_then(|t| t.stream_id()) {
                    // Start loading the likeliest pick before it's asked for.
                    if self.search_rows().first() == Some(&Hit::Stream(0)) {
                        stream::fetch(id);
                    }
                }
            }
            Err(e) => {
                self.online.results.clear();
                self.online.error = Some(e);
                if self.online.autoplay && pq == q {
                    self.online.autoplay = false;
                    let msg = self.online.error.clone().unwrap_or_default();
                    self.toast(format!("Couldn't search YouTube: {msg}"), time);
                }
            }
        }
    }

    fn window_chrome(&mut self, ui: &mut egui::Ui, full: Rect) {
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        let p = ui.painter().clone();

        // Traffic lights, top left. Their symbols show when the group is hovered.
        let group = Rect::from_min_size(full.min + vec2(14.0, 10.0), vec2(70.0, 24.0));
        let near = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| group.contains(p));
        let lights = [
            ("close", Color32::from_rgb(255, 95, 87)),
            ("min", Color32::from_rgb(254, 188, 46)),
            ("max", Color32::from_rgb(40, 200, 64)),
        ];
        let focused = ctx.input(|i| i.focused);
        for (i, (kind, col)) in lights.into_iter().enumerate() {
            let c = pos2(group.left() + 10.0 + i as f32 * 20.0, group.center().y);
            let resp = ui.interact(Rect::from_center_size(c, vec2(18.0, 18.0)), Id::unique(("wax-light", kind)), Sense::click());
            let fill = if focused || near { col } else { Color32::from_white_alpha(46) };
            p.circle_filled(c, 6.0, fill);
            p.circle_stroke(c, 6.0, Stroke::new(0.5, Color32::from_black_alpha(60)));
            if near {
                let ink = Stroke::new(1.3, Color32::from_black_alpha(150));
                match kind {
                    "close" => {
                        p.line_segment([c + vec2(-2.6, -2.6), c + vec2(2.6, 2.6)], ink);
                        p.line_segment([c + vec2(-2.6, 2.6), c + vec2(2.6, -2.6)], ink);
                    }
                    "min" => {
                        p.line_segment([c + vec2(-3.0, 0.0), c + vec2(3.0, 0.0)], ink);
                    }
                    _ => {
                        p.add(egui::Shape::convex_polygon(vec![c + vec2(-3.0, 1.0), c + vec2(-3.0, -3.0), c + vec2(1.0, -3.0)], ink.color, Stroke::NONE));
                        p.add(egui::Shape::convex_polygon(vec![c + vec2(3.0, -1.0), c + vec2(3.0, 3.0), c + vec2(-1.0, 3.0)], ink.color, Stroke::NONE));
                    }
                }
            }
            if resp.clicked() {
                ctx.send_viewport_cmd(match kind {
                    "min" => ViewportCommand::Minimized(true),
                    "max" => ViewportCommand::Maximized(!maximized),
                    _ => ViewportCommand::Close,
                });
            }
        }

        // Mini player, top right.
        let mc = pos2(full.right() - 30.0, group.center().y);
        if icon_button(ui, Id::unique("wax-to-mini"), mc, 30.0, Icon::Mini, 12.0, paint::DIM).on_hover_text("Mini player  (Ctrl+M)").clicked() {
            self.set_mini(&ctx, true);
        }

        // Resize from the window edges (there is no native frame).
        if maximized || self.mini {
            return;
        }
        let (pos, pressed) = ctx.input(|i| (i.pointer.hover_pos(), i.pointer.primary_pressed()));
        let Some(pos) = pos else { return };
        const E: f32 = 5.0;
        let (l, r) = (pos.x < full.left() + E, pos.x > full.right() - E);
        let (t, b) = (pos.y < full.top() + E, pos.y > full.bottom() - E);
        use egui::ResizeDirection as D;
        let hit = match (l, r, t, b) {
            (true, _, true, _) => Some((D::NorthWest, CursorIcon::ResizeNwSe)),
            (_, true, true, _) => Some((D::NorthEast, CursorIcon::ResizeNeSw)),
            (true, _, _, true) => Some((D::SouthWest, CursorIcon::ResizeNeSw)),
            (_, true, _, true) => Some((D::SouthEast, CursorIcon::ResizeNwSe)),
            (true, ..) => Some((D::West, CursorIcon::ResizeHorizontal)),
            (_, true, ..) => Some((D::East, CursorIcon::ResizeHorizontal)),
            (_, _, true, _) => Some((D::North, CursorIcon::ResizeVertical)),
            (_, _, _, true) => Some((D::South, CursorIcon::ResizeVertical)),
            _ => None,
        };
        if let Some((dir, cursor)) = hit {
            ctx.set_cursor_icon(cursor);
            if pressed {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
            }
        }
    }

    /// Switch between the full window and the floating mini player.
    pub fn set_mini(&mut self, ctx: &egui::Context, on: bool) {
        if on == self.mini {
            return;
        }
        self.mini = on;
        if on {
            let (size, maximized) = ctx.input(|i| (i.viewport().inner_rect.map(|r| r.size()), i.viewport().maximized.unwrap_or(false)));
            if maximized {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(false));
            } else if let Some(s) = size {
                self.cfg.full_size = Some((s.x, s.y));
            }
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(MINI));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(MINI));
            ctx.send_viewport_cmd(ViewportCommand::Resizable(false));
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
        } else {
            let (w, h) = self.cfg.full_size.unwrap_or((1320.0, 820.0));
            ctx.send_viewport_cmd(ViewportCommand::WindowLevel(egui::WindowLevel::Normal));
            ctx.send_viewport_cmd(ViewportCommand::Resizable(true));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(vec2(w, h).max(FULL_MIN)));
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(FULL_MIN));
        }
        self.cfg.save();
    }

    fn overlays(&mut self, ui: &egui::Ui, full: Rect, time: f32) {
        let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("wax-overlay")));
        if ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) {
            p.rect_filled(full, 14.0, Color32::from_black_alpha(170));
            p.rect_stroke(full.shrink(18.0), 14.0, Stroke::new(2.0, self.accent()), egui::StrokeKind::Inside);
            p.text(full.center(), Align2::CENTER_CENTER, "Drop to add to your collection", paint::display(30.0), paint::TEXT);
        }
        if let Some((text, at)) = &self.toast {
            let age = time - at;
            if age > 3.2 {
                self.toast = None;
            } else {
                let a = (age * 6.0).min(1.0) * ((3.2 - age) * 3.0).min(1.0);
                let g = p.layout_no_wrap(text.clone(), paint::body(14.0), paint::alpha(paint::TEXT, a));
                let y = if self.mini { full.bottom() - 20.0 } else { full.bottom() - DOCK_H - GUTTER - 30.0 };
                let rect = Rect::from_center_size(pos2(full.center().x, y), g.size() + vec2(32.0, 18.0));
                p.rect_filled(rect, 20.0, paint::alpha(Color32::from_rgb(34, 36, 46), a * 0.97));
                p.rect_stroke(rect, 20.0, Stroke::new(1.0, paint::alpha(Color32::WHITE, a * 0.1)), egui::StrokeKind::Inside);
                p.galley(rect.min + vec2(16.0, 9.0), g, Color32::WHITE);
                ui.ctx().request_repaint();
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let full = ctx.content_rect();
        self.glass.update(frame, &ctx, if self.mini { full.height() * 0.5 } else { 14.0 });
        let (dt, time, focused) = ctx.input(|i| (i.stable_dt.min(0.05), i.time as f32, i.focused));
        self.frames += 1;

        self.art.begin_frame(&ctx);
        self.poll(&ctx, time, dt);
        self.keys(&ctx);
        self.refresh_filter(time);

        if self.mini {
            self.mini_view(ui, full, time);
        } else {
            // Frosted backdrop tinted with the record's colours.
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            let wr = if maximized { 0.0 } else { 14.0 };
            let tint = paint::lerp_color(Color32::from_rgb(12, 13, 18), self.palette().b, 0.12);
            ui.painter().rect_filled(full, wr, Color32::from_rgba_unmultiplied(tint.r(), tint.g(), tint.b(), 168));
            ui.painter().rect_stroke(full, wr, Stroke::new(1.0, Color32::from_white_alpha(20)), egui::StrokeKind::Inside);

            // The title band drags the window; registered first so real widgets win.
            let strip = Rect::from_min_size(full.min, vec2(full.width(), TOP));
            let drag = ui.interact(strip, Id::unique("wax-titlebar"), Sense::click_and_drag());
            if drag.drag_started() {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }

            let g = GUTTER;
            let dock = Rect::from_min_max(pos2(full.left() + g, full.bottom() - g - DOCK_H), pos2(full.right() - g, full.bottom() - g));
            let shelf_w = (full.width() * 0.38).clamp(380.0, 600.0);
            let shelf = Rect::from_min_max(pos2(full.right() - g - shelf_w, full.top() + TOP), pos2(full.right() - g, dock.top() - g));
            let stage = Rect::from_min_max(pos2(full.left() + g, full.top() + TOP), pos2(shelf.left() - g, dock.top() - g));

            self.stage(ui, stage, time);
            self.shelf(ui, shelf, time);
            self.dock(ui, dock, time);
            self.window_chrome(ui, full);
            self.show_pending_menu(&ctx, time);
        }
        self.overlays(ui, full, time);

        // Only keep painting while something is actually moving.
        let settling = self.spin > 0.01 || self.slide < 1.0 || (time - self.peaks_at) < 0.8;
        if self.playing() || settling {
            if focused {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(Duration::from_millis(33));
            }
        }

        if let Some(path) = self.launch.shot.clone() {
            ctx.request_repaint();
            if time >= self.launch.shot_delay && !self.shot_taken {
                self.shot_taken = true;
                ctx.request_screenshot(move |img| {
                    let _ = image::save_buffer(
                        &path,
                        img.as_raw(),
                        img.width() as u32,
                        img.height() as u32,
                        image::ExtendedColorType::Rgba8,
                    );
                    std::process::exit(0);
                });
            }
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.launch.shot.is_none() {
            self.cfg.last_pos_ms = self.engine.shared.pos_ms.load(Ordering::Relaxed);
            self.cfg.save();
        }
    }
}

// ----------------------------------------------------------------- setup

fn default_palette() -> Palette {
    Palette { a: Color32::from_rgb(52, 150, 160), b: Color32::from_rgb(226, 66, 60) }
}

fn rgb_f(c: Color32) -> [f32; 3] {
    [c.r() as f32, c.g() as f32, c.b() as f32]
}

fn fonts_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts")
}

/// Use the system's Segoe UI and Bahnschrift rather than bundling fonts;
/// falls back to egui's built-in face when they are missing.
fn install_fonts(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    let dir = fonts_dir();
    let mut stack = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (family, name, file) in [
        (FontFamily::Proportional, "body", "segoeui.ttf"),
        (FontFamily::Name("semibold".into()), "semibold", "seguisb.ttf"),
        (FontFamily::Name("display".into()), "display", "bahnschrift.ttf"),
    ] {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            defs.font_data.insert(name.to_owned(), Arc::new(FontData::from_owned(bytes)));
            stack.insert(0, name.to_owned());
        }
        defs.families.insert(family, stack.clone());
    }
    ctx.set_fonts(defs);
}

fn add_fallback_font(ctx: &egui::Context, name: &str, file: &str) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    let Ok(bytes) = std::fs::read(fonts_dir().join(file)) else { return };
    let families = [FontFamily::Proportional, FontFamily::Name("semibold".into()), FontFamily::Name("display".into())]
        .into_iter()
        .map(|family| InsertFontFamily { family, priority: FontPriority::Lowest })
        .collect();
    ctx.add_font(FontInsert { name: name.to_owned(), data: FontData::from_owned(bytes), families });
}

fn install_style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.all_styles_mut(|style| {
        style.spacing.scroll = egui::style::ScrollStyle::floating();
        style.spacing.item_spacing = vec2(0.0, 0.0);
        style.interaction.selectable_labels = false;
        let v = &mut style.visuals;
        v.override_text_color = Some(paint::TEXT);
        v.selection.bg_fill = Color32::from_rgb(70, 76, 96);
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.window_fill = Color32::from_rgb(30, 32, 40);
        v.window_stroke = Stroke::new(1.0, Color32::from_white_alpha(22));
        v.window_corner_radius = egui::CornerRadius::same(12);
        v.menu_corner_radius = egui::CornerRadius::same(12);
        v.panel_fill = paint::PANEL;
        v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(110) };
        // Flat, macOS-style menu rows: no boxes, a soft highlight on hover.
        let w = &mut v.widgets;
        for (state, fill) in [
            (&mut w.inactive, Color32::TRANSPARENT),
            (&mut w.hovered, Color32::from_white_alpha(24)),
            (&mut w.active, Color32::from_white_alpha(36)),
            (&mut w.open, Color32::from_white_alpha(24)),
        ] {
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::NONE;
            state.corner_radius = egui::CornerRadius::same(7);
            state.expansion = 0.0;
        }
        style.spacing.button_padding = vec2(10.0, 5.0);
        style.spacing.menu_margin = egui::Margin::same(6);
    });
}

/// Round button with an icon; returns the response so callers can react.
pub fn icon_button(ui: &egui::Ui, id: Id, center: egui::Pos2, hit: f32, icon: Icon, size: f32, color: Color32) -> egui::Response {
    let rect = Rect::from_center_size(center, vec2(hit, hit));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let h = ui.ctx().animate_bool_with_time(id.with("h"), resp.hovered(), 0.12);
    let p = ui.painter();
    p.circle_filled(center, hit * 0.5, Color32::from_white_alpha((h * 28.0) as u8));
    paint::icon(p, center, size * (1.0 + 0.08 * h), icon, paint::lerp_color(color, Color32::WHITE, h * 0.6));
    resp
}

/// Rounded text button.
pub fn pill(ui: &egui::Ui, id: Id, min: egui::Pos2, label: &str, fill: Option<Color32>) -> egui::Response {
    let p = ui.painter();
    let ink = if fill.is_some() { paint::INK } else { paint::TEXT };
    let g = p.layout_no_wrap(label.to_owned(), paint::semibold(13.5), ink);
    let rect = Rect::from_min_size(min, g.size() + vec2(34.0, 16.0));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
    let h = ui.ctx().animate_bool_with_time(id.with("h"), resp.hovered(), 0.12);
    match fill {
        Some(c) => {
            p.rect_filled(rect, 20.0, paint::lerp_color(c, Color32::WHITE, h * 0.25));
        }
        None => {
            p.rect_filled(rect, 20.0, Color32::from_white_alpha((h * 26.0) as u8));
            p.rect_stroke(rect, 20.0, Stroke::new(1.0, Color32::from_white_alpha(70)), egui::StrokeKind::Inside);
        }
    }
    p.galley(rect.min + vec2(17.0, 8.0), g, ink);
    resp
}
