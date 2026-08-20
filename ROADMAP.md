# Quotty — working context (read before substantive work)

Project root: `D:\Documents\Coding\Quotty`. Rust + eframe/egui, Windows-only.
Repo: `https://github.com/confeden/Quotty` (nothing pushed yet as of v1.0.0 prep).

## Current state
- v1.0.0, builds clean (no warnings): `cargo build --release`; one unit test (`update::is_newer`).
- `target/release/quotty.exe` ~4.2 MB, windowless, icon + VERSIONINFO embedded via `build.rs` + `quotty.rc`.
- Installer: `powershell -ExecutionPolicy Bypass -File tools\build-installer.ps1` → `dist\Quotty-Setup-<ver>.exe` (Inno Setup 6 at `D:\Programs\Inno Setup 6`). Silent install + uninstall verified.
- Desktop/autostart shortcuts point at `target\release\quotty.exe` (the dev build), not the installed copy.

## What it does
A movable, translucent, always-on-top, borderless strip showing the quota of the AI tool **currently in use**. No taskbar button.

- **Three families** (`src/providers/`), each a module with `fetch() -> Snapshot{family, plan, limits}`:
  - **Claude** — Claude Desktop's encrypted token → `api.anthropic.com/api/oauth/usage`. Covers Claude Code/CLI (same account).
  - **Codex** — `~/.codex/auth.json` (shared by app + CLI) → `https://chatgpt.com/backend-api/codex/usage`, headers `Authorization: Bearer`, `chatgpt-account-id`. Returns `plan_type` + `rate_limit.{primary,secondary}_window{used_percent, limit_window_seconds, reset_at}`.
  - **Antigravity** — local language server RPC `POST https://127.0.0.1:<port>/exa.language_server_pb.LanguageServerService/GetUserStatus`, body `{"metadata":{"ideName":"antigravity"}}`, headers `X-Codeium-Csrf-Token`, `Connect-Protocol-Version: 1`. Response: `userStatus.cascadeModelConfigData.clientModelConfigs[].{label, quotaInfo{remainingFraction, resetTime}}`, `userTier.name`, `planStatus.planInfo.planName`.
- **Active-tool switching** (`src/active.rs`): foreground window → process name → family; hosts (terminals, VS Code-likes) are scanned for a descendant `claude.exe`/`codex.exe`/`agy.exe`. Unknown app → keep the previous family. Pinned mode in settings overrides.
- **Header**: environment+plan / family only / hidden (setting) · status dot right.
- **Per limit**, one bar each, stride 34px: title · used% · exact reset · usage bar with white time marker; green (under pace, bubbles) / dim-yellow past the marker (overspend) / dim-orange (≥100%); offline = grey flowing bubbles and "—".
- **Tray menu**: Автозапуск · Настройки… · Обновить сейчас · Выход. Tooltip carries the version and any pending update.
- **Settings window** (`src/settings_ui.rs`): borderless dark panel in the strip's palette, fitted to content, centred on the monitor it was opened from, custom title bar with the `Brent © | t.me/nova_txt` credit. Cards: внешний вид · источники (+ live per-source status) · опрос квот · система · версия.
- **Update check** (`src/update.rs`): GitHub `releases/latest` tag every 8 h (and on demand); shows a link only, never downloads.

## Architecture & invariants
- `Shared`: `Mutex<Vec<FetchState>>` indexed by `Family::idx()`, `enabled` bitmask, `want` (poll this family now), `refresh`, `interval`, `update`, `update_now`.
- Poller thread: per-family due times, error backoff 5 s → ×2 → 120 s, so a dead source (IDE not running) can't drag the others into a fast retry loop. Switching family sets `want` → immediate poll.
- Window height = `8 + 17 + n*34 + 6`, recomputed when the active family's limit count changes. **The top edge must stay put** (owner decision) — grow downward, no work-area clamp.
- Claude token: `%APPDATA%\Claude\config.json` → `oauth:tokenCache`, Chromium `v10` + 12-byte nonce, AES-256-GCM; key from `Local State` → `os_crypt.encrypted_key`, strip `DPAPI`, `CryptUnprotectData`. Files re-read every poll.
- `window_start` is synthesized (`resets_at - window length`); no API gives a window start. Antigravity has no length at all → 5 h assumed, stretched if the reset is further out.
- Settings persist to `%APPDATA%\Quotty\settings.json`; window position saved on pointer release; auto-switched family saved at most every 30 s.
- Shortcuts via `mslnk`. Autostart = `Quotty.lnk` in `%APPDATA%\...\Startup`.

## Gotchas
- **egui keeps one style per theme.** `ctx.set_style()` writes only the *current* theme's style, and winit reports the system theme after `App::new`, so a light-mode desktop silently reverted the settings window to egui's default greys. `apply_style` writes both slots and pins `ThemePreference::Dark`.
- **Modal Win32 loops freeze the animation.** The tray's right-click menu runs its own message pump; winit's `MsgWaitForMultipleObjectsEx` wake-up never fires, so the strip froze until the mouse touched it. Fix: `SetTimer` with a TIMERPROC on our HWND → `InvalidateRect` → WM_PAINT → winit `RedrawRequested`. It only invalidates when egui hasn't painted within the period, so idle cost is nil. Verified: bubbles keep moving with the menu open and after it closes.
- **`conhost.exe` is a *child* of the console owner**, so a CLI is its sibling, not its descendant — `family_in_tree(pid, hop_parent)` starts one level up for console hosts.
- **Antigravity's language server uses a self-signed cert on 127.0.0.1** → dedicated `ureq` agent with a rustls verifier that accepts anything (only for that host). Port+CSRF discovery: `%APPDATA%\Antigravity[ IDE]\logs\main.log` (`--csrf_token …`, `Local: https://127.0.0.1:<port>/`) and `~/.gemini/*/daemon/ls_*.json` (`httpsPort`, `csrfToken`), newest first.
- **Gemini Pro and Flash share one quota pool** (owner) — one "Gemini" row, third-party models ("Claude / GPT") a second one.
- **Never refresh the Codex token.** Refresh tokens rotate; writing `auth.json` would sign the user out of Codex. Access tokens live ~10 days and Codex itself refreshes them.
- **Windows 11 draws its own rounded border** on top-level windows; over a borderless panel that paints its own corners it reads as a double arc. `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE=DONOTROUND, DWMWA_BORDER_COLOR=NONE)`.
- **Claude data dir resolution must be multi-source** (`APPDATA` → `dirs::config_dir()` → `USERPROFILE\AppData\Roaming` → home) and files must be **tested by reading**, not `is_file()` — Claude Desktop replaces them atomically. `read_with_retry()` = 5 × 120 ms.
- **config.json holds MULTIPLE OAuth entries**; only some work (200 / 429 / 403). `load_tokens()` returns profile-scoped ones (subscription first); `fetch()` tries each and caches the winner in `LAST_GOOD`.
- **Taskbar covering the strip**: Win32 `SetWindowPos(HWND_TOPMOST, …NOACTIVATE)` every 0.7 s.
- **Tray menu events must wake the loop** — `MenuEvent::set_event_handler` forwards into `App.menu_rx` *and* calls `ctx.request_repaint()`. Poll `menu_rx`, not the global receiver.
- The "●" glyph and "✕" are **not** in the bundled font — draw them with the painter.
- No panel border stroke on the strip — it reads as a fringe on the transparent rounded corners.
- Debug log `quotty-debug.log` is written next to the exe **only on failure paths**.
- Moving the project invalidates `target/` and the `.lnk` targets.

## Negative knowledge (tested — do not retry)
- **`ViewportCommand::WindowLevel(AlwaysOnTop)` cannot re-assert topmost** — winit early-returns when the level is unchanged. Win32 only.
- **`ctx.set_style()` alone is not enough** (see gotcha) — it looks fine until the OS theme is light.
- Dropping the wgpu feature did not reduce memory; glow was already the runtime path.
- PowerShell 5.1 has **no `AesGcm`** — can't decrypt the token cache there.
- `.claude/.credentials.json` does not exist on this machine and Credential Manager has no Claude entry — the Claude token lives only in the Desktop app's encrypted `config.json`.
- `AppActivate` / `SwitchToThisWindow` alone cannot take the foreground; the dev helper needs `AttachThreadInput` + an ALT tap (`tools/dev/focus.ps1`, gitignored).

## Owner decisions
- Height changes keep the **top** edge fixed.
- One shared Gemini quota row for Antigravity.
- Opacity presets live in Settings; the tray's Непрозрачность submenu is gone.
- Credit line `Brent © | t.me/nova_txt` on the settings title row, link clickable.
- README in Russian, in the style of the owner's other repos, emphasising **no telemetry, no ads**.
- Update check by GitHub release tag, every 8 hours, link-only.

## Performance
- Runtime ~73 MB working set — the eframe+glow+winit floor.
- Animating repaint ~20 FPS, idle 2 FPS; ~0.2 % CPU at 12 FPS.
- `reset_cache`: "Resets …" strings reformatted at most once per second.
- Bubbles are a pure function of `ctx.input().time` — no per-bubble state.

## Open issues / Plans
- Nothing pushed to GitHub yet: no commits on the remote, no release, no LICENSE file chosen.
- Codex goes offline if Codex itself hasn't run for ~10 days (token expiry, by design). Antigravity needs its app/IDE running.
- The settings credit link is rendered as a hyperlink but the click was never exercised (would open a browser on the owner's desktop).
- Claude "exhausted" (orange) state still unobserved live; overspend (yellow) was seen on Codex.
