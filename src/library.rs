//! The collection: tracks on disk, grouped into albums, cached between runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use lofty::prelude::*;
use serde::{Deserialize, Serialize};

/// Formats the decoder can actually play (symphonia has no Opus decoder).
const AUDIO_EXT: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "wav", "m4a", "aac", "aif", "aiff", "mka",
];

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Track {
    pub path: PathBuf,
    pub title: Box<str>,
    pub artist: Box<str>,
    pub album: Box<str>,
    pub album_artist: Box<str>,
    pub track_no: u16,
    pub disc_no: u16,
    pub year: u16,
    pub duration_ms: u32,
    pub mtime: u64,
}

impl Track {
    /// Stable id for on-disk caches; changes when the file is modified.
    pub fn key(&self) -> u64 {
        let mut h = Fnv::new();
        h.write(self.path.to_string_lossy().as_bytes());
        h.write(&self.mtime.to_le_bytes());
        h.finish()
    }

    /// Groups tracks into records. A streamed song is a record of its own.
    pub fn album_key(&self) -> u64 {
        match self.stream_id() {
            Some(id) => album_key("youtube", id),
            None => album_key(&self.album_artist, &self.album),
        }
    }

    pub fn stream_id(&self) -> Option<&str> {
        crate::stream::id_of(self)
    }
}

pub fn album_key(artist: &str, album: &str) -> u64 {
    let mut h = Fnv::new();
    h.write(artist.to_lowercase().as_bytes());
    h.write(&[0]);
    h.write(album.to_lowercase().as_bytes());
    h.finish()
}

pub struct Album {
    pub key: u64,
    pub title: Box<str>,
    pub artist: Box<str>,
    pub year: u16,
    pub tracks: Vec<Arc<Track>>,
    pub duration_ms: u64,
    /// Lowercased "title artist track track…" for substring search.
    pub haystack: Box<str>,
}

#[derive(Default)]
pub struct Library {
    pub albums: Vec<Arc<Album>>,
    pub track_count: usize,
    by_path: HashMap<PathBuf, Arc<Track>>,
}

impl Library {
    pub fn build(tracks: Vec<Track>) -> Self {
        let track_count = tracks.len();
        let mut by_path = HashMap::with_capacity(track_count);
        let mut groups: HashMap<u64, Vec<Arc<Track>>> = HashMap::new();
        for t in tracks {
            let t = Arc::new(t);
            by_path.insert(t.path.clone(), t.clone());
            groups.entry(t.album_key()).or_default().push(t);
        }
        let mut albums: Vec<Arc<Album>> = groups
            .into_iter()
            .map(|(key, mut tracks)| {
                tracks.sort_by(|a, b| {
                    (a.disc_no, a.track_no, &a.title).cmp(&(b.disc_no, b.track_no, &b.title))
                });
                let first = &tracks[0];
                let mut haystack = format!("{} {}", first.album, first.album_artist);
                for t in &tracks {
                    haystack.push(' ');
                    haystack.push_str(&t.title);
                    if t.artist != first.album_artist {
                        haystack.push(' ');
                        haystack.push_str(&t.artist);
                    }
                }
                Arc::new(Album {
                    key,
                    title: first.album.clone(),
                    artist: first.album_artist.clone(),
                    year: tracks.iter().map(|t| t.year).max().unwrap_or(0),
                    duration_ms: tracks.iter().map(|t| t.duration_ms as u64).sum(),
                    haystack: haystack.to_lowercase().into(),
                    tracks,
                })
            })
            .collect();
        albums.sort_by_cached_key(|a| (a.artist.to_lowercase(), a.year, a.title.to_lowercase()));
        Self { albums, track_count, by_path }
    }

    pub fn track(&self, path: &Path) -> Option<&Arc<Track>> {
        self.by_path.get(path)
    }

    /// Every track, for rebuilding after a change.
    pub fn all_tracks(&self) -> Vec<Track> {
        self.albums.iter().flat_map(|a| a.tracks.iter().map(|t| (**t).clone())).collect()
    }

    pub fn album_of(&self, track: &Track) -> Option<&Arc<Album>> {
        let key = track.album_key();
        self.albums.iter().find(|a| a.key == key)
    }

    pub fn find_track(&self, path: &Path) -> Option<(Arc<Album>, usize)> {
        self.albums.iter().find_map(|a| {
            a.tracks.iter().position(|t| t.path == path).map(|i| (a.clone(), i))
        })
    }

    /// True if any tag needs a glyph the bundled Latin fonts don't have.
    pub fn needs_scripts(&self) -> (bool, bool) {
        self.albums.iter().fold((false, false), |acc, a| {
            let n = needs_scripts(&a.haystack);
            (acc.0 || n.0, acc.1 || n.1)
        })
    }
}

/// (CJK, Hangul): whether `text` needs glyphs the Latin fonts don't have.
pub fn needs_scripts(text: &str) -> (bool, bool) {
    let (mut cjk, mut hangul) = (false, false);
    for c in text.chars() {
        match c as u32 {
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul = true,
            0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF => cjk = true,
            _ => {}
        }
    }
    (cjk, hangul)
}

// ---------------------------------------------------------------- scanning

/// Walk `folders`, reusing tags from `known` for files whose mtime is unchanged.
pub fn scan(folders: &[PathBuf], known: Vec<Track>, progress: &AtomicUsize) -> Vec<Track> {
    let mut known: HashMap<PathBuf, Track> = known.into_iter().map(|t| (t.path.clone(), t)).collect();
    let mut out = Vec::with_capacity(known.len());
    for folder in folders {
        for entry in walkdir::WalkDir::new(folder).follow_links(true).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if !AUDIO_EXT.contains(&ext.as_str()) {
                continue;
            }
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            let track = match known.remove(path) {
                Some(t) if t.mtime == mtime => t,
                _ => read_track(path, mtime),
            };
            out.push(track);
            progress.fetch_add(1, Ordering::Relaxed);
        }
    }
    out
}

pub fn read_track(path: &Path, mtime: u64) -> Track {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Unsorted".into());

    let mut t = Track {
        path: path.to_owned(),
        title: stem.into(),
        artist: "Unknown Artist".into(),
        album: folder.into(),
        album_artist: "".into(),
        track_no: 0,
        disc_no: 0,
        year: 0,
        duration_ms: 0,
        mtime,
    };

    if let Ok(file) = lofty::read_from_path(path) {
        t.duration_ms = file.properties().duration().as_millis() as u32;
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            let clean = |s: &str| -> Option<Box<str>> {
                let s = s.trim();
                (!s.is_empty()).then(|| s.into())
            };
            if let Some(v) = tag.title().as_deref().and_then(clean) {
                t.title = v;
            }
            if let Some(v) = tag.artist().as_deref().and_then(clean) {
                t.artist = v;
            }
            if let Some(v) = tag.album().as_deref().and_then(clean) {
                t.album = v;
            }
            if let Some(v) = tag.get_string(ItemKey::AlbumArtist).and_then(clean) {
                t.album_artist = v;
            }
            t.track_no = tag.track().unwrap_or(0) as u16;
            t.disc_no = tag.disk().unwrap_or(0) as u16;
            t.year = tag.date().map_or(0, |d| d.year);
        }
    }
    if t.album_artist.is_empty() {
        t.album_artist = t.artist.clone();
    }
    t
}

// ------------------------------------------------------------- persistence

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub folders: Vec<PathBuf>,
    pub volume: f32,
    pub shuffle: bool,
    pub repeat: u8,
    pub last_track: Option<PathBuf>,
    pub last_pos_ms: u64,
    /// Streamed songs kept in the collection: YouTube id → saved file.
    pub saved: std::collections::BTreeMap<String, PathBuf>,
    /// Window size before switching to the mini player.
    pub full_size: Option<(f32, f32)>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            volume: 0.8,
            shuffle: false,
            repeat: 1,
            last_track: None,
            last_pos_ms: 0,
            saved: Default::default(),
            full_size: None,
        }
    }
}

/// Where config and caches live. `WAX_DATA_DIR` overrides the default
/// (`%LOCALAPPDATA%\wax`) for portable installs and testing.
pub fn data_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::var_os("WAX_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("wax"));
        for sub in ["art", "wave"] {
            let _ = std::fs::create_dir_all(dir.join(sub));
        }
        dir
    })
    .clone()
}

impl Config {
    pub fn load() -> Self {
        std::fs::read(data_dir().join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(data_dir().join("config.json"), json);
        }
    }
}

pub fn load_cached_tracks() -> Vec<Track> {
    std::fs::read(data_dir().join("library.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_cached_tracks(tracks: &[Track]) {
    if let Ok(json) = serde_json::to_vec(tracks) {
        let _ = std::fs::write(data_dir().join("library.json"), json);
    }
}

// ------------------------------------------------------------------- hash

/// FNV-1a: stable across Rust versions, so cache file names stay valid.
pub struct Fnv(u64);

impl Fnv {
    pub fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    pub fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    pub fn finish(&self) -> u64 {
        self.0
    }
}

pub fn file_mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

pub fn fmt_time(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}
