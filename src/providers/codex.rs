//! Codex module: reads the OAuth token that the Codex CLI and the Codex/ChatGPT
//! desktop app share in `~/.codex/auth.json` (plain JSON, no encryption) and
//! asks the ChatGPT backend for the account's rate-limit windows.
//!
//! The token is refreshed by Codex itself; we re-read the file on every poll so
//! a refresh is picked up. We never write to it — rotating the refresh token
//! from here would sign the user out of Codex.

use super::{dbg_log, diag, window_title, Family, Limit, Snapshot};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::path::PathBuf;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/codex/usage";

/// `CODEX_HOME`, else `~/.codex`. Same resolution order the CLI uses.
fn codex_home() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("CODEX_HOME") {
        if !h.is_empty() {
            return Some(PathBuf::from(h));
        }
    }
    if let Some(h) = dirs::home_dir() {
        return Some(h.join(".codex"));
    }
    std::env::var("USERPROFILE")
        .ok()
        .map(|u| PathBuf::from(u).join(".codex"))
}

#[derive(Deserialize)]
struct AuthFile {
    #[serde(default)]
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

struct Auth {
    access: String,
    account_id: Option<String>,
}

fn load_auth() -> Result<Auth, String> {
    let dir = codex_home().ok_or("не найден домашний каталог")?;
    let path = dir.join("auth.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}: {e}", path.display()))
        .map_err(|e| {
            dbg_log(&format!("codex auth.json unreadable: {e}"));
            format!("Codex не найден ({e})")
        })?;
    let parsed: AuthFile =
        serde_json::from_str(&raw).map_err(|e| format!("parse auth.json: {e}"))?;
    let tokens = parsed.tokens.ok_or("в auth.json нет tokens")?;
    let access = tokens
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or("в auth.json нет access_token (войдите в Codex)")?;
    Ok(Auth {
        access,
        account_id: tokens.account_id,
    })
}

// ---------------------------------------------------------------------------
// Usage endpoint
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UsageResponse {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
}

#[derive(Deserialize)]
struct RateLimit {
    #[serde(default)]
    primary_window: Option<Window>,
    #[serde(default)]
    secondary_window: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
    /// Unix seconds. Absent on some plans — then `reset_after_seconds` is used.
    #[serde(default)]
    reset_at: Option<i64>,
}

pub fn fetch() -> Result<Snapshot, String> {
    let auth = load_auth()?;

    let mut req = ureq::get(USAGE_URL)
        .set("Authorization", &format!("Bearer {}", auth.access))
        .set("User-Agent", "Quotty/0.1")
        .timeout(std::time::Duration::from_secs(20));
    if let Some(acc) = &auth.account_id {
        req = req.set("chatgpt-account-id", acc);
    }

    let started = std::time::Instant::now();
    let usage: UsageResponse = match req.call() {
        Ok(resp) => {
            diag(&format!(
                "codex: 200 in {} ms",
                started.elapsed().as_millis()
            ));
            resp.into_json().map_err(|e| format!("parse usage: {e}"))?
        }
        Err(ureq::Error::Status(401, _)) => {
            return Err("токен Codex устарел — откройте Codex".into())
        }
        Err(ureq::Error::Status(code, _)) => {
            diag(&format!("codex: status {code}"));
            return Err(format!("status {code}"));
        }
        Err(e) => {
            diag(&format!("codex: network error ({e})"));
            return Err(format!("network: {e}"));
        }
    };

    Ok(build_snapshot(usage))
}

fn build_snapshot(usage: UsageResponse) -> Snapshot {
    let now = Utc::now();
    let mut limits = Vec::new();
    if let Some(rl) = usage.rate_limit {
        for w in [rl.primary_window, rl.secondary_window]
            .into_iter()
            .flatten()
        {
            if let Some(l) = to_limit(w, now) {
                limits.push(l);
            }
        }
    }
    Snapshot {
        family: Family::Codex,
        plan: pretty_plan(usage.plan_type.as_deref()),
        limits,
    }
}

fn to_limit(w: Window, now: DateTime<Utc>) -> Option<Limit> {
    let used = w.used_percent? as f32;
    let resets_at = w
        .reset_at
        .and_then(|s| Utc.timestamp_opt(s, 0).single())
        .or_else(|| {
            w.reset_after_seconds
                .map(|s| now + chrono::Duration::seconds(s))
        })?;
    // Window length is given; without it the time marker would be meaningless,
    // so fall back to "the window started when we first saw it".
    let span = w
        .limit_window_seconds
        .unwrap_or_else(|| (resets_at - now).num_seconds().max(1));
    Some(Limit {
        title: window_title(span),
        used_percent: used,
        window_start: resets_at - chrono::Duration::seconds(span),
        resets_at,
    })
}

/// `plan_type` from the backend ("free", "plus", "pro", "business", …).
fn pretty_plan(plan: Option<&str>) -> String {
    match plan {
        None | Some("") => "Codex".into(),
        Some(p) => {
            let mut c = p.chars();
            let pretty = match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            };
            format!("Codex {pretty}")
        }
    }
}
