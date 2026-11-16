#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app;
mod art;
mod engine;
mod library;
mod mini;
mod paint;
mod platform;
mod playlist;
mod save;
mod shelf;
mod stage;
mod stream;
mod wave;

use eframe::egui;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let launch = app::Launch {
        // `--shot <file.png>` renders a few frames muted, saves one, and exits.
        shot: args.iter().position(|a| a == "--shot").and_then(|i| args.get(i + 1)).map(Into::into),
        autoplay: args.iter().any(|a| a == "--autoplay"),
        open_album: args.iter().any(|a| a == "--open-album"),
        // `--search "<song>"` behaves as if it was typed and Enter pressed.
        search: args.iter().position(|a| a == "--search").and_then(|i| args.get(i + 1)).cloned(),
        shot_delay: args
            .iter()
            .position(|a| a == "--shot-delay")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(2.5),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Wax")
            .with_inner_size([1320.0, 820.0])
            .with_min_inner_size([920.0, 600.0])
            .with_decorations(false)
            .with_drag_and_drop(true)
            .with_transparent(true)
            .with_icon(std::sync::Arc::new(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).unwrap_or_default(),
            )),
        ..Default::default()
    };
    eframe::run_native("wax", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, launch)))))
}
