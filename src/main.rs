// Quotty — a movable, translucent tray strip showing your Claude quota windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod active;
mod app;
mod config;
mod icon;
mod providers;
mod settings_ui;
mod shortcuts;
mod tray;
mod update;
mod winproc;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let settings = config::Settings::load();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([430.0, 100.0])
        // Low enough for the Nano design with a single limit row; the window is
        // not resizable by hand anyway.
        .with_min_inner_size([160.0, 30.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_resizable(false)
        .with_taskbar(false);
    if let Some((x, y)) = settings.pos {
        viewport = viewport.with_position([x, y]);
    }

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "Quotty",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, settings)))),
    )
}
