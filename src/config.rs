//! Persistent Quotty settings, stored in %APPDATA%\Quotty\settings.json.

use crate::providers::Family;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

impl Settings {
    fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|p| p.join("Quotty").join("settings.json"))
    }

    pub fn load() -> Self {
        let mut s: Settings = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
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
