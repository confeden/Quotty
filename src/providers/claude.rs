//! Claude module: reads the Claude Desktop app's locally-stored OAuth token
//! (Chromium `os_crypt` AES-256-GCM, key wrapped with Windows DPAPI) and queries
//! the account usage endpoint for the 5-hour and weekly quota windows.
//!
//! Covers Claude Code / the Claude CLI too: on Windows they run inside the
//! Desktop app's account, so the same token and the same quota apply.

use super::{dbg_log, Family, Limit, Snapshot};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Token retrieval
// ---------------------------------------------------------------------------

/// Every directory Claude Desktop might keep its data in.
///
/// Two install kinds have to be covered:
/// * the classic installer, which uses `%APPDATA%\Claude` — but different launch
///   contexts can make `dirs::config_dir()` disagree with the `APPDATA` env var,
///   so several roots are tried and validated on disk;
/// * the Microsoft Store (MSIX) build, which never touches the real `%APPDATA%`
///   at all. Inside its package container that path is redirected, so from any
///   other process the data is at
///   `%LOCALAPPDATA%\Packages\<package>\LocalCache\Roaming\Claude`.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(a) = std::env::var("APPDATA") {
        roots.push(PathBuf::from(a));
    }
    if let Some(c) = dirs::config_dir() {
        roots.push(c);
    }
    if let Ok(u) = std::env::var("USERPROFILE") {
        roots.push(PathBuf::from(u).join("AppData").join("Roaming"));
    }
    if let Some(h) = dirs::home_dir() {
        roots.push(h.join("AppData").join("Roaming"));
    }
    roots.dedup();

    let mut dirs_list: Vec<PathBuf> = roots.into_iter().map(|r| r.join("Claude")).collect();
    msix_dirs(&mut dirs_list);
    dirs_list.dedup();
    dirs_list
}

/// Data directories of Store-installed Claude packages.
fn msix_dirs(out: &mut Vec<PathBuf>) {
    let mut locals: Vec<PathBuf> = Vec::new();
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        locals.push(PathBuf::from(l));
    }
    if let Some(l) = dirs::data_local_dir() {
        locals.push(l);
    }
    locals.dedup();

    for local in locals {
        let Ok(packages) = std::fs::read_dir(local.join("Packages")) else {
            continue;
        };
        for entry in packages.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !(name.starts_with("claude") || name.contains("anthropic")) {
                continue;
            }
            let cache = entry.path().join("LocalCache");
            out.push(cache.join("Roaming").join("Claude"));
            out.push(cache.join("Local").join("Claude"));
        }
    }
}

struct ClaudeFiles {
    local_state: String,
    config: String,
}

/// Read a file, retrying briefly to ride out the moment when Claude Desktop
/// atomically replaces it (write-temp + rename can make the path transiently
/// unreadable).
fn read_with_retry(path: &std::path::Path) -> std::io::Result<String> {
    let mut last = None;
    for _ in 0..5 {
        match std::fs::read_to_string(path) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(120));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "unreadable")))
}

/// Find the Claude Desktop data dir by actually reading its two files (not just
/// `is_file()`, which is racy). With both a classic and a Store install present,
/// the one whose `config.json` was written last wins — the other is a leftover
/// holding a stale token.
fn find_claude_files() -> Result<ClaudeFiles, String> {
    let mut diag: Vec<String> = Vec::new();
    let mut tried: Vec<String> = Vec::new();
    let mut best: Option<(std::time::SystemTime, ClaudeFiles)> = None;

    for dir in candidate_dirs() {
        // Skip absent directories outright: retrying reads there would add up
        // to half a second each, and there is nothing to race with.
        let Ok(config_meta) = std::fs::metadata(dir.join("config.json")) else {
            tried.push(dir.display().to_string());
            continue;
        };
        let ls = read_with_retry(&dir.join("Local State"));
        let cfg = read_with_retry(&dir.join("config.json"));
        match (ls, cfg) {
            (Ok(local_state), Ok(config)) => {
                let stamp = config_meta
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                if best.as_ref().map_or(true, |(t, _)| stamp > *t) {
                    best = Some((
                        stamp,
                        ClaudeFiles {
                            local_state,
                            config,
                        },
                    ));
                }
            }
            (ls, cfg) => {
                diag.push(format!(
                    "candidate {:?}: Local State={} config.json={}",
                    dir,
                    ls.map(|_| "ok".into()).unwrap_or_else(|e| e.to_string()),
                    cfg.map(|_| "ok".into()).unwrap_or_else(|e| e.to_string()),
                ));
                tried.push(dir.display().to_string());
            }
        }
    }
    if let Some((_, files)) = best {
        return Ok(files);
    }
    // Only touch the log when we actually fail.
    dbg_log("--- find_claude_files() FAILED ---");
    dbg_log(&format!("APPDATA env = {:?}", std::env::var("APPDATA")));
    dbg_log(&format!("dirs::config_dir() = {:?}", dirs::config_dir()));
    for d in &diag {
        dbg_log(d);
    }
    tried.dedup();
    Err(format!(
        "Claude data dir not found. Открыт ли Claude Desktop? Искал: {}",
        tried.join(" | ")
    ))
}

#[derive(Deserialize)]
struct LocalState {
    os_crypt: OsCrypt,
}
#[derive(Deserialize)]
struct OsCrypt {
    encrypted_key: String,
}

/// Unwrap the Chromium AES key from the `Local State` JSON using DPAPI.
#[cfg(windows)]
fn master_key(local_state_json: &str) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptUnprotectData, CRYPT_INTEGER_BLOB};

    let ls: LocalState =
        serde_json::from_str(local_state_json).map_err(|e| format!("parse Local State: {e}"))?;
    let mut key = B64
        .decode(ls.os_crypt.encrypted_key.as_bytes())
        .map_err(|e| format!("b64 key: {e}"))?;
    // Strip the "DPAPI" prefix (5 bytes).
    if key.len() < 5 || &key[..5] != b"DPAPI" {
        return Err("unexpected key prefix".into());
    }
    let mut blob = key.split_off(5);

    unsafe {
        let mut in_blob = CRYPT_INTEGER_BLOB {
            cbData: blob.len() as u32,
            pbData: blob.as_mut_ptr(),
        };
        let mut out_blob = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&mut in_blob, None, None, None, None, 0, &mut out_blob)
            .map_err(|e| format!("CryptUnprotectData: {e}"))?;

        let out = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out_blob.pbData as *mut _));
        Ok(out)
    }
}

#[cfg(not(windows))]
fn master_key(_local_state_json: &str) -> Result<Vec<u8>, String> {
    Err("Claude token decryption is only implemented on Windows".into())
}

#[derive(Deserialize)]
struct TokenEntry {
    token: String,
    #[serde(default)]
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(default)]
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
}

pub struct OauthToken {
    pub access: String,
    pub subscription: Option<String>,
    pub tier: Option<String>,
}

/// Decrypt `config.json`'s `oauth:tokenCache` into all usable tokens, ordered by
/// preference (subscription + `user:profile` scope first). config.json can hold
/// several OAuth entries (different app registrations) — some are stale/rate-
/// limited/wrong-scope, so `fetch()` tries them in order until one works.
pub fn load_tokens() -> Result<Vec<OauthToken>, String> {
    let files = find_claude_files()?;
    let key_bytes = master_key(&files.local_state)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

    let cfg: serde_json::Value =
        serde_json::from_str(&files.config).map_err(|e| format!("parse config.json: {e}"))?;
    let enc = cfg
        .get("oauth:tokenCache")
        .and_then(|v| v.as_str())
        .ok_or("no oauth:tokenCache in config.json")?;

    let raw = B64
        .decode(enc.as_bytes())
        .map_err(|e| format!("b64 cache: {e}"))?;
    if raw.len() < 3 + 12 + 16 || &raw[..3] != b"v10" {
        return Err("unexpected token cache format".into());
    }
    let nonce = Nonce::from_slice(&raw[3..15]);
    let plain = cipher
        .decrypt(nonce, &raw[15..])
        .map_err(|_| "AES-GCM decrypt failed".to_string())?;
    let json: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&plain).map_err(|e| format!("parse token json: {e}"))?;

    let mut scored: Vec<(u8, OauthToken)> = Vec::new();
    for (scope_key, val) in json.iter() {
        let entry: TokenEntry = match serde_json::from_value(val.clone()) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let has_profile = scope_key.contains("user:profile");
        let has_sub = entry.subscription_type.is_some();
        // The usage endpoint needs the profile scope; inference-only tokens 403.
        if !has_profile {
            continue;
        }
        let score = (has_sub as u8) * 2 + has_profile as u8;
        scored.push((
            score,
            OauthToken {
                access: entry.token,
                subscription: entry.subscription_type,
                tier: entry.rate_limit_tier,
            },
        ));
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0)); // stable: keeps file order within a score
    let tokens: Vec<OauthToken> = scored.into_iter().map(|(_, t)| t).collect();
    if tokens.is_empty() {
        return Err("no usable OAuth token (profile scope) in config.json".into());
    }
    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Usage endpoint
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UsageWindow {
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Deserialize)]
struct UsageResponse {
    #[serde(default)]
    five_hour: Option<UsageWindow>,
    #[serde(default)]
    seven_day: Option<UsageWindow>,
}

fn parse_ts(s: &Option<String>) -> Option<DateTime<Utc>> {
    let s = s.as_ref()?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// The access token that last returned 200, so we try it first next poll and
/// don't repeatedly hit stale/rate-limited entries.
static LAST_GOOD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// One usage request. `Err((retryable, msg))`: retryable = try the next token
/// (auth/scope/rate-limit status); non-retryable = give up this poll.
fn request_usage(access: &str) -> Result<UsageResponse, (bool, String)> {
    match ureq::get("https://api.anthropic.com/api/oauth/usage")
        .set("Authorization", &format!("Bearer {access}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("anthropic-version", "2023-06-01")
        .set("User-Agent", "Quotty/0.1")
        .timeout(std::time::Duration::from_secs(20))
        .call()
    {
        Ok(resp) => resp
            .into_json()
            .map_err(|e| (false, format!("parse usage: {e}"))),
        Err(ureq::Error::Status(code, _)) => {
            let retryable = matches!(code, 401 | 403 | 429);
            Err((retryable, format!("status {code}")))
        }
        // Transport error (offline, DNS, TLS): same for every token → stop.
        Err(e) => Err((false, format!("network: {e}"))),
    }
}

/// Fetch a fresh snapshot: reads tokens from disk and tries each against the
/// usage endpoint until one succeeds.
pub fn fetch() -> Result<Snapshot, String> {
    let mut tokens = load_tokens()?;

    // Try the previously-working token first.
    if let Some(good) = LAST_GOOD.lock().unwrap().clone() {
        if let Some(pos) = tokens.iter().position(|t| t.access == good) {
            tokens.swap(0, pos);
        }
    }

    let mut last_err = "no token tried".to_string();
    for tok in &tokens {
        match request_usage(&tok.access) {
            Ok(usage) => {
                *LAST_GOOD.lock().unwrap() = Some(tok.access.clone());
                return Ok(build_snapshot(usage, tok));
            }
            Err((retryable, msg)) => {
                let head: String = tok.access.chars().take(20).collect();
                dbg_log(&format!("token {head}… -> {msg} (retryable={retryable})"));
                last_err = msg;
                if !retryable {
                    break;
                }
            }
        }
    }
    *LAST_GOOD.lock().unwrap() = None;
    Err(format!("usage request: {last_err}"))
}

fn build_snapshot(usage: UsageResponse, tok: &OauthToken) -> Snapshot {
    let mut limits = Vec::new();

    if let Some(w) = usage.five_hour {
        if let Some(reset) = parse_ts(&w.resets_at) {
            limits.push(Limit {
                title: "5-hour limit".into(),
                used_percent: w.utilization.unwrap_or(0.0) as f32,
                window_start: reset - chrono::Duration::hours(5),
                resets_at: reset,
            });
        }
    }
    if let Some(w) = usage.seven_day {
        if let Some(reset) = parse_ts(&w.resets_at) {
            limits.push(Limit {
                title: "Weekly · all models".into(),
                used_percent: w.utilization.unwrap_or(0.0) as f32,
                window_start: reset - chrono::Duration::days(7),
                resets_at: reset,
            });
        }
    }

    let plan = pretty_plan(tok.subscription.as_deref(), tok.tier.as_deref());
    Snapshot {
        family: Family::Claude,
        plan,
        limits,
    }
}

fn pretty_plan(sub: Option<&str>, tier: Option<&str>) -> String {
    match sub {
        Some("max") => {
            if let Some(t) = tier {
                if t.contains("20x") {
                    return "Claude Max 20×".into();
                }
                if t.contains("5x") {
                    return "Claude Max 5×".into();
                }
            }
            "Claude Max".into()
        }
        Some("pro") => "Claude Pro".into(),
        Some(other) => format!("Claude {other}"),
        None => "Claude".into(),
    }
}
