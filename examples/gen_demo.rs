//! Writes a small fake music library (quiet synth tones with tags and covers)
//! for trying the player without real files:
//!
//!     cargo run --example gen_demo -- <output-dir>

use std::f32::consts::TAU;
use std::io::Write;
use std::path::Path;

use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{Tag, TagType};

const RATE: u32 = 22_050;

const ALBUMS: &[(&str, &str, u16, [u8; 3], [u8; 3], &[&str])] = &[
    ("Summer Days", "Marlow Garris", 2019, [40, 160, 170], [230, 70, 60], &["Summer Days", "Señorita Sun", "S.O.S.", "Arcade Nights", "Low Tide"]),
    ("Sound Postproduction", "Chemodan", 2021, [20, 20, 24], [200, 40, 60], &["Before", "After", "Menu", "Portfolio"]),
    ("Paper Lanterns", "Ito & The Quiet", 2016, [240, 180, 60], [200, 80, 120], &["Paper Lanterns", "夜の散歩", "Tidewater", "Glasshouse", "Ember", "Last Train Home"]),
    ("Night Drive", "Violet Motorway", 2023, [90, 60, 200], [20, 30, 90], &["Ignition", "Neon Rain", "Overpass", "Halogen"]),
    ("Fieldnotes", "Hollow Pines", 2012, [70, 140, 80], [200, 190, 120], &["Moss", "Riverbed", "A Long Way Round the Lake Without Stopping", "Kindling"]),
    ("Untitled Sketches", "Hollow Pines", 2014, [0, 0, 0], [0, 0, 0], &["Sketch I", "Sketch II", "Sketch III"]),
    ("Copper", "Ana Lindqvist", 2020, [200, 110, 50], [90, 40, 30], &["Copper", "Verdigris", "Patina", "Smelt", "Alloy"]),
    ("Blue Hour", "The Slow Cinema", 2018, [40, 90, 200], [230, 230, 240], &["Blue Hour", "Matinee", "Intermission", "Credits"]),
];

fn main() {
    let out = std::env::args().nth(1).expect("usage: gen_demo <output-dir>");
    let out = Path::new(&out);
    let mut seed = 0x2545_F491u32;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };

    for (ai, (album, artist, year, a, b, tracks)) in ALBUMS.iter().enumerate() {
        let dir = out.join(artist).join(album);
        std::fs::create_dir_all(&dir).unwrap();
        if *a != [0, 0, 0] {
            cover(&dir.join("cover.jpg"), *a, *b, ai as f32);
        }
        for (ti, title) in tracks.iter().enumerate() {
            let path = dir.join(format!("{:02}.wav", ti + 1));
            let secs = 14.0 + rand() * 22.0;
            tone(&path, secs, 110.0 * (1.0 + rand() * 2.0), rand() * 10.0);

            let mut tag = Tag::new(TagType::Id3v2);
            tag.set_title(title.to_string());
            tag.set_artist(artist.to_string());
            tag.set_album(album.to_string());
            tag.set_track(ti as u32 + 1);
            tag.insert_text(ItemKey::AlbumArtist, artist.to_string());
            tag.insert_text(ItemKey::RecordingDate, year.to_string());
            tag.save_to_path(&path, WriteOptions::default()).unwrap();
        }
    }
    println!("wrote {} albums to {}", ALBUMS.len(), out.display());
}

/// A quiet two-voice tone with a lumpy envelope so the waveform has a shape.
fn tone(path: &Path, secs: f32, freq: f32, shape: f32) {
    let n = (secs * RATE as f32) as u32;
    let mut data = Vec::with_capacity(n as usize * 2);
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        let f = t / secs;
        let env = (f * 3.14).sin().powf(0.6)
            * (0.35 + 0.65 * ((f * (5.0 + shape) + shape).sin() * 0.5 + 0.5))
            * (0.6 + 0.4 * ((t * 2.0).sin() * 0.5 + 0.5).powi(3));
        let s = ((t * freq * TAU).sin() * 0.6 + (t * freq * 1.5 * TAU).sin() * 0.4) * env * 0.08;
        data.extend_from_slice(&((s * i16::MAX as f32) as i16).to_le_bytes());
    }
    let mut f = std::fs::File::create(path).unwrap();
    let len = data.len() as u32;
    f.write_all(b"RIFF").unwrap();
    f.write_all(&(36 + len).to_le_bytes()).unwrap();
    f.write_all(b"WAVEfmt ").unwrap();
    f.write_all(&16u32.to_le_bytes()).unwrap();
    f.write_all(&1u16.to_le_bytes()).unwrap(); // PCM
    f.write_all(&1u16.to_le_bytes()).unwrap(); // mono
    f.write_all(&RATE.to_le_bytes()).unwrap();
    f.write_all(&(RATE * 2).to_le_bytes()).unwrap();
    f.write_all(&2u16.to_le_bytes()).unwrap();
    f.write_all(&16u16.to_le_bytes()).unwrap();
    f.write_all(b"data").unwrap();
    f.write_all(&len.to_le_bytes()).unwrap();
    f.write_all(&data).unwrap();
}

fn cover(path: &Path, a: [u8; 3], b: [u8; 3], seed: f32) {
    let img = image::RgbImage::from_fn(600, 600, |x, y| {
        let (u, v) = (x as f32 / 600.0, y as f32 / 600.0);
        let wave = ((v * 6.0 + seed).sin() * 0.08 + (v * 13.0 - seed * 2.0).sin() * 0.03 + u - 0.5) * 5.0;
        let band = (wave.clamp(-2.0, 2.0) + 2.0).floor() / 4.0;
        let d = ((u - 0.68).powi(2) + (v - 0.34).powi(2)).sqrt();
        let k = if d < 0.16 { 1.0 - band * 0.5 } else { band };
        image::Rgb(std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * k) as u8))
    });
    img.save(path).unwrap();
}
