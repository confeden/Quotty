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

/// Why a fetch failed. The distinction that matters to the UI is whether the
/// numbers we already have are still meaningful: a throttled request says
/// nothing about the quota itself, so the last values stay on screen.
#[derive(Clone, Debug)]
pub struct FetchError {
    pub msg: String,
    /// The service refused to answer *for now* (HTTP 429 or our own cooldown).
    pub rate_limited: bool,
}

impl From<String> for FetchError {
    fn from(msg: String) -> Self {
        Self {
            msg,
            rate_limited: false,
        }
    }
}

impl From<&str> for FetchError {
    fn from(msg: &str) -> Self {
        Self::from(msg.to_string())
    }
}

impl FetchError {
    pub fn rate_limited(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            rate_limited: true,
        }
    }
}

/// Fetch a fresh snapshot for one family.
pub fn fetch(family: Family) -> Result<Snapshot, FetchError> {
    match family {
        Family::Claude => claude::fetch(),
        Family::Codex => codex::fetch(),
        Family::Antigravity => antigravity::fetch(),
    }
}

/// Verbose logging, off unless the user turns it on in the settings.
static DIAGNOSTICS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_diagnostics(on: bool) {
    DIAGNOSTICS.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn diagnostics_on() -> bool {
    DIAGNOSTICS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Where both kinds of log line go.
pub fn log_path() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("quotty-debug.log"))
}

/// Best-effort append to the log next to the exe. Failure paths always write
/// here — it exists to diagnose "why did this machine find nothing".
pub fn dbg_log(msg: &str) {
    write_log(msg);
}

/// A line that is only interesting while diagnosing: written when the user has
/// switched diagnostics on, and never otherwise.
pub fn diag(msg: &str) {
    if diagnostics_on() {
        write_log(msg);
    }
}

/// Cap: a machine stuck behind a rate-limited relay produced 47 KB in a day,
/// and nobody prunes this file by hand.
const LOG_MAX_BYTES: u64 = 512 * 1024;

fn write_log(msg: &str) {
    let Some(path) = log_path() else { return };
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > LOG_MAX_BYTES {
        let _ = std::fs::write(&path, b"");
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "{stamp}  {}", scrub(msg));
    }
}

/// Strip anything that identifies the machine or its account: the log is meant
/// to be sendable as-is. Secrets keep a short prefix so two different tokens can
/// still be told apart in one file.
pub fn scrub(msg: &str) -> String {
    let mut out = msg.to_string();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        out = out.replace(&profile, "%USERPROFILE%");
        if let Some(user) = profile.rsplit(['\\', '/']).next() {
            if user.len() >= 3 {
                out = out.replace(user, "<user>");
            }
        }
    }
    for marker in ["sk-ant-", "ya29.", "rt.1.", "eyJ"] {
        out = mask_after(&out, marker);
    }
    out
}

/// Keep `marker` plus six characters, drop the rest of the secret.
fn mask_after(text: &str, marker: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(marker) {
        let (head, tail) = rest.split_at(at + marker.len());
        out.push_str(head);
        let secret_len = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
            .unwrap_or(tail.len());
        let keep = tail
            .char_indices()
            .nth(6)
            .map_or(secret_len, |(i, _)| i.min(secret_len));
        out.push_str(&tail[..keep]);
        out.push('…');
        rest = &tail[secret_len..];
    }
    out.push_str(rest);
    out
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

#[cfg(test)]
mod tests {
    use super::{mask_after, scrub};

    #[test]
    fn secrets_keep_only_a_short_prefix() {
        let out = mask_after("token sk-ant-oat01-ABCDEFGHIJKLMNOP failed", "sk-ant-");
        assert!(out.starts_with("token sk-ant-oat01-…"), "{out}");
        assert!(!out.contains("ABCDEF"), "{out}");
        assert!(out.ends_with(" failed"), "{out}");
    }

    #[test]
    fn the_user_is_not_named() {
        let Ok(profile) = std::env::var("USERPROFILE") else {
            return;
        };
        let out = scrub(&format!(r"reading {profile}\Quotty\settings.json"));
        assert!(!out.contains(&profile), "{out}");
        assert!(out.contains("%USERPROFILE%"), "{out}");
    }
}
