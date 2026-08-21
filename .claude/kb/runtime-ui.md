# Runtime and UI — repaint scheme, geometry, palette, measurements
Expands G1, G2, G7, G12, G13, I1, I8, I9, D1, D3, D4, M1-M10, M15, M16.
Code: `src/app.rs`, `src/settings_ui.rs`.

## Measurements (observed, v1.0.0)
| Metric | Value |
|---|---|
| working set | ~73 MB — the eframe + glow + winit floor, not Quotty's own allocation |
| repaint while animating | ~20 FPS (50 ms period) |
| repaint while idle | 2 FPS (500 ms period) — keeps timers and labels flowing |
| CPU | ~0.2 % at 12 FPS |
| release binary | ~4.2 MB, windowless, `opt-level="z"`, LTO, stripped, `panic=abort` |

Cheapness comes from two rules: `reset_cache` reformats the "Resets …" strings at most once
per second, and the bubbles are a pure function of `ctx.input().time` — no per-bubble state,
nothing to update between frames.

## G2 — modal Win32 loops froze the animation
The tray's right-click menu runs its own message pump, so winit's
`MsgWaitForMultipleObjectsEx` wake-up never fires and the strip froze until the mouse
touched it. Fix: `SetTimer` with a TIMERPROC on our HWND → `InvalidateRect` → `WM_PAINT` →
winit `RedrawRequested`. The TIMERPROC only invalidates when egui has not painted within the
current period (`LAST_PAINT_MS` / `TIMER_GRACE_MS`), so the idle cost is nil. Verified: the
bubbles keep moving with the menu open and after it closes.

The timer is re-armed only when the period changes between the 50 ms and 500 ms modes.

## Geometry
- width 430 px, height `8 + 17 + n_limits*34 + 6`, recomputed when the active family's limit
  count changes (I1).
- Growth is downward only and is not clamped to the work area: the **top** edge must stay
  where the user put it (D1).
- Topmost is re-asserted with `SetWindowPos(HWND_TOPMOST, …NOACTIVATE)` every 0.7 s so the
  taskbar cannot cover the strip (I8, N1).

## Strip content
- Header (setting): environment + plan / family only / hidden; status dot on the right.
- One bar per limit, stride 34 px: title · used % · exact reset time · usage bar with a white
  time marker.
- Colours: green while under pace (bubbles rise from the spent edge) · dim yellow past the
  marker (overspending) · dim orange at ≥ 100 % · offline shows grey flowing bubbles and "—".
- No panel border stroke — a coloured outline reads as a fringe on the transparent rounded
  corners (G13).
- "●" and "✕" are not in the bundled font; the painter draws them (G12).
- LMB drags the strip, RMB opens settings.

## Tray
Menu items (literal, Russian): `Автозапуск` · `Настройки…` · `Обновить сейчас` · `Выход`.
The tooltip carries the version and any pending update.
Menu events must be forwarded into `App.menu_rx` *and* trigger `ctx.request_repaint()` (G11).

## Settings window
Borderless dark panel in the strip's palette, fitted to content, centred on the monitor it
was opened from (from the strip's HWND work area, else the cursor's), custom title bar with
the clickable `Brent © | t.me/nova_txt` credit (D4).
Cards (literal, Russian): `внешний вид` · `источники` (with live per-source status) ·
`опрос квот` · `система` · `версия`. Opacity presets live here; the tray's `Непрозрачность`
submenu was removed (D3).

Palette constants live at the top of `src/settings_ui.rs`
(`BG`, `CARD`, `CARD_HI`, `ACCENT`, `ACCENT_BG`, `TEXT`, `DIM`, `HINT`, `WARN`).

### G1 / I9 — the style must be written to both theme slots
egui keeps one style per theme and `ctx.set_style()` writes only the current theme's slot;
winit reports the system theme *after* `App::new`, so on a light-mode desktop the window
silently reverted to egui's default greys. `apply_style` calls `set_style_of` for both
`Theme::Dark` and `Theme::Light` and pins `ThemePreference::Dark`.

### G7 — Windows 11's own rounded border
Over a borderless panel that paints its own corners it reads as a double arc.
`drop_system_chrome` sets `DWMWA_WINDOW_CORNER_PREFERENCE = DONOTROUND` and
`DWMWA_BORDER_COLOR = NONE`.
