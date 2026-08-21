//! Persistent Quotty settings, stored in %APPDATA%\Quotty\settings.json.

use crate::providers::Family;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

/// What the strip's header line shows.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderMode {
    /// Environment + plan, e.g. "Claude Max 20×".
    Full,
    /// Family only, e.g. "Claude".
    FamilyOnly,
    Hidden,
}

/// Which family the strip shows.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveMode {
    /// Follow the foreground window — whichever tool was used last.
    Auto,
    /// Always show `Settings::family`.
    Pinned,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// Window opacity, 0.15..=1.0
    pub opacity: f32,
    /// Last window position (screen points). None = let the OS place it.
    pub pos: Option<(f32, f32)>,
    /// Poll interval in seconds.
    pub poll_secs: u64,
    /// Bubble animation on/off (off = lowest CPU).
    pub animate: bool,
    /// Header line content.
    pub header_mode: HeaderMode,
    pub claude_enabled: bool,
    pub codex_enabled: bool,
    pub antigravity_enabled: bool,
    pub active_mode: ActiveMode,
    /// Pinned family, and the one restored at startup.
    pub family: Family,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            opacity: 0.8,
            pos: None,
            poll_secs: 60,
            animate: true,
            header_mode: HeaderMode::Full,
            claude_enabled: true,
            codex_enabled: true,
            antigravity_enabled: true,
            active_mode: ActiveMode::Auto,
            family: Family::Claude,
        }
    }
}

/// Where `settings.json` may live. `dirs::config_dir()` alone is not enough:
/// depending on how the exe was started (shortcut, Startup folder, shell) it has
/// been seen to resolve to something other than the real Roaming directory, and
/// the app would then silently start from defaults every login.
fn candidate_paths() -> Vec<PathBuf> {
    let mut dirs_list: Vec<PathBuf> = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        dirs_list.push(PathBuf::from(appdata));
    }
    if let Some(cfg) = dirs::config_dir() {
        dirs_list.push(cfg);
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        dirs_list.push(PathBuf::from(profile).join("AppData").join("Roaming"));
    }
    if let Some(home) = dirs::home_dir() {
        dirs_list.push(home.join("AppData").join("Roaming"));
    }
    dirs_list.dedup();
    dirs_list
        .into_iter()
        .map(|d| d.join("Quotty").join("settings.json"))
        .collect()
}

impl Settings {
    /// Resolved once: an existing file wins wherever it is, so load and save
    /// can never end up on different paths.
    fn path() -> Option<PathBuf> {
        static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
        PATH.get_or_init(|| {
            let candidates = candidate_paths();
            candidates
                .iter()
                .find(|p| std::fs::metadata(p).is_ok())
                .cloned()
                .or_else(|| candidates.into_iter().next())
        })
        .clone()
    }

    pub fn load() -> Self {
        let path = Self::path();
        let raw = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
        // Falling back to defaults silently would look like "the app forgot
        // everything", so say why in the debug log.
        let mut s: Settings = match &raw {
            None => {
                crate::providers::dbg_log(&format!(
                    "settings: nothing readable among {:?}",
                    candidate_paths()
                ));
                Settings::default()
            }
            // Windows editors and PowerShell's `Set-Content -Encoding UTF8`
            // put a BOM in front, which JSON parsers reject outright.
            Some(text) => match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
                Ok(parsed) => parsed,
                Err(e) => {
                    crate::providers::dbg_log(&format!("settings: {path:?} unparseable: {e}"));
                    Settings::default()
                }
            },
        };
        s.opacity = s.opacity.clamp(0.15, 1.0);
        if s.poll_secs < 15 {
            s.poll_secs = 15;
        }
        // Never leave the user with nothing to show.
        if !(s.claude_enabled || s.codex_enabled || s.antigravity_enabled) {
            s.claude_enabled = true;
        }
        if !s.enabled(s.family) {
            s.family = s.first_enabled();
        }
        s
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(raw) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, raw);
            }
        }
    }

    pub fn enabled(&self, f: Family) -> bool {
        match f {
            Family::Claude => self.claude_enabled,
            Family::Codex => self.codex_enabled,
            Family::Antigravity => self.antigravity_enabled,
        }
    }

    pub fn set_enabled(&mut self, f: Family, on: bool) {
        match f {
            Family::Claude => self.claude_enabled = on,
            Family::Codex => self.codex_enabled = on,
            Family::Antigravity => self.antigravity_enabled = on,
        }
        if !(self.claude_enabled || self.codex_enabled || self.antigravity_enabled) {
            // Refuse to turn the last one off.
            self.set_enabled(f, true);
        }
        if !self.enabled(self.family) {
            self.family = self.first_enabled();
        }
    }

    pub fn first_enabled(&self) -> Family {
        Family::ALL
            .into_iter()
            .find(|f| self.enabled(*f))
            .unwrap_or(Family::Claude)
    }

    /// Bitmask of enabled families, as handed to the poller thread.
    pub fn enabled_mask(&self) -> u8 {
        Family::ALL
            .into_iter()
            .filter(|f| self.enabled(*f))
            .fold(0u8, |m, f| m | (1 << f.idx()))
    }
}
