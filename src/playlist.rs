//! Playlists: ordered lists of songs from the collection and/or YouTube.
//! Entries carry enough to show and play themselves even if the file moved
//! or the song was never saved.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::library::{data_dir, Library, Track};

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub title: Box<str>,
    pub artist: Box<str>,
    #[serde(default)]
    pub duration_ms: u32,
}

impl Entry {
    pub fn of(t: &Track) -> Self {
        Self { path: t.path.clone(), title: t.title.clone(), artist: t.artist.clone(), duration_ms: t.duration_ms }
    }

    /// The playable track: the collection's copy when there is one.
    pub fn resolve(&self, lib: &Library) -> Arc<Track> {
        if let Some(t) = lib.track(&self.path) {
            return t.clone();
        }
        Arc::new(Track {
            path: self.path.clone(),
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: "".into(),
            album_artist: self.artist.clone(),
            track_no: 0,
            disc_no: 0,
            year: 0,
            duration_ms: self.duration_ms,
            mtime: 0,
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Playlist {
    pub id: u64,
    pub name: String,
    pub entries: Vec<Entry>,
}

impl Playlist {
    pub fn duration_ms(&self) -> u64 {
        self.entries.iter().map(|e| e.duration_ms as u64).sum()
    }

    pub fn tracks(&self, lib: &Library) -> Vec<Arc<Track>> {
        self.entries.iter().map(|e| e.resolve(lib)).collect()
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Playlists {
    pub lists: Vec<Playlist>,
    #[serde(default)]
    next_id: u64,
}

impl Playlists {
    pub fn load() -> Self {
        std::fs::read(data_dir().join("playlists.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            // Write then rename, so a crash can't leave a half-written file.
            let dir = data_dir();
            let tmp = dir.join("playlists.json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, dir.join("playlists.json"));
            }
        }
    }

    pub fn get(&self, id: u64) -> Option<&Playlist> {
        self.lists.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Playlist> {
        self.lists.iter_mut().find(|p| p.id == id)
    }

    pub fn create(&mut self) -> u64 {
        self.next_id = self.next_id.max(self.lists.iter().map(|p| p.id).max().unwrap_or(0)) + 1;
        let n = self.lists.len() + 1;
        let mut name = format!("Playlist {n}");
        let mut k = n;
        while self.lists.iter().any(|p| p.name == name) {
            k += 1;
            name = format!("Playlist {k}");
        }
        self.lists.push(Playlist { id: self.next_id, name, entries: Vec::new() });
        self.save();
        self.next_id
    }

    /// Append tracks, skipping ones already in the list. Returns how many were added.
    pub fn add(&mut self, id: u64, tracks: &[Arc<Track>]) -> usize {
        let Some(list) = self.get_mut(id) else { return 0 };
        let before = list.entries.len();
        for t in tracks {
            if !list.entries.iter().any(|e| e.path == t.path) {
                list.entries.push(Entry::of(t));
            }
        }
        let added = list.entries.len() - before;
        self.save();
        added
    }

    pub fn remove_entry(&mut self, id: u64, index: usize) {
        if let Some(list) = self.get_mut(id) {
            if index < list.entries.len() {
                list.entries.remove(index);
            }
        }
        self.save();
    }

    pub fn move_entry(&mut self, id: u64, from: usize, to: usize) {
        if let Some(list) = self.get_mut(id) {
            if from < list.entries.len() {
                let e = list.entries.remove(from);
                list.entries.insert(to.min(list.entries.len()), e);
            }
        }
        self.save();
    }

    pub fn rename(&mut self, id: u64, name: &str) {
        let name = name.trim();
        if let (Some(list), false) = (self.get_mut(id), name.is_empty()) {
            list.name = name.to_owned();
        }
        self.save();
    }

    pub fn delete(&mut self, id: u64) {
        self.lists.retain(|p| p.id != id);
        self.save();
    }

    /// A saved file replaces its streamed entry everywhere.
    pub fn relink(&mut self, from: &PathBuf, to: &Track) {
        let mut changed = false;
        for list in &mut self.lists {
            for e in &mut list.entries {
                if &e.path == from {
                    *e = Entry::of(to);
                    changed = true;
                }
            }
        }
        if changed {
            self.save();
        }
    }
}
