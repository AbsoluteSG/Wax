//! Keeping a streamed song: its audio (already in memory from streaming)
//! is written to `Music\Wax\<Artist>\<Title>.m4a` with tags and cover art,
//! and it becomes a normal track in the collection.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::Tag;

use crate::library::{self, Track};
use crate::stream::{self, Ready};

/// `Music\Wax`, or `WAX_SAVE_DIR` when set.
pub fn save_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("WAX_SAVE_DIR") {
        return PathBuf::from(dir);
    }
    dirs::audio_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Music")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Wax")
}

/// Blocking: waits for the download if needed. Returns the new local track.
pub fn save(track: &Track) -> Result<Track, String> {
    let id = track.stream_id().ok_or("only streamed songs need saving")?;
    let buf = stream::fetch(id);
    let started = Instant::now();
    loop {
        match buf.status() {
            Ready::Done => break,
            Ready::Failed(e) => return Err(e),
            Ready::Partial if started.elapsed() > Duration::from_secs(180) => return Err("download timed out".into()),
            Ready::Partial => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    let mut bytes = Vec::with_capacity(buf.len());
    buf.reader().read_to_end(&mut bytes).map_err(|e| e.to_string())?;

    let dir = save_dir().join(clean(&track.artist, "Unknown Artist"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = free_name(&dir, &clean(&track.title, id));
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;

    // Tags make it a proper part of the collection; the audio is fine without them.
    let _ = write_tags(&path, track, id);

    let mut saved = library::read_track(&path, library::file_mtime(&path));
    if saved.duration_ms == 0 {
        saved.duration_ms = track.duration_ms;
    }
    Ok(saved)
}

fn write_tags(path: &Path, track: &Track, id: &str) -> Result<(), String> {
    let mut file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    if file.primary_tag_mut().is_none() {
        file.insert_tag(Tag::new(file.primary_tag_type()));
    }
    let tag = file.primary_tag_mut().ok_or("no tag")?;
    tag.set_title(track.title.to_string());
    tag.set_artist(track.artist.to_string());
    // A single is a record of its own.
    tag.set_album(track.title.to_string());
    tag.insert_text(ItemKey::AlbumArtist, track.artist.to_string());
    tag.insert_text(ItemKey::Comment, format!("https://www.youtube.com/watch?v={id}"));
    if let Some(img) = stream::thumbnail(id) {
        let img = img.resize_to_fill(600, 600, image::imageops::FilterType::Triangle).to_rgb8();
        let mut jpeg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88);
        if img.write_with_encoder(encoder).is_ok() {
            tag.push_picture(Picture::unchecked(jpeg).pic_type(PictureType::CoverFront).mime_type(MimeType::Jpeg).build());
        }
    }
    file.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())
}

/// A file-system-safe name.
fn clean(s: &str, fallback: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { ' ' } else { c })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_end_matches(['.', ' ']).chars().take(100).collect::<String>();
    if s.is_empty() { fallback.to_owned() } else { s }
}

fn free_name(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.m4a"));
    if !first.exists() {
        return first;
    }
    (2..).map(|n| dir.join(format!("{stem} ({n}).m4a"))).find(|p| !p.exists()).unwrap()
}

/// Run `save` on a worker thread.
pub fn spawn(track: Arc<Track>, wake: impl Fn() + Send + 'static) -> std::sync::mpsc::Receiver<(Arc<Track>, Result<Track, String>)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = save(&track);
        let _ = tx.send((track, result));
        wake();
    });
    rx
}
