//! The settings window: a borderless panel painted in the same palette as the
//! strip, opened centred on whichever monitor the user called it from.

use crate::app::App;
use crate::config::{ActiveMode, HeaderMode};
use crate::providers::Family;
use crate::shortcuts;

use eframe::egui;
use egui::{Color32, RichText, Rounding, Sense, Stroke, Vec2, ViewportCommand};
use std::sync::atomic::Ordering;

const BG: Color32 = Color32::from_rgb(18, 20, 26);
const CARD: Color32 = Color32::from_rgb(30, 33, 42);
const CARD_HI: Color32 = Color32::from_rgb(48, 53, 65);
const ACCENT: Color32 = Color32::from_rgb(110, 210, 146);
const ACCENT_BG: Color32 = Color32::from_rgb(44, 86, 63);
const TEXT: Color32 = Color32::from_rgb(234, 238, 246);
const DIM: Color32 = Color32::from_rgb(176, 184, 200);
const HINT: Color32 = Color32::from_rgb(158, 167, 184);
const WARN: Color32 = Color32::from_rgb(238, 162, 92);

const WIN_W: f32 = 430.0;
const WIN_TITLE: &str = "Quotty — настройки";
const AUTHOR_URL: &str = "https://t.me/nova_txt";

/// Dark palette shared with the strip. Written into *both* theme slots and the
/// theme pinned to dark: egui keeps a style per theme and switches to the
/// system one as soon as winit reports it, which would otherwise drop this.
pub fn apply_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let mut v = egui::Visuals::dark();

    v.panel_fill = BG;
    v.window_fill = BG;
    v.extreme_bg_color = Color32::from_rgb(46, 51, 62); // slider rail
    v.faint_bg_color = CARD;
    // Every widget's text goes through this, so nothing inherits egui's dim greys.
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.window_rounding = Rounding::same(12.0);
    v.window_stroke = Stroke::NONE;
    v.popup_shadow = egui::epaint::Shadow::NONE;
    v.window_shadow = egui::epaint::Shadow::NONE;

    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = Rounding::same(7.0);
        w.bg_stroke = Stroke::NONE;
        // Checkmarks and slider handles, not label text.
        w.fg_stroke = Stroke::new(1.8, ACCENT);
    }
    v.widgets.noninteractive.rounding = Rounding::same(7.0);
    v.widgets.noninteractive.bg_stroke = Stroke::NONE;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.bg_fill = CARD_HI;
    v.widgets.inactive.weak_bg_fill = CARD_HI;
    v.widgets.hovered.bg_fill = Color32::from_rgb(60, 66, 80);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(60, 66, 80);
    v.widgets.active.bg_fill = Color32::from_rgb(70, 77, 92);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(70, 77, 92);
    v.selection.bg_fill = ACCENT_BG;
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    style.visuals = v;
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 5.0);
    style.spacing.interact_size.y = 22.0;

    let style: std::sync::Arc<egui::Style> = style.into();
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
}

impl App {
    pub(crate) fn render_settings(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut close = false;
        let mut refresh_now = false;
        let mut check_update = false;
        let mut content_h = self.settings_h;

        let vid = egui::ViewportId::from_hash_of("quotty-settings");
        let builder = egui::ViewportBuilder::default()
            .with_title(WIN_TITLE)
            .with_inner_size([WIN_W, self.settings_h])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false);

        ctx.show_viewport_immediate(vid, builder, |ctx, _class| {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::none()
                        .fill(BG)
                        .rounding(Rounding::same(12.0))
                        .inner_margin(egui::Margin::symmetric(14.0, 12.0)),
                )
                .show(ctx, |ui| {
                    close |= self.title_bar(ui, ctx);
                    self.appearance_card(ui);
                    self.sources_card(ui);
                    refresh_now |= self.polling_card(ui);
                    self.system_card(ui);
                    check_update |= self.version_card(ui);

                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Закрыть").clicked() {
                            close = true;
                        }
                    });
                    // Fit the window to its content instead of guessing a height.
                    content_h = ui.min_rect().height() + 24.0;
                });

            if ctx.input(|i| i.viewport().close_requested())
                || ctx.input(|i| i.key_pressed(egui::Key::Escape))
            {
                close = true;
            }
        });

        if (content_h - self.settings_h).abs() > 1.0 {
            self.settings_h = content_h;
            // Re-centre once the final size is known.
            self.settings_center = true;
        } else if self.settings_center && center_window(WIN_TITLE, self.settings_area) {
            self.settings_center = false;
        }

        if refresh_now {
            self.shared.refresh.store(true, Ordering::Relaxed);
        }
        if check_update {
            self.shared.update_now.store(true, Ordering::Relaxed);
        }
        if close {
            self.show_settings = false;
            self.settings_center = false;
            self.settings.save();
        }
        self.shared
            .interval
            .store(self.settings.poll_secs, Ordering::Relaxed);
        self.shared
            .enabled
            .store(self.settings.enabled_mask(), Ordering::Relaxed);
    }

    /// Custom chrome: drag anywhere on the bar, ✕ closes. Returns "close".
    fn title_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        let mut close = false;
        let bar =
            egui::Rect::from_min_size(ui.max_rect().min, Vec2::new(ui.max_rect().width(), 24.0));
        let drag = ui.interact(bar, ui.id().with("settings-drag"), Sense::click_and_drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        ui.horizontal(|ui| {
            ui.label(RichText::new("Quotty").size(14.5).strong().color(TEXT));
            ui.label(RichText::new("· настройки").size(11.5).color(DIM));
            // Author credit sits on the same line, just left of the ✕.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                close = close_button(ui);
                ui.spacing_mut().item_spacing.x = 5.0;
                ui.hyperlink_to(
                    RichText::new("t.me/nova_txt").size(11.0).color(ACCENT),
                    AUTHOR_URL,
                );
                ui.label(RichText::new("Brent ©  |").size(11.0).color(DIM));
            });
        });
        close
    }

    fn appearance_card(&mut self, ui: &mut egui::Ui) {
        card(ui, "ВНЕШНИЙ ВИД", |ui| {
            let s = &mut self.settings;
            value_row(ui, "Непрозрачность", &format!("{:.0}%", s.opacity * 100.0));
            if full_width_slider(ui, &mut s.opacity, 0.2..=1.0) {
                s.save();
            }

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                for (label, val) in [
                    ("100%", 1.0f32),
                    ("80%", 0.8),
                    ("66%", 0.66),
                    ("50%", 0.5),
                    ("33%", 0.33),
                    ("20%", 0.2),
                ] {
                    let on = (s.opacity - val).abs() < 0.005;
                    if ui.selectable_label(on, label).clicked() {
                        s.opacity = val;
                        s.save();
                    }
                }
            });

            ui.add_space(2.0);
            if ui.checkbox(&mut s.animate, "Анимация пузырьков").changed() {
                s.save();
            }

            ui.add_space(4.0);
            caption(ui, "Заголовок строки");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let mut pick = |ui: &mut egui::Ui, mode: HeaderMode, label: &str| {
                    if ui.selectable_label(s.header_mode == mode, label).clicked() {
                        s.header_mode = mode;
                        s.save();
                    }
                };
                pick(ui, HeaderMode::Full, "Среда и тариф");
                pick(ui, HeaderMode::FamilyOnly, "Только семейство");
                pick(ui, HeaderMode::Hidden, "Скрыть");
            });
        });
    }

    fn sources_card(&mut self, ui: &mut egui::Ui) {
        // Read each family's live state first: the card doubles as the place to
        // see whether a source is actually being picked up.
        let status: Vec<(bool, bool, Option<String>)> = {
            let st = self.shared.states.lock().unwrap();
            Family::ALL
                .iter()
                .map(|f| {
                    let s = &st[f.idx()];
                    (s.online, s.ever, s.error.clone())
                })
                .collect()
        };

        card(ui, "ИСТОЧНИКИ", |ui| {
            for f in Family::ALL {
                let (online, ever, err) = &status[f.idx()];
                ui.horizontal(|ui| {
                    let mut on = self.settings.enabled(f);
                    if ui.checkbox(&mut on, f.name()).changed() {
                        self.settings.set_enabled(f, on);
                        self.settings.save();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (text, col) = if !self.settings.enabled(f) {
                            ("выключен".to_string(), HINT)
                        } else if *online {
                            ("данные получены".to_string(), ACCENT)
                        } else if let Some(e) = err {
                            (short(e), WARN)
                        } else if *ever {
                            ("нет связи".to_string(), WARN)
                        } else {
                            ("опрос…".to_string(), HINT)
                        };
                        ui.label(RichText::new(text).size(11.0).color(col));
                    });
                });
            }

            ui.add_space(4.0);
            caption(ui, "Что показывать на строке");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                let auto = self.settings.active_mode == ActiveMode::Auto;
                if ui.selectable_label(auto, "Активный").clicked() {
                    self.settings.active_mode = ActiveMode::Auto;
                    self.settings.save();
                }
                for f in Family::ALL {
                    if !self.settings.enabled(f) {
                        continue;
                    }
                    let on = self.settings.active_mode == ActiveMode::Pinned
                        && self.settings.family == f;
                    if ui.selectable_label(on, f.name()).clicked() {
                        self.settings.active_mode = ActiveMode::Pinned;
                        self.settings.family = f;
                        self.active = f;
                        self.settings.save();
                    }
                }
            });
            ui.label(
                RichText::new(
                    "«Активный» — квота того инструмента, окно которого было\n\
                     на переднем плане последним (приложение, IDE или CLI).",
                )
                .size(10.5)
                .color(HINT),
            );
        });
    }

    /// Returns true when "обновить сейчас" was pressed.
    fn polling_card(&mut self, ui: &mut egui::Ui) -> bool {
        let mut refresh = false;
        card(ui, "ОПРОС КВОТ", |ui| {
            let s = &mut self.settings;
            value_row(ui, "Интервал опроса", &format!("{} с", s.poll_secs));
            if full_width_slider(ui, &mut s.poll_secs, 15..=600) {
                s.save();
            }
            if ui.button("Обновить сейчас").clicked() {
                refresh = true;
            }
        });
        refresh
    }

    /// Version + the result of the GitHub release check. Returns "check now".
    fn version_card(&mut self, ui: &mut egui::Ui) -> bool {
        let (checked, available, failed) = {
            let st = self.shared.update.lock().unwrap();
            (st.checked, st.available.clone(), st.error.is_some())
        };
        let mut check = false;
        card(ui, "ВЕРСИЯ", |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Quotty {}", crate::update::current()))
                        .size(12.0)
                        .color(TEXT),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (text, col) = match (&available, checked, failed) {
                        (Some(u), _, _) => (format!("доступна {}", u.version), WARN),
                        (None, true, false) => ("актуальная версия".to_string(), ACCENT),
                        (None, true, true) => ("проверка не удалась".to_string(), HINT),
                        _ => ("проверка…".to_string(), HINT),
                    };
                    ui.label(RichText::new(text).size(11.0).color(col));
                });
            });
            ui.horizontal(|ui| {
                if ui.button("Проверить обновления").clicked() {
                    check = true;
                }
                if let Some(u) = &available {
                    ui.hyperlink_to(
                        RichText::new("Открыть страницу релиза")
                            .size(11.5)
                            .color(ACCENT),
                        u.url.clone(),
                    );
                }
            });
            ui.label(
                RichText::new("Проверка раз в 8 часов, только чтение тега релиза на GitHub.")
                    .size(10.5)
                    .color(HINT),
            );
        });
        check
    }

    fn system_card(&mut self, ui: &mut egui::Ui) {
        card(ui, "СИСТЕМА", |ui| {
            let mut a = self.autostart;
            if ui
                .checkbox(&mut a, "Автозапуск при входе (ярлык в Startup)")
                .changed()
                && shortcuts::set_autostart(a).is_ok()
            {
                self.autostart = a;
                if let Some(t) = &self.tray {
                    t.autostart_item.set_checked(a);
                }
            }
            if ui.button("Создать ярлык на рабочем столе").clicked() {
                let _ = shortcuts::force_desktop_shortcut();
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Small pieces
// ---------------------------------------------------------------------------

/// Section: a dim caption over a rounded card.
fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).size(10.5).color(DIM));
    ui.add_space(1.0);
    egui::Frame::none()
        .fill(CARD)
        .rounding(Rounding::same(10.0))
        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(DIM));
}

/// "Label ………… value", the value in the accent colour.
fn value_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.0).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).size(12.0).color(ACCENT));
        });
    });
}

/// A slider that spans the card and shows no value of its own (the row above
/// carries it), so long Russian labels never squeeze it.
fn full_width_slider<T: egui::emath::Numeric>(
    ui: &mut egui::Ui,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
) -> bool {
    ui.spacing_mut().slider_width = ui.available_width() - 6.0;
    ui.add(egui::Slider::new(value, range).show_value(false))
        .changed()
}

/// A hand-drawn ✕ — the bundled font has no reliable glyph for it.
fn close_button(ui: &mut egui::Ui) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    let col = if resp.hovered() {
        Color32::from_rgb(240, 124, 114)
    } else {
        DIM
    };
    if resp.hovered() {
        ui.painter().rect_filled(rect, Rounding::same(6.0), CARD_HI);
    }
    let c = rect.center();
    let r = 4.5;
    let s = Stroke::new(1.6, col);
    ui.painter()
        .line_segment([c + Vec2::new(-r, -r), c + Vec2::new(r, r)], s);
    ui.painter()
        .line_segment([c + Vec2::new(r, -r), c + Vec2::new(-r, r)], s);
    resp.clicked()
}

/// Keep an error readable on one line.
fn short(e: &str) -> String {
    let mut s: String = e.chars().take(30).collect();
    if e.chars().count() > 30 {
        s.push('…');
    }
    s
}

// ---------------------------------------------------------------------------
// Placement
// ---------------------------------------------------------------------------

/// Work area (left, top, right, bottom, in physical pixels) of the monitor the
/// pointer is on — captured when the window is opened, so later mouse movement
/// can't send the window to another screen.
#[cfg(windows)]
pub(crate) fn cursor_work_area() -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt).ok()?;
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            return None;
        }
        let w = info.rcWork;
        Some((w.left, w.top, w.right, w.bottom))
    }
}

/// Work area of the monitor a window sits on.
#[cfg(windows)]
pub(crate) fn window_work_area(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    unsafe {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let monitor = MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST);
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let w = info.rcWork;
        Some((w.left, w.top, w.right, w.bottom))
    }
}

#[cfg(not(windows))]
pub(crate) fn window_work_area(_hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    None
}

/// Centre the settings window in `area`. Done through Win32 on the real window:
/// egui's viewport position is logical and relative to one screen, which lands
/// in the wrong place on a multi-monitor desktop.
#[cfg(windows)]
fn center_window(title: &str, area: Option<(i32, i32, i32, i32)>) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetWindowRect, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE,
    };
    let Some((left, top, right, bottom)) = area else {
        return true; // nothing to aim at — leave the window where it is
    };
    unsafe {
        let Ok(hwnd) = FindWindowW(None, &HSTRING::from(title)) else {
            return false;
        };
        if hwnd.0.is_null() {
            return false;
        }
        let mut win = RECT::default();
        if GetWindowRect(hwnd, &mut win).is_err() {
            return false;
        }
        drop_system_chrome(hwnd);
        let x = left + ((right - left) - (win.right - win.left)) / 2;
        let y = top + ((bottom - top) - (win.bottom - win.top)) / 2;
        SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE).is_ok()
    }
}

/// Windows 11 rounds and outlines every top-level window itself. On a
/// borderless window that already paints its own rounded panel it shows up as a
/// second arc in each corner, so turn both off and let the panel define the shape.
#[cfg(windows)]
fn drop_system_chrome(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE,
        DWMWCP_DONOTROUND,
    };
    const COLOR_NONE: u32 = 0xFFFF_FFFE;
    unsafe {
        let pref = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &COLOR_NONE as *const _ as *const _,
            std::mem::size_of_val(&COLOR_NONE) as u32,
        );
    }
}

#[cfg(not(windows))]
pub(crate) fn cursor_work_area() -> Option<(i32, i32, i32, i32)> {
    None
}

#[cfg(not(windows))]
fn center_window(_title: &str, _area: Option<(i32, i32, i32, i32)>) -> bool {
    true
}
