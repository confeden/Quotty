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

#[cfg(windows)]
fn single_instance_guard() -> Option<windows::Win32::Foundation::HANDLE> {
    use windows::core::w;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    unsafe {
        let handle = CreateMutexW(None, true, w!("Local\\QuottySingleInstanceMutex")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return None;
        }
        Some(handle)
    }
}

#[cfg(windows)]
fn is_position_visible(x: f32, y: f32) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONULL};
    unsafe {
        let pt = POINT {
            x: x as i32,
            y: y as i32,
        };
        !MonitorFromPoint(pt, MONITOR_DEFAULTTONULL).0.is_null()
    }
}

#[cfg(not(windows))]
fn is_position_visible(_x: f32, _y: f32) -> bool {
    true
}

fn main() -> eframe::Result<()> {
    #[cfg(windows)]
    let _guard = match single_instance_guard() {
        Some(g) => g,
        None => {
            // Already running: do not spawn a second instance.
            return Ok(());
        }
    };

    let settings = config::Settings::load();

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([430.0, 100.0])
        .with_min_inner_size([180.0, 40.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_taskbar(false);
    if let Some((x, y)) = settings.pos {
        if is_position_visible(x, y) {
            viewport = viewport.with_position([x, y]);
        }
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
