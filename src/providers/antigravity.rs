//! Antigravity module: asks the locally running Antigravity language server for
//! the account's model quotas — the same RPC the IDE's own usage panel uses.
//!
//! There is no cloud endpoint we could call on our own: quota lives behind the
//! language server, which every Antigravity surface (2.0 app, IDE, `agy` CLI)
//! starts. It listens on 127.0.0.1 over HTTPS with a **self-signed** cert and
//! authenticates callers with a per-launch CSRF token, so we need both the port
//! and that token, and we must skip certificate verification for it.

use super::{dbg_log, diag, Family, FetchError, Limit, LimitWindow, Snapshot};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

const RPC_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";
/// The call the IDE's own usage panel makes. Unlike `GetUserStatus` — which
/// carries one quota per model, always the 5-hour one — this returns every
/// window the account is metered on, the weekly limit included.
const QUOTA_PATH: &str = "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const REQUEST_BODY: &str = r#"{"metadata":{"ideName":"antigravity"}}"#;
/// Fallback window length, used only where nothing states one: `GetUserStatus`
/// never does, and its quotas are the 5-hour ones.
const WINDOW_SECS: i64 = 5 * 3600;

// ---------------------------------------------------------------------------
// Finding the language server
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Endpoint {
    port: u16,
    csrf: String,
    /// Freshness of the file we learned this from — newest is tried first.
    seen: SystemTime,
}

/// Every place a running language server announces itself, best first.
fn candidates() -> Vec<Endpoint> {
    let mut out: Vec<Endpoint> = Vec::new();

    // 1. Whatever answered last time — a running server keeps its port and
    //    token until it restarts, so the steady state is a single request.
    if let Some(ep) = last_good() {
        out.push(ep);
    }

    // 2. The running processes themselves: port from the TCP table, token from
    //    the command line. The only source that works for Antigravity IDE,
    //    which writes neither a daemon descriptor nor an Electron log.
    from_processes(&mut out);

    // 3. Daemon descriptors: `~/.gemini/<surface>/daemon/ls_*.json`, written by
    //    the language server with its own port and token.
    let mut files: Vec<Endpoint> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        let gemini = home.join(".gemini");
        if let Ok(subdirs) = std::fs::read_dir(&gemini) {
            for sub in subdirs.flatten() {
                scan_daemon_dir(&sub.path().join("daemon"), &mut files);
            }
        }
    }

    // 4. The Electron app's own log, which records the language server command
    //    line (with `--csrf_token`) and the resulting local URL.
    for base in appdata_bases() {
        for app in ["Antigravity", "Antigravity IDE"] {
            if let Some(e) = from_main_log(&base.join(app).join("logs").join("main.log")) {
                files.push(e);
            }
        }
    }
    files.sort_by(|a, b| b.seen.cmp(&a.seen));
    out.append(&mut files);

    // Both files outlive the server they describe — the daemon descriptor on
    // this machine names a port from months ago. Anything nobody is listening
    // on would only buy a connect timeout.
    let listening = crate::winproc::listening_ports();
    if !listening.is_empty() {
        out.retain(|e| listening.iter().any(|(_, port)| *port == e.port));
    }

    out.dedup_by(|a, b| a.port == b.port && a.csrf == b.csrf);
    out
}

/// Language servers that are alive right now. Works across elevation: the IDE
/// may run "as administrator" while Quotty doesn't, and both the TCP table and
/// `PROCESS_QUERY_LIMITED_INFORMATION` still answer.
fn from_processes(out: &mut Vec<Endpoint>) {
    use crate::winproc;

    // The 2.0 app ships `language_server.exe`, the IDE
    // `language_server_windows_x64.exe`.
    let mut servers: Vec<(u32, String)> = winproc::snapshot()
        .into_iter()
        .filter(|p| p.name.starts_with("language_server"))
        .filter_map(|p| Some((p.pid, csrf_of(p.pid)?)))
        .collect();
    if servers.is_empty() {
        return;
    }
    servers.sort_by(|a, b| b.0.cmp(&a.0)); // newest first

    let listening = winproc::listening_ports();
    for (pid, csrf) in servers {
        let mut ports: Vec<u16> = listening
            .iter()
            .filter(|(owner, _)| *owner == pid)
            .map(|(_, port)| *port)
            .collect();
        ports.sort_unstable();
        ports.dedup();
        // The server opens HTTPS and HTTP on adjacent ports, HTTPS the lower of
        // the two; its other port (LSP) speaks no TLS and would eat a timeout.
        let chosen: Vec<u16> = match ports.iter().copied().find(|p| ports.contains(&(p + 1))) {
            Some(https) => vec![https],
            None => ports.iter().copied().take(2).collect(),
        };
        for port in chosen {
            out.push(Endpoint {
                port,
                csrf: csrf.clone(),
                seen: SystemTime::now(),
            });
        }
    }
}

/// `--csrf_token <uuid>` out of a language server's command line.
fn csrf_of(pid: u32) -> Option<String> {
    let cmd = crate::winproc::command_line(pid)?;
    let at = cmd.rfind("--csrf_token")? + "--csrf_token".len();
    let token: String = cmd[at..]
        .trim_start_matches([' ', '=', '"'])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    (!token.is_empty()).then_some(token)
}

fn appdata_bases() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    if let Ok(a) = std::env::var("APPDATA") {
        v.push(PathBuf::from(a));
    }
    if let Some(c) = dirs::config_dir() {
        v.push(c);
    }
    v.dedup();
    v
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DaemonInfo {
    #[serde(default)]
    https_port: Option<u16>,
    #[serde(default)]
    csrf_token: Option<String>,
}

fn scan_daemon_dir(dir: &Path, out: &mut Vec<Endpoint>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(info) = serde_json::from_str::<DaemonInfo>(&raw) else {
            continue;
        };
        if let (Some(port), Some(csrf)) = (info.https_port, info.csrf_token) {
            out.push(Endpoint {
                port,
                csrf,
                seen: e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
}

/// Pull the most recent `--csrf_token <uuid>` and `127.0.0.1:<port>` out of the
/// Electron log. Both are written at every app start, newest last.
fn from_main_log(path: &Path) -> Option<Endpoint> {
    let meta = std::fs::metadata(path).ok()?;
    let raw = std::fs::read(path).ok()?;
    // The tail is enough and keeps this cheap on a log that grows for months.
    let tail = &raw[raw.len().saturating_sub(256 * 1024)..];
    let text = String::from_utf8_lossy(tail);

    let csrf = last_after(&text, "--csrf_token ", |c| {
        c.is_ascii_alphanumeric() || c == '-'
    })?;
    let port: u16 = last_after(&text, "127.0.0.1:", |c| c.is_ascii_digit())?
        .parse()
        .ok()?;
    Some(Endpoint {
        port,
        csrf,
        seen: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
    })
}

/// Text right after the last occurrence of `marker`, taking chars while `keep`.
fn last_after(haystack: &str, marker: &str, keep: impl Fn(char) -> bool) -> Option<String> {
    let at = haystack.rfind(marker)? + marker.len();
    let s: String = haystack[at..].chars().take_while(|c| keep(*c)).collect();
    (!s.is_empty()).then_some(s)
}

// ---------------------------------------------------------------------------
// Local HTTPS with a self-signed certificate
// ---------------------------------------------------------------------------

/// Accepts any certificate. Only ever used for 127.0.0.1, where the language
/// server generates a throw-away self-signed cert at every launch, and where
/// the CSRF token — not the certificate — is what authenticates the call.
#[derive(Debug)]
struct AcceptAnyCert(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .expect("rustls default protocol versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyCert(provider)))
            .with_no_client_auth();
        ureq::AgentBuilder::new()
            .tls_config(Arc::new(cfg))
            // A wrong guess should cost little: a dead port refuses instantly,
            // and a live port that speaks no TLS is capped by the read timeout.
            .timeout_connect(std::time::Duration::from_secs(2))
            .timeout(std::time::Duration::from_secs(5))
            .build()
    })
}

/// The endpoint that answered last. A server keeps its port and token for its
/// whole life, so remembering the winner keeps the steady state at one request.
static LAST_GOOD: OnceLock<std::sync::Mutex<Option<Endpoint>>> = OnceLock::new();

fn last_good_slot() -> &'static std::sync::Mutex<Option<Endpoint>> {
    LAST_GOOD.get_or_init(|| std::sync::Mutex::new(None))
}

fn last_good() -> Option<Endpoint> {
    last_good_slot().lock().ok()?.clone()
}

fn remember(ep: &Endpoint) {
    if let Ok(mut slot) = last_good_slot().lock() {
        *slot = Some(ep.clone());
    }
}

// ---------------------------------------------------------------------------
// RPC response
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    #[serde(default)]
    user_status: Option<UserStatus>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserStatus {
    #[serde(default)]
    plan_status: Option<PlanStatus>,
    #[serde(default)]
    user_tier: Option<UserTier>,
    #[serde(default)]
    cascade_model_config_data: Option<CascadeData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanStatus {
    #[serde(default)]
    plan_info: Option<PlanInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanInfo {
    #[serde(default)]
    plan_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserTier {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CascadeData {
    #[serde(default)]
    client_model_configs: Vec<ModelConfig>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelConfig {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    quota_info: Option<QuotaInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaInfo {
    /// Numeric in practice, but the gateway has been seen to stringify numbers.
    #[serde(default)]
    remaining_fraction: Option<serde_json::Value>,
    /// RFC3339 string, or epoch millis as a number.
    #[serde(default)]
    reset_time: Option<serde_json::Value>,
}

/// `RetrieveUserQuotaSummary`: one group per model pool, one bucket per window.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaSummaryResponse {
    #[serde(default)]
    response: Option<QuotaSummary>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaSummary {
    #[serde(default)]
    groups: Vec<QuotaGroup>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaGroup {
    /// "Gemini Models" / "Claude and GPT models" — what `group_of` reads.
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    buckets: Vec<QuotaBucket>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaBucket {
    /// "5h" / "weekly" — the only place Antigravity ever states a window length.
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    remaining_fraction: Option<serde_json::Value>,
    #[serde(default)]
    reset_time: Option<serde_json::Value>,
}

pub fn fetch() -> Result<Snapshot, FetchError> {
    let eps = candidates();
    if eps.is_empty() {
        return Err("Antigravity не запущен".into());
    }

    diag(&format!("antigravity: {} endpoint(s) to try", eps.len()));
    let mut last_err = String::new();
    for ep in &eps {
        let started = std::time::Instant::now();
        match user_status(ep) {
            Ok(status) => {
                remember(ep);
                diag(&format!(
                    "antigravity: 200 from port {} in {} ms",
                    ep.port,
                    started.elapsed().as_millis()
                ));
                // The quota summary is where the weekly window lives, but it is
                // a newer method than `GetUserStatus`: a language server that
                // does not know it must still get its 5-hour rows drawn.
                let summary = match quota_summary(ep) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        diag(&format!("antigravity: quota summary -> {e}"));
                        None
                    }
                };
                return Ok(build_snapshot(status, summary));
            }
            Err(e) => {
                diag(&format!("antigravity: port {} -> {e}", ep.port));
                last_err = e;
            }
        }
    }
    dbg_log(&format!(
        "antigravity: {} endpoint(s) tried, last error: {last_err}",
        eps.len()
    ));
    Err(format!("Antigravity не отвечает ({last_err})").into())
}

fn call<T: serde::de::DeserializeOwned>(ep: &Endpoint, path: &str) -> Result<T, String> {
    let url = format!("https://127.0.0.1:{}{path}", ep.port);
    let resp = agent()
        .post(&url)
        .set("Content-Type", "application/json")
        .set("X-Codeium-Csrf-Token", &ep.csrf)
        .set("Connect-Protocol-Version", "1")
        .send_string(REQUEST_BODY)
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("status {code}"),
            e => format!("{e}"),
        })?;
    resp.into_json().map_err(|e| format!("parse: {e}"))
}

fn user_status(ep: &Endpoint) -> Result<UserStatus, String> {
    call::<StatusResponse>(ep, RPC_PATH)?
        .user_status
        .ok_or("нет userStatus в ответе".into())
}

fn quota_summary(ep: &Endpoint) -> Result<QuotaSummary, String> {
    call::<QuotaSummaryResponse>(ep, QUOTA_PATH)?
        .response
        .ok_or("нет response в сводке квот".into())
}

/// Model label — or the summary's group name — → the quota group it draws from.
/// All Gemini models (Pro *and* Flash) spend from one shared pool; the
/// third-party models have their own.
fn group_of(label: &str) -> usize {
    if label.to_lowercase().contains("gemini") {
        0
    } else {
        1
    }
}

const GROUP_TITLES: [&str; 2] = ["Gemini", "Claude / GPT"];

/// Seconds in a bucket's `window`, which the summary states as a word ("5h",
/// "weekly"). An unknown word falls back to the 5-hour roll-over.
fn window_secs(window: &str) -> i64 {
    let w = window.trim().to_ascii_lowercase();
    match w.as_str() {
        "daily" => 24 * 3600,
        "weekly" => 7 * 24 * 3600,
        "monthly" => 30 * 24 * 3600,
        _ => span_suffix(&w).unwrap_or(WINDOW_SECS),
    }
}

/// "5h" / "7d" → seconds.
fn span_suffix(s: &str) -> Option<i64> {
    if let Some(n) = s.strip_suffix('h') {
        return n.parse::<i64>().ok().map(|n| n * 3600);
    }
    if let Some(n) = s.strip_suffix('d') {
        return n.parse::<i64>().ok().map(|n| n * 24 * 3600);
    }
    None
}

/// How a window is named in a row title: whole days as "7d", otherwise hours.
fn window_label(secs: i64) -> String {
    let day = 24 * 3600;
    if secs >= day && secs % day == 0 {
        format!("{}d", secs / day)
    } else {
        format!("{}h", (secs / 3600).max(1))
    }
}

/// A fraction the service left out is a **zero** fraction: protobuf JSON omits
/// default values, so an exhausted pool arrives as a bucket with a reset time
/// and no `remainingFraction` at all. Reading that as "unknown" and dropping the
/// row hid exactly the limit the user most needed to see.
fn remaining_of(v: Option<&serde_json::Value>) -> f32 {
    as_f64(v).unwrap_or(0.0).clamp(0.0, 1.0) as f32
}

/// One row per (model pool × window): Gemini 5h, Gemini 7d, Claude / GPT 5h,
/// Claude / GPT 7d. Ordered by pool, then shortest window first — the same
/// reading order as Claude's "5-hour limit" above "Weekly".
fn limits_from_summary(summary: QuotaSummary, now: DateTime<Utc>) -> Vec<Limit> {
    let mut rows: Vec<(usize, i64, Limit)> = Vec::new();
    for group in summary.groups {
        let g = group_of(group.display_name.as_deref().unwrap_or_default());
        for b in group.buckets {
            let secs = window_secs(b.window.as_deref().unwrap_or_default());
            // Here the window length is stated, so the start is a fact rather
            // than the assumption `ending_at` has to make elsewhere (I5).
            let window = as_time(b.reset_time.as_ref())
                .map(|r| LimitWindow::ending_at(r, chrono::Duration::seconds(secs), now));
            rows.push((
                g,
                secs,
                Limit {
                    title: format!("{} · {}", GROUP_TITLES[g], window_label(secs)),
                    used_percent: (1.0 - remaining_of(b.remaining_fraction.as_ref())) * 100.0,
                    window,
                },
            ));
        }
    }
    rows.sort_by_key(|(g, secs, _)| (*g, *secs));
    rows.into_iter().map(|(_, _, lim)| lim).collect()
}

/// Quotas as `GetUserStatus` carries them: one per model, always the 5-hour
/// window, no weekly row at all. Only reached against a language server that
/// does not answer `RetrieveUserQuotaSummary`.
fn limits_from_model_configs(data: Option<CascadeData>, now: DateTime<Utc>) -> Vec<Limit> {
    // Per group: worst (smallest) remaining fraction and earliest reset, so the
    // bar shows the limit the user will actually hit first.
    let mut worst: [Option<(f32, Option<DateTime<Utc>>)>; GROUP_TITLES.len()] = [None, None];

    for cfg in data.into_iter().flat_map(|d| d.client_model_configs) {
        let (Some(label), Some(q)) = (cfg.label, cfg.quota_info) else {
            continue;
        };
        let reset = as_time(q.reset_time.as_ref());
        let g = group_of(&label);
        let remaining = remaining_of(q.remaining_fraction.as_ref());
        match &mut worst[g] {
            None => worst[g] = Some((remaining, reset)),
            Some((r, t)) => {
                *r = r.min(remaining);
                if let Some(new) = reset {
                    if t.map_or(true, |cur| new < cur) {
                        *t = Some(new);
                    }
                }
            }
        }
    }

    let mut limits = Vec::new();
    for (g, entry) in worst.iter().enumerate() {
        let Some((remaining, reset)) = entry else {
            continue;
        };
        let resets_at = reset.unwrap_or(now + chrono::Duration::seconds(WINDOW_SECS));
        // No window start here either; quotas roll over every 5 hours, so the
        // start is derived from that. A reset further out than 5 hours means the
        // assumption does not hold for this row — `ending_at` then leaves the
        // start unplaced rather than pretending the window began now, which put
        // the time marker at zero and made any spend look like overspending.
        limits.push(Limit {
            title: GROUP_TITLES[g].to_string(),
            used_percent: (1.0 - remaining) * 100.0,
            window: Some(LimitWindow::ending_at(
                resets_at,
                chrono::Duration::seconds(WINDOW_SECS),
                now,
            )),
        });
    }
    limits
}

fn build_snapshot(status: UserStatus, summary: Option<QuotaSummary>) -> Snapshot {
    let now = Utc::now();
    let limits = summary
        .map(|s| limits_from_summary(s, now))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| limits_from_model_configs(status.cascade_model_config_data, now));

    let tier = status.user_tier.and_then(|t| t.name).or_else(|| {
        status
            .plan_status
            .and_then(|p| p.plan_info)
            .and_then(|p| p.plan_name)
    });
    let plan = match tier {
        Some(t) if !t.is_empty() => format!("Antigravity · {t}"),
        _ => "Antigravity".to_string(),
    };

    Snapshot {
        family: Family::Antigravity,
        plan,
        limits,
    }
}

fn as_f64(v: Option<&serde_json::Value>) -> Option<f64> {
    match v? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn as_time(v: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    match v? {
        serde_json::Value::String(s) => DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc)),
        serde_json::Value::Number(n) => {
            let n = n.as_i64()?;
            // Seconds or milliseconds, depending on the surface.
            let secs = if n > 100_000_000_000 { n / 1000 } else { n };
            Utc.timestamp_opt(secs, 0).single()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed verbatim from a live `RetrieveUserQuotaSummary` on this machine:
    /// two pools, each with a weekly and a 5-hour bucket, weekly listed first,
    /// and the exhausted pool reporting `remainingFraction: 0`.
    const SUMMARY: &str = r#"{"response":{"groups":[
      {"displayName":"Gemini Models","buckets":[
        {"bucketId":"gemini-weekly","window":"weekly","remainingFraction":0.7057507,
         "resetTime":"2026-09-10T20:23:48Z"},
        {"bucketId":"gemini-5h","window":"5h","remainingFraction":0.9228146,
         "resetTime":"2026-09-08T03:43:32Z"}]},
      {"displayName":"Claude and GPT models","buckets":[
        {"bucketId":"3p-weekly","window":"weekly","remainingFraction":0,
         "resetTime":"2026-09-08T08:03:48Z"},
        {"bucketId":"3p-5h","window":"5h","remainingFraction":1,"disabled":true,
         "resetTime":"2026-09-08T03:43:22Z"}]}]}}"#;

    fn parse(raw: &str) -> QuotaSummary {
        serde_json::from_str::<QuotaSummaryResponse>(raw)
            .expect("summary parses")
            .response
            .expect("summary has a response")
    }

    #[test]
    fn the_summary_becomes_one_row_per_pool_and_window() {
        let now = Utc.with_ymd_and_hms(2026, 9, 8, 0, 15, 0).unwrap();
        let limits = limits_from_summary(parse(SUMMARY), now);

        let titles: Vec<&str> = limits.iter().map(|l| l.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Gemini · 5h",
                "Gemini · 7d",
                "Claude / GPT · 5h",
                "Claude / GPT · 7d"
            ],
            "pools in order, shortest window first"
        );
        // The weekly row is the point of the whole call: it must carry the
        // service's own reset time, not a 5-hour one derived from now.
        let weekly = &limits[1];
        assert_eq!(
            weekly.window.unwrap().resets_at,
            Utc.with_ymd_and_hms(2026, 9, 10, 20, 23, 48).unwrap()
        );
        // Stated window length → a real, placeable start (I5 no longer guesses).
        assert_eq!(
            weekly.window.unwrap().start,
            Some(Utc.with_ymd_and_hms(2026, 9, 3, 20, 23, 48).unwrap())
        );
        assert!((weekly.used_percent - 29.42).abs() < 0.01);
        assert!(
            (limits[3].used_percent - 100.0).abs() < 0.01,
            "3p weekly is gone"
        );
    }

    /// Protobuf JSON drops a zero, so the pool that matters most arrives with no
    /// `remainingFraction` at all — that must read as 100 % spent, not as a
    /// missing row (the bug that hid "Claude / GPT" once it ran out).
    #[test]
    fn a_missing_fraction_is_an_exhausted_pool() {
        let raw = r#"{"response":{"groups":[{"displayName":"Claude and GPT models",
          "buckets":[{"window":"weekly","resetTime":"2026-09-08T08:03:48Z"}]}]}}"#;
        let now = Utc.with_ymd_and_hms(2026, 9, 8, 0, 15, 0).unwrap();
        let limits = limits_from_summary(parse(raw), now);
        assert_eq!(limits.len(), 1);
        assert_eq!(limits[0].used_percent, 100.0);

        // Same rule on the legacy `GetUserStatus` path.
        let data: CascadeData = serde_json::from_str(
            r#"{"clientModelConfigs":[{"label":"Claude Opus 4.6 (Thinking)",
                "quotaInfo":{"resetTime":"2026-09-08T08:03:48Z"}}]}"#,
        )
        .unwrap();
        let legacy = limits_from_model_configs(Some(data), now);
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].title, "Claude / GPT");
        assert_eq!(legacy[0].used_percent, 100.0);
    }

    /// Live probe against whatever language server is running right now — the
    /// production path end to end. `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn probe_live_quota() {
        for ep in candidates() {
            println!("endpoint :{}", ep.port);
            match quota_summary(&ep) {
                Ok(s) => {
                    println!("  groups: {}", s.groups.len());
                    for g in &s.groups {
                        println!("  {:?} buckets={}", g.display_name, g.buckets.len());
                    }
                    for l in limits_from_summary(s, Utc::now()) {
                        println!(
                            "  row {:<20} {:.1}% {:?}",
                            l.title, l.used_percent, l.window
                        );
                    }
                }
                Err(e) => println!("  summary error: {e}"),
            }
        }
    }

    #[test]
    fn window_words_become_lengths_and_labels() {
        assert_eq!(window_secs("5h"), 5 * 3600);
        assert_eq!(window_secs("weekly"), 7 * 24 * 3600);
        assert_eq!(window_secs("Weekly"), 7 * 24 * 3600);
        assert_eq!(window_secs("daily"), 24 * 3600);
        assert_eq!(window_secs("12h"), 12 * 3600);
        assert_eq!(window_secs(""), WINDOW_SECS, "unknown → the old assumption");
        assert_eq!(window_secs("fortnightly"), WINDOW_SECS);

        assert_eq!(window_label(5 * 3600), "5h");
        assert_eq!(window_label(7 * 24 * 3600), "7d");
        assert_eq!(window_label(30 * 24 * 3600), "30d");
    }
}
