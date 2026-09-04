//! The Quotty window (a compact, movable, translucent strip) plus tray wiring.

use crate::active;
use crate::config::{ActiveMode, HeaderMode, Settings};
use crate::providers::{self, Family, Snapshot};
use crate::shortcuts;
use crate::tray::Tray;
use crate::update::{self, UpdateState};

use chrono::{DateTime, Duration, Local, Utc};
use eframe::egui;
use egui::{Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Vec2, ViewportCommand};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Latest fetch state of one family. `last` keeps the most recent *successful*
/// snapshot so we can keep drawing structure (titles, reset times) while offline.
#[derive(Default)]
pub struct FetchState {
    pub last: Option<Snapshot>,
    pub online: bool,
    pub ever: bool,
    pub error: Option<String>,
    /// The last poll failed only because the service is throttling us. The
    /// numbers we already have are still true, so they stay on screen.
    pub rate_limited: bool,
}

/// What the strip needs to know about the family it is drawing.
struct ActiveState {
    online: bool,
    /// Values on screen are the last good ones; the service is throttling us.
    stale: bool,
    ever: bool,
    last: Option<Snapshot>,
    error: Option<String>,
}

/// `want`/`enabled` sentinel: no family singled out for an immediate poll.
const NO_FAMILY: u8 = 0xFF;

pub struct Shared {
    /// One state per family, indexed by `Family::idx`.
    pub states: Mutex<Vec<FetchState>>,
    pub refresh: AtomicBool,
    pub interval: AtomicU64,
    /// Bitmask of families the poller should keep fresh.
    pub enabled: AtomicU8,
    /// Family to poll right now (set when the user switches tools).
    pub want: AtomicU8,
    /// Result of the last GitHub release check.
    pub update: Mutex<UpdateState>,
    /// Set to run an update check without waiting for the 8-hour timer.
    pub update_now: AtomicBool,
}

#[derive(Clone, Debug)]
pub struct ResetTime {
    #[allow(dead_code)]
    pub abs: String,
    pub row: String,
    pub badge: String,
}

pub struct App {
    pub(crate) settings: Settings,
    pub(crate) shared: Arc<Shared>,
    pub(crate) tray: Option<Tray>,
    pub(crate) show_settings: bool,
    /// Set on open: the settings window still has to be moved to the monitor
    /// the user called it from.
    pub(crate) settings_center: bool,
    /// Work area of that monitor, captured when the window was opened.
    pub(crate) settings_area: Option<(i32, i32, i32, i32)>,
    /// Height the settings window is currently sized to (fitted to content).
    pub(crate) settings_h: f32,
    pub(crate) autostart: bool,
    /// Family currently on screen.
    pub(crate) active: Family,
    detector: active::Detector,
    is_ai_active: bool,
    is_visible: bool,
    /// Height we last asked the OS for, so we only resize when it changes.
    applied_h: f32,
    /// Cached reset strings, refreshed at most once per second.
    reset_cache: Vec<Option<ResetTime>>,
    reset_cache_sec: i64,
    /// Throttle for persisting the auto-switched family.
    last_family_save: f64,
    /// Tray menu events, delivered via a handler that also wakes the UI so
    /// changes are applied immediately, not on next hover.
    menu_rx: std::sync::mpsc::Receiver<tray_icon::menu::MenuEvent>,
    /// Last time (s) we re-asserted always-on-top so the taskbar can't cover us.
    last_topmost: f64,
    /// Native window handle, resolved lazily from `eframe::Frame`.
    hwnd: Option<isize>,
    /// Period the Win32 backstop timer is currently armed at.
    timer_period: u32,
    /// Tray hover text we last set, so we only touch the icon on a change.
    tooltip: String,
    /// A drag we started ourselves is in progress; only then is the window's
    /// new position worth persisting.
    dragging: bool,
}

/// Pull the Win32 HWND out of eframe's frame (None on other platforms).
fn native_hwnd(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// Put the window back at the top of the topmost band, without stealing focus.
#[cfg(windows)]
fn force_topmost(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
    };
    unsafe {
        let _ = SetWindowPos(
            HWND(hwnd as *mut _),
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

#[cfg(not(windows))]
fn force_topmost(_hwnd: isize) {}

pub(crate) static NATIVE_HWND: AtomicIsize = AtomicIsize::new(0);

/// Show or hide the native window without stealing focus.
#[cfg(windows)]
pub(crate) fn set_native_visible(hwnd: isize, visible: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNA};
    unsafe {
        let _ = ShowWindow(
            HWND(hwnd as *mut _),
            if visible { SW_SHOWNA } else { SW_HIDE },
        );
    }
}

#[cfg(not(windows))]
pub(crate) fn set_native_visible(_hwnd: isize, _visible: bool) {}

#[cfg(windows)]
fn show_context_menu(hwnd: isize, compact_mode: bool) -> Option<usize> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, SetForegroundWindow,
        TrackPopupMenu, MF_STRING, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_TOPALIGN,
    };

    unsafe {
        let hmenu = CreatePopupMenu().ok()?;
        let compact_text = if compact_mode {
            "Обычный режим (с полосами)"
        } else {
            "Компактный режим (без полос)"
        };
        let _ = AppendMenuW(hmenu, MF_STRING, 1, &HSTRING::from(compact_text));
        let _ = AppendMenuW(hmenu, MF_STRING, 2, &HSTRING::from("Настройки…"));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(HWND(hwnd as *mut _));
        let cmd = TrackPopupMenu(
            hmenu,
            TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN,
            pt.x,
            pt.y,
            0,
            HWND(hwnd as *mut _),
            None,
        );
        let _ = DestroyMenu(hmenu);
        if cmd.0 > 0 {
            Some(cmd.0 as usize)
        } else {
            None
        }
    }
}

#[cfg(not(windows))]
fn show_context_menu(_hwnd: isize, _compact_mode: bool) -> Option<usize> {
    None
}

// ---------------------------------------------------------------------------
// Repaint backstop
//
// While a modal Win32 loop is running — the tray's own right-click menu is one —
// winit's event loop is not, so the wake-up it scheduled for our next animation
// frame never fires and the strip freezes until some input reaches it again.
// A window timer keeps working inside those loops: WM_TIMER is dispatched by the
// modal pump and, with a TIMERPROC, needs no window procedure of our own. It
// invalidates the window (→ WM_PAINT → winit's RedrawRequested → a repaint) but
// only when egui has *not* painted within the last period, so in normal
// operation this costs nothing.
// ---------------------------------------------------------------------------

const REPAINT_TIMER_ID: usize = 0x9101;
static START: OnceLock<std::time::Instant> = OnceLock::new();
static LAST_PAINT_MS: AtomicU64 = AtomicU64::new(0);
static TIMER_GRACE_MS: AtomicU64 = AtomicU64::new(500);

fn uptime_ms() -> u64 {
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}

#[cfg(windows)]
unsafe extern "system" fn repaint_tick(
    hwnd: windows::Win32::Foundation::HWND,
    _msg: u32,
    _id: usize,
    _time: u32,
) {
    let since = uptime_ms().saturating_sub(LAST_PAINT_MS.load(Ordering::Relaxed));
    if since >= TIMER_GRACE_MS.load(Ordering::Relaxed) {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, false);
    }
}

#[cfg(windows)]
fn arm_repaint_timer(hwnd: isize, period_ms: u32) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::SetTimer;
    TIMER_GRACE_MS.store(period_ms as u64, Ordering::Relaxed);
    unsafe {
        SetTimer(
            HWND(hwnd as *mut _),
            REPAINT_TIMER_ID,
            period_ms,
            Some(repaint_tick),
        );
    }
}

#[cfg(not(windows))]
fn arm_repaint_timer(_hwnd: isize, _period_ms: u32) {}

// ---------------------------------------------------------------------------

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, settings: Settings) -> Self {
        let _ = shortcuts::ensure_desktop_shortcut();
        crate::settings_ui::apply_style(&cc.egui_ctx);
        providers::set_diagnostics(settings.diagnostics);
        providers::diag(&format!(
            "--- quotty {} started, diagnostics on ---",
            update::current()
        ));

        let shared = Arc::new(Shared {
            states: Mutex::new(
                (0..Family::ALL.len())
                    .map(|_| FetchState::default())
                    .collect(),
            ),
            refresh: AtomicBool::new(false),
            interval: AtomicU64::new(settings.poll_secs),
            enabled: AtomicU8::new(settings.enabled_mask()),
            want: AtomicU8::new(NO_FAMILY),
            update: Mutex::new(UpdateState::default()),
            update_now: AtomicBool::new(false),
        });
        spawn_poller(shared.clone(), cc.egui_ctx.clone());
        spawn_update_checker(shared.clone(), cc.egui_ctx.clone());

        let autostart = shortcuts::is_autostart_enabled();
        let tray = Tray::new(autostart).ok();

        // Route tray menu events through our own channel and wake the UI on each
        // one, so a menu choice is applied immediately instead of on the next
        // timer tick / mouse hover.
        let (menu_tx, menu_rx) = std::sync::mpsc::channel();
        let wake = cc.egui_ctx.clone();
        tray_icon::menu::MenuEvent::set_event_handler(Some(move |ev| {
            let _ = menu_tx.send(ev);
            wake.request_repaint();
        }));

        active::spawn_watcher(cc.egui_ctx.clone());

        Self {
            active: settings.family,
            settings,
            shared,
            tray,
            show_settings: false,
            settings_center: false,
            settings_area: None,
            settings_h: 640.0,
            autostart,
            detector: active::Detector::default(),
            is_ai_active: true,
            is_visible: true,
            applied_h: 0.0,
            reset_cache: Vec::new(),
            reset_cache_sec: 0,
            last_family_save: f64::MIN,
            menu_rx,
            last_topmost: 0.0,
            hwnd: None,
            timer_period: 0,
            tooltip: String::new(),
            dragging: false,
        }
    }

    /// `from_strip`: opened by right-clicking the strip, so the strip's own
    /// monitor is the one the user is looking at. From the tray menu we only
    /// have the pointer to go by.
    pub(crate) fn open_settings(&mut self, from_strip: bool) {
        self.show_settings = true;
        self.settings_center = true;
        self.settings_area = from_strip
            .then(|| self.hwnd.and_then(crate::settings_ui::window_work_area))
            .flatten()
            .or_else(crate::settings_ui::cursor_work_area);
    }

    fn handle_tray_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.menu_rx.try_recv() {
            let id = ev.id;
            let Some(tray) = &self.tray else { continue };

            if id == tray.id_quit {
                ctx.send_viewport_cmd(ViewportCommand::Close);
            } else if id == tray.id_refresh {
                self.shared.refresh.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            } else if id == tray.id_settings {
                self.open_settings(false);
            } else if id == tray.id_autostart {
                let now_checked = tray.autostart_item.is_checked();
                match shortcuts::set_autostart(now_checked) {
                    Ok(_) => self.autostart = now_checked,
                    Err(_) => tray.autostart_item.set_checked(!now_checked),
                }
            }
        }
    }

    /// Follow the foreground window (or the pinned choice) and, on a switch,
    /// ask the poller to refresh that family right away.
    fn update_active(&mut self, t: f64) {
        let active_status = self.detector.poll(t);
        self.is_ai_active = active_status.is_ai;

        let target = match self.settings.active_mode {
            ActiveMode::Pinned => self.settings.family,
            ActiveMode::Auto => match active_status.family {
                Some(f) if self.settings.enabled(f) => f,
                _ => self.active,
            },
        };
        if target == self.active {
            return;
        }
        self.active = target;
        self.settings.family = target;
        self.reset_cache.clear();
        self.shared
            .want
            .store(target.idx() as u8, Ordering::Relaxed);
        // Persisted so the next launch starts on the tool you were using — but
        // not on every alt-tab.
        if t - self.last_family_save > 30.0 {
            self.last_family_save = t;
            self.settings.save();
        }
    }

    fn draw_strip(&mut self, ui: &mut egui::Ui, anim_t: f64, animate: bool) {
        let op = self.settings.opacity;
        let full = ui.max_rect();
        let painter = ui.painter().clone();

        let text_a = ((0.35 + 0.65 * op) * 255.0) as u8;
        let dim = Color32::from_rgba_unmultiplied(190, 196, 210, text_a);
        let strong = Color32::from_rgba_unmultiplied(232, 236, 245, text_a);

        let left = full.left() + 12.0;
        let right = full.right() - 12.0;
        let mut y = full.top() + 8.0;

        let now = Utc::now();
        let ActiveState {
            online,
            stale,
            ever,
            last,
            error: err,
        } = self.active_state();
        // Both states draw real numbers; only the status word differs.
        let show_values = online || stale;

        // Partition limits into active and exhausted
        let all_limits = last.as_ref().map(|s| s.limits.as_slice()).unwrap_or(&[]);
        let (active_indices, exhausted_indices): (Vec<usize>, Vec<usize>) = (0..all_limits.len())
            .partition(|&i| all_limits[i].used_percent < LIMIT_PCT);
        let all_exhausted = active_indices.is_empty();

        let (visible_indices, hidden_exhausted): (Vec<usize>, Vec<usize>) = match self.settings.exhausted_mode {
            crate::config::ExhaustedMode::Full | crate::config::ExhaustedMode::Compact => {
                ((0..all_limits.len()).collect(), Vec::new())
            }
            crate::config::ExhaustedMode::Hidden => {
                if all_exhausted {
                    // All exhausted: keep showing so the window is never empty
                    ((0..all_limits.len()).collect(), Vec::new())
                } else {
                    (active_indices, exhausted_indices)
                }
            }
        };

        // Determine content height so the background never cuts off content
        let mut rows_h = 0.0f32;
        if show_values && !visible_indices.is_empty() {
            for &i in &visible_indices {
                let lim = &all_limits[i];
                let is_compact = self.settings.compact_mode
                    || (lim.used_percent >= LIMIT_PCT
                        && (self.settings.exhausted_mode == crate::config::ExhaustedMode::Compact
                            || (self.settings.exhausted_mode == crate::config::ExhaustedMode::Hidden && all_exhausted)));
                rows_h += if is_compact { 18.0 } else { 34.0 };
            }
        } else {
            rows_h = 18.0;
        }
        let content_h = 18.0 + 17.0 + rows_h; // pad_h + header_h + rows_h
        let bg_h = content_h.max(self.applied_h);
        let strip_rect = Rect::from_min_size(full.min, Vec2::new(full.width(), bg_h));
        painter.rect_filled(
            strip_rect,
            egui::Rounding::same(8.0),
            Color32::from_rgba_unmultiplied(22, 24, 30, (op * 235.0) as u8),
        );

        // Cache reset times
        let sec = now.timestamp();
        if self.reset_cache_sec != sec || self.reset_cache.len() != all_limits.len() {
            self.reset_cache = all_limits
                .iter()
                .map(|l| l.window.map(|w| fmt_reset(w.resets_at, now)))
                .collect();
            self.reset_cache_sec = sec;
        }

        // Check if there is an exhausted limit hidden from the list that needs a header badge
        let hidden_badge = if !hidden_exhausted.is_empty() {
            hidden_exhausted
                .iter()
                .filter_map(|&i| {
                    let lim = &all_limits[i];
                    let reset_time = self.reset_cache.get(i).and_then(|r| r.as_ref())?;
                    let name = lim.title.split_whitespace().next().unwrap_or(&lim.title).to_string();
                    let resets_at = lim.window?.resets_at;
                    Some((name, reset_time.badge.clone(), resets_at))
                })
                .min_by_key(|(_, _, t)| *t)
                .map(|(name, badge, _)| (name, badge))
        } else {
            None
        };

        // Header: environment/plan (left) + optional hidden badge + online/offline status (right).
        let header = match self.settings.header_mode {
            HeaderMode::Hidden => String::new(),
            HeaderMode::FamilyOnly => self.active.name().to_string(),
            HeaderMode::Full => last
                .as_ref()
                .map(|s| s.plan.clone())
                .unwrap_or_else(|| self.active.name().to_string()),
        };
        if !header.is_empty() {
            painter.text(
                Pos2::new(left, y),
                Align2::LEFT_TOP,
                header,
                FontId::proportional(11.5),
                strong,
            );
        }
        let (status, status_col, dot) = if !ever && !online {
            ("загрузка…", dim, false)
        } else if online {
            (
                "онлайн",
                Color32::from_rgba_unmultiplied(120, 205, 150, text_a),
                true,
            )
        } else if stale {
            (
                "подключение",
                Color32::from_rgba_unmultiplied(214, 200, 110, text_a),
                true,
            )
        } else {
            (
                "оффлайн",
                Color32::from_rgba_unmultiplied(232, 150, 80, text_a),
                true,
            )
        };
        let status_rect = painter.text(
            Pos2::new(right, y),
            Align2::RIGHT_TOP,
            status,
            FontId::proportional(10.5),
            status_col,
        );
        if dot {
            // While throttled the dot breathes, so "подключение" reads as
            // something still trying rather than something stuck.
            let col = if stale {
                let pulse = 0.45 + 0.55 * (0.5 + 0.5 * (anim_t * 2.2).sin() as f32);
                status_col.gamma_multiply(pulse)
            } else {
                status_col
            };
            painter.circle_filled(
                Pos2::new(status_rect.left() - 6.0, status_rect.center().y),
                3.0,
                col,
            );
        }

        // Draw hidden quota reset badge in header next to network status
        if let Some((h_name, h_badge)) = hidden_badge {
            let badge_font = FontId::proportional(10.0);
            let prefix = format!("{h_name} сброс: ");
            let prefix_g = painter.layout_no_wrap(prefix, badge_font.clone(), dim);
            let badge_g = painter.layout_no_wrap(
                h_badge,
                badge_font,
                Color32::from_rgba_unmultiplied(214, 150, 74, text_a),
            );
            let pill_pad = 5.0;
            let prefix_w = prefix_g.size().x;
            let pill_w = prefix_w + badge_g.size().x + pill_pad * 2.0;
            let pill_h = 16.0;
            let status_left = if dot { status_rect.left() - 12.0 } else { status_rect.left() - 4.0 };
            let pill_right = status_left - 6.0;
            let pill_left = pill_right - pill_w;
            let pill_rect = Rect::from_min_size(Pos2::new(pill_left, y - 1.0), Vec2::new(pill_w, pill_h));

            painter.rect_filled(
                pill_rect,
                egui::Rounding::same(4.0),
                Color32::from_rgba_unmultiplied(214, 150, 74, (0.18 * 255.0) as u8),
            );
            painter.galley(Pos2::new(pill_left + pill_pad, y + 1.0), prefix_g, dim);
            painter.galley(
                Pos2::new(pill_left + pill_pad + prefix_w, y + 1.0),
                badge_g,
                Color32::from_rgba_unmultiplied(214, 150, 74, text_a),
            );
        }

        y += 17.0;

        if show_values && !visible_indices.is_empty() {
            for &i in &visible_indices {
                let lim = &all_limits[i];
                let reset = self.reset_cache.get(i).and_then(|r| r.as_ref());
                let is_compact = self.settings.compact_mode
                    || (lim.used_percent >= LIMIT_PCT
                        && (self.settings.exhausted_mode == crate::config::ExhaustedMode::Compact
                            || (self.settings.exhausted_mode == crate::config::ExhaustedMode::Hidden && all_exhausted)));
                let row_h = draw_limit(
                    &painter,
                    lim,
                    reset,
                    left,
                    right,
                    y,
                    now,
                    op,
                    text_a,
                    dim,
                    strong,
                    show_values,
                    animate,
                    anim_t,
                    i,
                    is_compact,
                    self.settings.show_weekly_limits,
                );
                y += row_h;
            }
        } else if !online && ever {
            painter.text(
                Pos2::new(left, y),
                Align2::LEFT_TOP,
                "нет данных",
                FontId::proportional(11.0),
                dim,
            );
        } else if let Some(e) = &err {
            // Never got data and failing — surface the reason.
            painter.text(
                Pos2::new(left, y),
                Align2::LEFT_TOP,
                format!("ошибка: {e}"),
                FontId::proportional(10.5),
                Color32::from_rgba_unmultiplied(232, 150, 80, text_a),
            );
        } else {
            painter.text(
                Pos2::new(left, y),
                Align2::LEFT_TOP,
                "загрузка данных…",
                FontId::proportional(11.0),
                dim,
            );
        }
    }

    /// Announce a pending update on the tray icon, where it can be seen without
    /// opening anything.
    fn sync_tooltip(&mut self) {
        let want = match &self.shared.update.lock().unwrap().available {
            Some(u) => format!(
                "Quotty {} — доступно обновление {}",
                update::current(),
                u.version
            ),
            None => format!("Quotty {}", update::current()),
        };
        if want != self.tooltip {
            if let Some(t) = &self.tray {
                t.set_tooltip(&want);
            }
            self.tooltip = want;
        }
    }

    /// (online, ever, snapshot, error) of the family on screen.
    fn active_state(&self) -> ActiveState {
        let st = self.shared.states.lock().unwrap();
        let s = &st[self.active.idx()];
        // Guard against a stale slot: only draw a snapshot that says it belongs
        // to the family we're showing.
        let last = s.last.clone().filter(|snap| snap.family == self.active);
        ActiveState {
            // Throttled with data in hand: keep showing it rather than dashes.
            stale: !s.online && s.rate_limited && last.is_some(),
            online: s.online,
            ever: s.ever,
            last,
            error: s.error.clone(),
        }
    }
}

/// Background poller: keeps every enabled family fresh, each on its own
/// schedule, so one dead source (an IDE that isn't running) can't drag the
/// others into a fast retry loop.
fn spawn_poller(shared: Arc<Shared>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let now = std::time::Instant::now();
        let mut due = [now; Family::ALL.len()];
        let mut backoff = [5u64; Family::ALL.len()];
        loop {
            let force = shared.refresh.swap(false, Ordering::Relaxed);
            let want = shared.want.swap(NO_FAMILY, Ordering::Relaxed);
            let mask = shared.enabled.load(Ordering::Relaxed);
            let interval = shared.interval.load(Ordering::Relaxed).max(5);
            let mut changed = false;

            for family in Family::ALL {
                let i = family.idx();
                if mask & (1 << i) == 0 {
                    continue;
                }
                if !(force || want == i as u8 || std::time::Instant::now() >= due[i]) {
                    continue;
                }

                let result = providers::fetch(family);
                let ok = result.is_ok();
                {
                    let mut st = shared.states.lock().unwrap();
                    let s = &mut st[i];
                    match result {
                        Ok(snap) => {
                            s.last = Some(snap);
                            s.online = true;
                            s.ever = true;
                            s.error = None;
                            s.rate_limited = false;
                        }
                        Err(e) => {
                            s.online = false;
                            s.rate_limited = e.rate_limited;
                            s.error = Some(e.msg);
                        }
                    }
                }
                changed = true;
                backoff[i] = if ok { 5 } else { (backoff[i] * 2).min(120) };
                let wait = if ok { interval } else { backoff[i] };
                due[i] = std::time::Instant::now() + std::time::Duration::from_secs(wait);
            }

            if changed {
                ctx.request_repaint();
            }
            providers::flush_log();
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });
}

/// Checks GitHub for a newer release every 8 hours (and on demand). Failures
/// are kept quiet — an offline machine shouldn't produce noise in the UI.
fn spawn_update_checker(shared: Arc<Shared>, ctx: egui::Context) {
    std::thread::spawn(move || loop {
        let result = update::check();
        {
            let mut st = shared.update.lock().unwrap();
            st.checked = true;
            match result {
                Ok(found) => {
                    st.available = found;
                    st.error = None;
                }
                Err(e) => st.error = Some(e),
            }
        }
        ctx.request_repaint();

        // Wake once a second so "проверить сейчас" doesn't wait eight hours.
        for _ in 0..update::CHECK_EVERY_SECS {
            if shared.update_now.swap(false, Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_limit(
    painter: &egui::Painter,
    lim: &providers::Limit,
    reset: Option<&ResetTime>,
    left: f32,
    right: f32,
    y: f32,
    now: DateTime<Utc>,
    op: f32,
    text_a: u8,
    dim: Color32,
    strong: Color32,
    show_values: bool,
    animate: bool,
    anim_t: f64,
    idx: usize,
    is_compact: bool,
    show_weekly_limits: bool,
) -> f32 {
    let time_frac = lim.window.map(|w| w.marker_frac(now));
    let use_frac = (lim.used_percent / 100.0).clamp(0.0, 1.0);
    let past_reset = lim.window.map(|w| now - w.resets_at);
    let show = show_values && past_reset.map_or(true, |d| d < RESET_GRACE);
    let exhausted = lim.used_percent >= LIMIT_PCT;
    let overspend = show && !exhausted && time_frac.is_some_and(|t| use_frac > t + 0.02);

    // Title line: name (left) + optional weekly badge + reset time (far right) + used% (left of it).
    // In compact mode, active models stay strong/bright; only exhausted models are dimmed.
    let title_col = if exhausted { dim } else { strong };
    let title_rect = painter.text(
        Pos2::new(left, y),
        Align2::LEFT_TOP,
        &lim.title,
        FontId::proportional(12.5),
        title_col,
    );

    if show_weekly_limits {
        if let Some(badge_str) = &lim.badge {
            let badge_font = FontId::proportional(10.0);
            let badge_color = Color32::from_rgba_unmultiplied(214, 150, 74, text_a);
            let badge_g = painter.layout_no_wrap(badge_str.clone(), badge_font, badge_color);
            let h_pad = 4.0;
            let v_pad = 1.5;
            let b_w = badge_g.size().x + h_pad * 2.0;
            let b_h = badge_g.size().y + v_pad * 2.0;
            let b_left = title_rect.right() + 5.0;
            let b_top = y + (title_rect.height() - b_h) / 2.0;
            let b_rect = Rect::from_min_size(Pos2::new(b_left, b_top), Vec2::new(b_w, b_h));

            painter.rect_filled(
                b_rect,
                egui::Rounding::same(3.5),
                Color32::from_rgba_unmultiplied(214, 150, 74, (0.15 * 255.0) as u8),
            );
            painter.galley(
                Pos2::new(b_left + h_pad, b_top + v_pad),
                badge_g,
                badge_color,
            );
        }
    }
    let reset_text = match reset {
        Some(r) => r.row.clone(),
        None => "окно ещё не начато".to_string(),
    };
    let reset_rect = painter.text(
        Pos2::new(right, y + 1.0),
        Align2::RIGHT_TOP,
        reset_text,
        FontId::proportional(11.0),
        dim,
    );
    let pct_text = if show {
        format!("{:.0}%", lim.used_percent)
    } else {
        "—".to_string()
    };
    let pct_col = if !show {
        Color32::from_rgba_unmultiplied(150, 155, 165, text_a)
    } else {
        spend_text_color(exhausted, overspend, text_a)
    };
    painter.text(
        Pos2::new(reset_rect.left() - 12.0, y),
        Align2::RIGHT_TOP,
        pct_text,
        FontId::proportional(12.5),
        pct_col,
    );

    if is_compact {
        return 18.0;
    }

    // Single usage bar.
    let track_col = Color32::from_rgba_unmultiplied(60, 64, 76, (op * 220.0) as u8);
    let full_w = right - left;
    let ub_y = y + 18.0;
    let ub_h = 11.0;
    let yc = ub_y + ub_h / 2.0;
    let ub_track = Rect::from_min_max(Pos2::new(left, ub_y), Pos2::new(right, ub_y + ub_h));
    painter.rect_filled(ub_track, egui::Rounding::same(4.0), track_col);

    // Where the time marker sits — nowhere, when the window has no clock.
    let marker_x = time_frac.map(|t| left + full_w * t);

    let green =
        Color32::from_rgba_unmultiplied(96, 196, 132, ((0.55 + 0.45 * op) * 255.0) as u8);
    let yellow =
        Color32::from_rgba_unmultiplied(208, 192, 96, ((0.55 + 0.45 * op) * 255.0) as u8);
    let orange =
        Color32::from_rgba_unmultiplied(214, 150, 74, ((0.55 + 0.45 * op) * 255.0) as u8);

    if show {
        let use_end = left + full_w * use_frac;
        if exhausted {
            painter.rect_filled(ub_track, egui::Rounding::same(4.0), orange);
        } else if let Some(mx) = marker_x.filter(|_| overspend) {
            painter.rect_filled(
                Rect::from_min_max(ub_track.min, Pos2::new(mx, ub_y + ub_h)),
                egui::Rounding::same(4.0),
                green,
            );
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(mx, ub_y), Pos2::new(use_end, ub_y + ub_h)),
                egui::Rounding::same(4.0),
                yellow,
            );
        } else {
            let ub_fill_w = (use_end - left).max(if use_frac > 0.0 { 3.0 } else { 0.0 });
            painter.rect_filled(
                Rect::from_min_size(ub_track.min, Vec2::new(ub_fill_w, ub_h)),
                egui::Rounding::same(4.0),
                green,
            );
            if let Some(mx) = marker_x.filter(|mx| animate && *mx > use_end + 4.0) {
                draw_bubbles_headroom(
                    painter,
                    use_end,
                    mx,
                    0.33 * full_w,
                    ub_y,
                    ub_h,
                    anim_t,
                    (150, 224, 176),
                    op,
                    idx as f64 * 1.7,
                );
            }
        }
    } else if animate {
        draw_bubbles(
            painter,
            left + 2.0,
            right - 2.0,
            yc,
            anim_t,
            (170, 175, 186),
            op,
            idx as f64 * 0.41,
            6,
            0.5,
        );
    } else {
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(left, ub_y), Pos2::new(right, ub_y + ub_h)),
            egui::Rounding::same(4.0),
            Color32::from_rgba_unmultiplied(96, 100, 110, (op * 200.0) as u8),
        );
    }

    if let Some(mx) = marker_x {
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(mx - 1.0, ub_y - 2.0),
                Pos2::new(mx + 1.0, ub_y + ub_h + 2.0),
            ),
            egui::Rounding::same(1.0),
            Color32::from_rgba_unmultiplied(235, 238, 245, ((0.55 + 0.45 * op) * 255.0) as u8),
        );
    }

    34.0
}

/// How long a limit keeps showing its last percentage after its window was due
/// to reset. Longer than any poll interval that still refreshes promptly, short
/// enough that a throttled source cannot pass off yesterday's number as today's.
const RESET_GRACE: Duration = Duration::minutes(10);

/// Percent at/above which a limit is considered "reached" → dim-orange.
const LIMIT_PCT: f32 = 100.0;

/// Usage-% text colour: green (on pace), yellow (overspending vs. time), orange
/// (quota exhausted) — same hues as the bar fill.
fn spend_text_color(exhausted: bool, overspend: bool, a: u8) -> Color32 {
    if exhausted {
        Color32::from_rgba_unmultiplied(232, 150, 80, a)
    } else if overspend {
        Color32::from_rgba_unmultiplied(214, 200, 110, a)
    } else {
        Color32::from_rgba_unmultiplied(120, 205, 150, a)
    }
}

/// Deterministic pseudo-random in 0..1 from two inputs (no RNG → resume-safe).
fn pseudo(a: f64, b: f64) -> f64 {
    ((a * 12.9898 + b * 78.233).sin() * 43758.5453)
        .fract()
        .abs()
}

/// Headroom bubbles: emitted from the spend edge at varying heights, drifting a
/// short way right and dissolving near the left (never reaching the marker).
#[allow(clippy::too_many_arguments)]
fn draw_bubbles_headroom(
    painter: &egui::Painter,
    x_start: f32,
    marker_x: f32,
    max_reach: f32,
    ub_top: f32,
    ub_h: f32,
    t: f64,
    rgb: (u8, u8, u8),
    op: f32,
    seed: f64,
) {
    // Dissolve within min(gap-to-marker, max_reach): if the time marker is close
    // to the spend edge, bubbles dissolve near it; otherwise cap at max_reach.
    let reach = (marker_x - x_start).min(max_reach).max(6.0);

    // Two lanes (upper/lower) so bubbles travel in pairs at different heights,
    // phase-offset so they sit at different x too and never overlap.
    let lanes = [0.30f32, 0.70f32];
    let per_lane = 2; // two staggered bubbles per lane → continuous stream
    let speed = 0.7;
    for (li, lane_y) in lanes.iter().enumerate() {
        for j in 0..per_lane {
            let ph = t * speed
                + seed
                + li as f64 * 0.37 // desync the two lanes in x
                + j as f64 / per_lane as f64; // stagger within a lane
            let p = ph.rem_euclid(1.0) as f32;
            let cycle = ph.floor();
            // Fade in fast, dissolve out by ~0.75 of the (short) travel.
            let fade = ((p * 5.0).min(1.0) * (1.0 - (p / 0.75).min(1.0))).clamp(0.0, 1.0);
            // Small per-cycle jitter around the lane height (kept within the lane
            // band so the two lanes can't collide).
            let jit = (pseudo(li as f64 * 3.0 + j as f64, cycle) as f32 - 0.5) * 0.12;
            let x = x_start + reach * p;
            let y = ub_top + ub_h * (lane_y + jit) - p * 1.5;
            let a = (fade * 205.0 * (0.4 + 0.6 * op)) as u8;
            let r = 0.9 + 0.9 * (1.0 - p);
            painter.circle_filled(
                Pos2::new(x, y),
                r,
                Color32::from_rgba_unmultiplied(rgb.0, rgb.1, rgb.2, a),
            );
        }
    }
}

/// Lightweight procedural bubbles moving left→right, purely a function of time
/// (no per-bubble state → cheap and resume-safe). `seed` desyncs rows.
#[allow(clippy::too_many_arguments)]
fn draw_bubbles(
    painter: &egui::Painter,
    x0: f32,
    x1: f32,
    yc: f32,
    t: f64,
    rgb: (u8, u8, u8),
    op: f32,
    seed: f64,
    n: usize,
    speed: f64,
) {
    let w = x1 - x0;
    if w < 4.0 {
        return;
    }
    for i in 0..n {
        let ph = t * speed + seed + i as f64 / n as f64;
        let p = ph.rem_euclid(1.0) as f32;
        let x = x0 + w * p;
        // Fade in on the left, out on the right.
        let fade = (p * 3.0).min((1.0 - p) * 2.0).clamp(0.0, 1.0);
        let bob = ((ph * 2.0).sin() as f32) * 1.1;
        let a = (fade * 190.0 * (0.4 + 0.6 * op)) as u8;
        let r = 1.3 + 1.0 * (1.0 - p);
        painter.circle_filled(
            Pos2::new(x, yc + bob),
            r,
            Color32::from_rgba_unmultiplied(rgb.0, rgb.1, rgb.2, a),
        );
    }
}


fn fmt_reset(reset: DateTime<Utc>, now: DateTime<Utc>) -> ResetTime {
    let local = reset.with_timezone(&Local);
    let abs = local.format("%H:%M").to_string();
    let diff_secs = (reset - now).num_seconds();

    if diff_secs <= 0 {
        return ResetTime {
            abs: abs.clone(),
            row: format!("Сброс {abs} · сейчас"),
            badge: format!("{abs} (сейчас)"),
        };
    }

    let mins = diff_secs / 60;
    let hours = mins / 60;
    let rem_mins = mins % 60;
    let days = hours / 24;
    let rem_hours = hours % 24;

    let (rel_row, rel_badge) = if days > 0 {
        (
            format!("через {days}д {rem_hours}ч"),
            format!("{days}д {rem_hours}ч"),
        )
    } else if hours > 0 {
        (
            format!("через {hours}ч {rem_mins}м"),
            format!("{hours}ч {rem_mins}м"),
        )
    } else {
        let m = mins.max(1);
        (format!("через {m}м"), format!("{m}м"))
    };

    ResetTime {
        abs: abs.clone(),
        row: format!("Сброс {abs} · {rel_row}"),
        badge: format!("{abs} ({rel_badge})"),
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        LAST_PAINT_MS.store(uptime_ms(), Ordering::Relaxed);
        if self.hwnd.is_none() {
            self.hwnd = native_hwnd(frame);
            if let Some(h) = self.hwnd {
                NATIVE_HWND.store(h, Ordering::Relaxed);
            }
        }
        self.handle_tray_events(ctx);

        let anim_t = ctx.input(|i| i.time);
        self.update_active(anim_t);

        // Smart Focus: auto-hide when foreground app is not an AI tool or editor.
        // Keep visible for the first 10 seconds after launch so the user sees Quotty start.
        let in_grace_period = anim_t < 10.0;
        let should_show = if self.show_settings || in_grace_period {
            true
        } else if self.settings.auto_hide_on_inactive {
            self.is_ai_active
        } else {
            true
        };

        if should_show != self.is_visible {
            self.is_visible = should_show;
            if let Some(h) = self.hwnd {
                set_native_visible(h, should_show);
            }
        }

        // Snapshot animation-relevant state under one lock and calculate pixel-perfect height.
        let animate_on = self.settings.animate;
        let (want_h, animating) = {
            let st = self.shared.states.lock().unwrap();
            let s = &st[self.active.idx()];
            let last = s.last.as_ref().filter(|snap| snap.family == self.active);
            let show_values = s.online || (s.rate_limited && last.is_some());
            let header_h = 17.0;
            let pad_h = 18.0; // 8.0 top + 10.0 bottom

            let (h, anim) = if let (true, Some(snap)) = (show_values, last) {
                let all_limits = &snap.limits;
                let active_limits: Vec<_> = all_limits.iter().filter(|l| l.used_percent < LIMIT_PCT).collect();
                let all_exhausted = active_limits.is_empty();
                let visible_limits: Vec<&providers::Limit> = match self.settings.exhausted_mode {
                    crate::config::ExhaustedMode::Full | crate::config::ExhaustedMode::Compact => {
                        all_limits.iter().collect()
                    }
                    crate::config::ExhaustedMode::Hidden => {
                        if all_exhausted {
                            all_limits.iter().collect()
                        } else {
                            active_limits
                        }
                    }
                };

                let mut rows_h = 0.0f32;
                for lim in &visible_limits {
                    let is_compact = self.settings.compact_mode
                        || (lim.used_percent >= LIMIT_PCT
                            && (self.settings.exhausted_mode == crate::config::ExhaustedMode::Compact
                                || (self.settings.exhausted_mode == crate::config::ExhaustedMode::Hidden && all_exhausted)));
                    rows_h += if is_compact { 18.0 } else { 34.0 };
                }

                let now = Utc::now();
                let has_gap = snap.limits.iter().any(|l| {
                    let Some(time_frac) = l.window.map(|w| w.marker_frac(now)) else {
                        return false;
                    };
                    let use_frac = (l.used_percent / 100.0).clamp(0.0, 1.0);
                    time_frac > use_frac + 0.02
                });
                let stale = !s.online && s.rate_limited;
                let is_animating = if stale {
                    true
                } else if !animate_on {
                    false
                } else if !s.online {
                    true
                } else {
                    has_gap
                };

                (pad_h + header_h + rows_h, is_animating)
            } else {
                let stale = !s.online && s.rate_limited && s.last.is_some();
                let is_animating = if stale {
                    true
                } else if !animate_on {
                    false
                } else {
                    !s.online
                };
                (pad_h + header_h + 18.0, is_animating)
            };

            (h, anim)
        };

        // Fit the window height dynamically to visible content (top edge anchored).
        if (want_h - self.applied_h).abs() > 0.5 {
            #[cfg(windows)]
            if let Some(h) = self.hwnd {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{
                    SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOZORDER,
                };
                let ppp = ctx.pixels_per_point();
                let phys_w = (430.0 * ppp).round() as i32;
                let phys_h = (want_h * ppp).round() as i32;
                unsafe {
                    let _ = SetWindowPos(
                        HWND(h as *mut _),
                        HWND::default(),
                        0,
                        0,
                        phys_w,
                        phys_h,
                        SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
                    );
                }
            }
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(430.0, want_h)));
            self.applied_h = want_h;
        }

        // Re-assert always-on-top periodically
        if anim_t - self.last_topmost >= 0.7 {
            if let Some(h) = self.hwnd {
                force_topmost(h);
            }
            self.last_topmost = anim_t;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let resp = ui.interact(full, ui.id().with("strip-drag"), Sense::click_and_drag());
                if resp.drag_started_by(PointerButton::Primary) {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                    self.dragging = true;
                }
                if resp.clicked_by(PointerButton::Secondary) {
                    #[cfg(windows)]
                    {
                        if let Some(h) = self.hwnd {
                            if let Some(cmd) = show_context_menu(h, self.settings.compact_mode) {
                                match cmd {
                                    1 => {
                                        self.settings.compact_mode = !self.settings.compact_mode;
                                        self.settings.save();
                                    }
                                    2 => {
                                        self.open_settings(true);
                                    }
                                    _ => {}
                                }
                            }
                        } else {
                            self.open_settings(true);
                        }
                    }
                    #[cfg(not(windows))]
                    {
                        self.open_settings(true);
                    }
                }
                self.draw_strip(ui, anim_t, animate_on);
            });

        // Persist the window position when the user finishes moving it
        if self.dragging && ctx.input(|i| i.pointer.any_released()) {
            self.dragging = false;
            if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
                let p = (r.min.x, r.min.y);
                if self.settings.pos != Some(p) {
                    self.settings.pos = Some(p);
                    self.settings.save();
                }
            }
        }

        self.sync_tooltip();
        self.render_settings(ctx);

        let period = if animating { 50 } else { 500 };
        if let Some(h) = self.hwnd {
            if self.timer_period != period {
                arm_repaint_timer(h, period);
                self.timer_period = period;
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(period as u64));
    }
}

#[cfg(test)]
mod tests {
    use super::fmt_reset;
    use chrono::{Duration, Utc};

    #[test]
    fn reset_timer_formatting() {
        let now = Utc::now();
        let rel_row = |secs: i64| fmt_reset(now + Duration::seconds(secs), now).row;
        let rel_badge = |secs: i64| fmt_reset(now + Duration::seconds(secs), now).badge;

        assert!(rel_row(40).ends_with("через 1м"));
        assert!(rel_badge(40).ends_with("(1м)"));

        assert!(rel_row(95).ends_with("через 1м"));
        assert!(rel_badge(95).ends_with("(1м)"));

        assert!(rel_row(2 * 3600 + 45 * 60).ends_with("через 2ч 45м"));
        assert!(rel_badge(2 * 3600 + 45 * 60).ends_with("(2ч 45м)"));

        assert!(rel_row(25 * 3600).ends_with("через 1д 1ч"));
        assert!(rel_badge(25 * 3600).ends_with("(1д 1ч)"));

        assert!(rel_row(0).ends_with("сейчас"));
        assert!(rel_badge(0).ends_with("(сейчас)"));

        assert!(rel_row(-30).ends_with("сейчас"));
        assert!(rel_badge(-30).ends_with("(сейчас)"));
    }
}
