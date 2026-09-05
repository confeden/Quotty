//! Which tool family the user is working in right now.
//!
//! The rule is "whatever was in the foreground last": we look at the foreground
//! window's process, and — when that process is only a host (a terminal, or an
//! editor with an embedded terminal) — at what it is running underneath, so a
//! CLI session counts as its own family.

use crate::providers::Family;
use crate::winproc;

/// Result of checking the foreground window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActiveStatus {
    /// Whether the foreground app is an AI tool or supported editor.
    pub is_ai: bool,
    /// Detected family, if a specific tool or CLI was recognized.
    pub family: Option<Family>,
}

/// Exe name → family, for windows that *are* the tool.
fn direct(exe: &str) -> Option<Family> {
    let lower = exe.to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    match base {
        "claude" => Some(Family::Claude),
        "codex" | "chatgpt" => Some(Family::Codex),
        "antigravity" | "antigravity ide" | "agy" => Some(Family::Antigravity),
        _ => None,
    }
}

/// Supported code editors that count as AI-active workspaces.
fn is_editor(exe: &str) -> bool {
    let lower = exe.to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    matches!(
        base,
        "code"
            | "code - exploration"
            | "code - insiders"
            | "vscodium"
            | "cursor"
            | "windsurf"
            | "zed"
    )
}

/// Console hosts own the window but not the CLI: `conhost.exe` is a *child* of
/// the program whose console it draws, so the search has to start one level up.
fn is_console_host(exe: &str) -> bool {
    let lower = exe.to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    matches!(base, "conhost" | "openconsole")
}

/// Windows that merely *host* a terminal / shell: look at their process tree instead.
fn is_terminal(exe: &str) -> bool {
    let lower = exe.to_lowercase();
    let base = lower.strip_suffix(".exe").unwrap_or(&lower);
    matches!(
        base,
        "windowsterminal"
            | "windowsterminalpreview"
            | "openconsole"
            | "conhost"
            | "cmd"
            | "powershell"
            | "pwsh"
            | "bash"
            | "sh"
            | "mintty"
            | "alacritty"
            | "wezterm-gui"
            | "hyper"
            | "tabby"
            | "kitty"
            | "warp"
            | "ghostty"
            | "conemu64"
            | "cmder"
    )
}

/// Remembers the last answer so we only walk the process list when the
/// foreground window changes (or a host window may have started a new CLI).
pub struct Detector {
    last_pid: u32,
    last_status: ActiveStatus,
    /// Set when the answer came from a process-tree walk — those go stale
    /// without the foreground window ever changing (`claude` started in the
    /// terminal you are already looking at), so they are re-checked on a timer.
    from_tree: bool,
    checked_at: f64,
}

impl Default for Detector {
    fn default() -> Self {
        Self {
            last_pid: 0,
            last_status: ActiveStatus {
                is_ai: true,
                family: None,
            },
            from_tree: false,
            checked_at: f64::MIN,
        }
    }
}

impl Detector {
    /// `now` is any monotonically growing clock in seconds (egui's frame time).
    pub fn poll(&mut self, now: f64) -> ActiveStatus {
        let Some(pid) = foreground_pid() else {
            return self.last_status;
        };

        // Don't treat Quotty itself as deactivating the AI state (clicking settings or dragging)
        if pid == std::process::id() {
            return self.last_status;
        }

        let stale = self.from_tree && now - self.checked_at > 1.5;
        if pid == self.last_pid && !stale {
            return self.last_status;
        }

        let exe = match process_name(pid) {
            Some(name) => name.to_lowercase(),
            None => return self.last_status,
        };

        let (status, from_tree) = if let Some(f) = direct(&exe) {
            (
                ActiveStatus {
                    is_ai: true,
                    family: Some(f),
                },
                false,
            )
        } else if is_editor(&exe) {
            // Editor is active: is_ai is always true. Check if an AI CLI is running in its terminal.
            let cli_family = family_in_tree(pid, is_console_host(&exe));
            (
                ActiveStatus {
                    is_ai: true,
                    family: cli_family,
                },
                true,
            )
        } else if is_terminal(&exe) {
            // Terminal is active: is_ai is true ONLY if an AI CLI is running underneath.
            let cli_family = family_in_tree(pid, is_console_host(&exe));
            let is_ai = cli_family.is_some();
            (
                ActiveStatus {
                    is_ai,
                    family: cli_family,
                },
                true,
            )
        } else {
            (
                ActiveStatus {
                    is_ai: false,
                    family: None,
                },
                false,
            )
        };

        self.last_pid = pid;
        self.last_status = status;
        self.from_tree = from_tree;
        self.checked_at = now;
        status
    }
}

/// Spawns a lightweight watcher thread that checks foreground PID every 200ms.
/// Helper to check if a process is an AI tool or editor.
pub fn is_ai_process(pid: u32) -> bool {
    let Some(exe) = process_name(pid) else {
        return false;
    };
    let exe = exe.to_lowercase();
    if direct(&exe).is_some() || is_editor(&exe) {
        return true;
    }
    if is_terminal(&exe) {
        return family_in_tree(pid, is_console_host(&exe)).is_some();
    }
    false
}

/// If the foreground app changes, it immediately wakes egui so the strip can show/hide promptly.
pub fn spawn_watcher(ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let mut last_pid = 0u32;
        let mut tick = 0u32;
        loop {
            tick = tick.wrapping_add(1);
            if let Some(pid) = foreground_pid() {
                if (pid != last_pid && pid != std::process::id()) || tick % 5 == 0 {
                    last_pid = pid;

                    #[cfg(windows)]
                    {
                        let hwnd =
                            crate::app::NATIVE_HWND.load(std::sync::atomic::Ordering::Relaxed);
                        if hwnd != 0 && is_ai_process(pid) {
                            crate::app::set_native_visible(hwnd, true);
                        }
                    }

                    ctx.request_repaint();
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });
}

// ---------------------------------------------------------------------------
// Win32
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn foreground_pid() -> Option<u32> {
    use windows::core::w;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }

        // Check if this is an ApplicationFrameWindow hosting a UWP app (e.g. ChatGPT)
        let mut class_buf = [0u16; 64];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_buf[..len as usize]);
            if class_name == "ApplicationFrameWindow" {
                if let Ok(child) = FindWindowExW(
                    hwnd,
                    HWND(std::ptr::null_mut()),
                    w!("Windows.UI.Core.CoreWindow"),
                    None,
                ) {
                    if !child.0.is_null() {
                        let mut child_pid = 0u32;
                        GetWindowThreadProcessId(child, Some(&mut child_pid));
                        if child_pid != 0 {
                            return Some(child_pid);
                        }
                    }
                }
            }
        }

        Some(pid)
    }
}

fn process_name(pid: u32) -> Option<String> {
    let full = winproc::image_path(pid)?;
    Some(full.rsplit(['\\', '/']).next()?.to_string())
}

/// Walk everything started under `root` and return the family of the most
/// recently started match (highest pid — good enough to prefer the CLI you just
/// launched over one sitting in another tab). With `hop_parent`, the walk starts
/// at the root's parent instead: a console host's CLI is its sibling, not its
/// child.
fn family_in_tree(root: u32, hop_parent: bool) -> Option<Family> {
    let procs = winproc::snapshot();
    let root = if hop_parent {
        procs
            .iter()
            .find(|p| p.pid == root)
            .map(|p| p.parent)
            .filter(|p| *p != 0)
            .unwrap_or(root)
    } else {
        root
    };

    // Breadth-first over the descendants of `root`.
    let mut frontier = vec![root];
    let mut seen = vec![root];
    let mut best: Option<(u32, Family)> = None;
    while let Some(parent) = frontier.pop() {
        for p in &procs {
            if p.parent != parent || seen.contains(&p.pid) {
                continue;
            }
            seen.push(p.pid);
            frontier.push(p.pid);
            if let Some(f) = direct(&p.name) {
                if best.map_or(true, |(bp, _)| p.pid > bp) {
                    best = Some((p.pid, f));
                }
            }
        }
        // A runaway tree would only cost us time; the process list is finite.
        if seen.len() > 4096 {
            break;
        }
    }
    best.map(|(_, f)| f)
}

#[cfg(not(windows))]
fn foreground_pid() -> Option<u32> {
    None
}
