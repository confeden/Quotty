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
const REQUEST_BODY: &str = r#"{"metadata":{"ideName":"antigravity"}}"#;
/// Antigravity quota windows roll over every 5 hours.
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

pub fn fetch() -> Result<Snapshot, FetchError> {
    let eps = candidates();
    if eps.is_empty() {
        return Err("Antigravity не запущен".into());
    }

    diag(&format!("antigravity: {} endpoint(s) to try", eps.len()));
    let mut last_err = String::new();
    for ep in &eps {
        let started = std::time::Instant::now();
        match call(ep) {
            Ok(status) => {
                remember(ep);
                diag(&format!(
                    "antigravity: 200 from port {} in {} ms",
                    ep.port,
                    started.elapsed().as_millis()
                ));
                return Ok(build_snapshot(status));
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

fn call(ep: &Endpoint) -> Result<UserStatus, String> {
    let url = format!("https://127.0.0.1:{}{RPC_PATH}", ep.port);
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
    let parsed: StatusResponse = resp.into_json().map_err(|e| format!("parse status: {e}"))?;
    parsed.user_status.ok_or("нет userStatus в ответе".into())
}

/// Model label → the quota group it draws from. All Gemini models (Pro *and*
/// Flash) spend from one shared pool; the third-party models have their own.
fn group_of(label: &str) -> usize {
    if label.to_lowercase().contains("gemini") {
        0
    } else {
        1
    }
}

const GROUP_TITLES: [&str; 2] = ["Gemini", "Claude / GPT"];

fn build_snapshot(status: UserStatus) -> Snapshot {
    let now = Utc::now();
    // Per group: worst (smallest) remaining fraction and earliest reset, so the
    // bar shows the limit the user will actually hit first.
    let mut worst: [Option<(f32, Option<DateTime<Utc>>)>; GROUP_TITLES.len()] = [None, None];

    for cfg in status
        .cascade_model_config_data
        .into_iter()
        .flat_map(|d| d.client_model_configs)
    {
        let (Some(label), Some(q)) = (cfg.label, cfg.quota_info) else {
            continue;
        };
        let Some(remaining) = as_f64(q.remaining_fraction.as_ref()) else {
            continue;
        };
        let reset = as_time(q.reset_time.as_ref());
        let g = group_of(&label);
        let remaining = remaining.clamp(0.0, 1.0) as f32;
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
        // The RPC gives no window start; quotas roll over every 5 hours, so the
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
