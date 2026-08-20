//! Quota providers. One module per tool family; each knows how to read its own
//! quota from whatever the installed tool leaves on this machine (an encrypted
//! token, a plain OAuth file, a locally running language server).
//!
//! Everything above this module only ever sees `Family` / `Snapshot` / `Limit`.

pub mod antigravity;
pub mod claude;
pub mod codex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A tool family. One family covers every surface of the same product (app,
/// IDE and CLI) because they all bill against the same account quota.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Family {
    Claude,
    Codex,
    Antigravity,
}

impl Family {
    pub const ALL: [Family; 3] = [Family::Claude, Family::Codex, Family::Antigravity];

    /// Stable index, used for the per-family arrays and the enabled-bitmask.
    pub fn idx(self) -> usize {
        match self {
            Family::Claude => 0,
            Family::Codex => 1,
            Family::Antigravity => 2,
        }
    }

    /// Family name alone (no plan/tier) — what the header shows in "family only".
    pub fn name(self) -> &'static str {
        match self {
            Family::Claude => "Claude",
            Family::Codex => "Codex",
            Family::Antigravity => "Antigravity",
        }
    }
}

/// One quota window as we want to render it.
#[derive(Clone, Debug)]
pub struct Limit {
    pub title: String,
    /// Quota consumed, 0.0..=100.0
    pub used_percent: f32,
    /// Start of the current window (reset_at minus window length).
    pub window_start: DateTime<Utc>,
    /// When this quota window resets.
    pub resets_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub family: Family,
    /// Environment + plan, e.g. "Claude Max 20×", "Codex Plus".
    pub plan: String,
    pub limits: Vec<Limit>,
}

/// Fetch a fresh snapshot for one family.
pub fn fetch(family: Family) -> Result<Snapshot, String> {
    match family {
        Family::Claude => claude::fetch(),
        Family::Codex => codex::fetch(),
        Family::Antigravity => antigravity::fetch(),
    }
}

/// Best-effort append to a debug log next to the exe. Only written on failure
/// paths — it exists to diagnose "why did this machine find nothing".
pub fn dbg_log(msg: &str) {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let path = dir.join("quotty-debug.log");
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = writeln!(f, "{msg}");
            }
        }
    }
}

/// Human title for a quota window of the given length, so every provider names
/// its windows the same way.
pub fn window_title(seconds: i64) -> String {
    match seconds {
        s if s <= 0 => "limit".into(),
        s if (17000..=19000).contains(&s) => "5-hour limit".into(),
        s if (600_000..=700_000).contains(&s) => "Weekly · all models".into(),
        s if (2_500_000..=2_700_000).contains(&s) => "Monthly limit".into(),
        s if s % 86_400 == 0 => format!("{}-day limit", s / 86_400),
        s => format!("{}-hour limit", (s + 1800) / 3600),
    }
}
