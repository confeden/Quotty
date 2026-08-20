//! Which tool family the user is working in right now.
//!
//! The rule is "whatever was in the foreground last": we look at the foreground
//! window's process, and — when that process is only a host (a terminal, or an
//! editor with an embedded terminal) — at what it is running underneath, so a
//! CLI session counts as its own family.

use crate::providers::Family;

/// Exe name → family, for windows that *are* the tool.
fn direct(exe: &str) -> Option<Family> {
    match exe {
        "claude.exe" => Some(Family::Claude),
        "codex.exe" | "chatgpt.exe" => Some(Family::Codex),
        "antigravity.exe" | "antigravity ide.exe" | "agy.exe" => Some(Family::Antigravity),
        _ => None,
    }
}

/// Console hosts own the window but not the CLI: `conhost.exe` is a *child* of
/// the program whose console it draws, so the search has to start one level up.
fn is_console_host(exe: &str) -> bool {
    matches!(exe, "conhost.exe" | "openconsole.exe")
}

/// Windows that merely *host* a CLI: look at their process tree instead.
fn is_host(exe: &str) -> bool {
    matches!(
        exe,
        "windowsterminal.exe"
            | "windowsterminalpreview.exe"
            | "openconsole.exe"
            | "conhost.exe"
            | "cmd.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "bash.exe"
            | "sh.exe"
            | "mintty.exe"
            | "alacritty.exe"
            | "wezterm-gui.exe"
            | "hyper.exe"
            | "tabby.exe"
            | "kitty.exe"
            | "warp.exe"
            | "conemu64.exe"
            | "cmder.exe"
            | "code.exe"
            | "cursor.exe"
            | "windsurf.exe"
    )
}

/// Remembers the last answer so we only walk the process list when the
/// foreground window changes (or a host window may have started a new CLI).
pub struct Detector {
    last_pid: u32,
    last: Option<Family>,
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
            last: None,
            from_tree: false,
            checked_at: f64::MIN,
        }
    }
}

impl Detector {
    /// `now` is any monotonically growing clock in seconds (egui's frame time).
    /// Returns `None` when the foreground window belongs to nothing we track —
    /// the caller then keeps showing the previous family.
    pub fn poll(&mut self, now: f64) -> Option<Family> {
        let pid = foreground_pid()?;
        let stale = self.from_tree && now - self.checked_at > 2.0;
        if pid == self.last_pid && !stale {
            return self.last;
        }

        let exe = process_name(pid)?.to_lowercase();
        let (family, from_tree) = match direct(&exe) {
            Some(f) => (Some(f), false),
            None if is_host(&exe) => (family_in_tree(pid, is_console_host(&exe)), true),
            None => (None, false),
        };

        self.last_pid = pid;
        self.last = family;
        self.from_tree = from_tree;
        self.checked_at = now;
        family
    }
}

// ---------------------------------------------------------------------------
// Win32
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn foreground_pid() -> Option<u32> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}

#[cfg(windows)]
fn process_name(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        if !ok {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        Some(full.rsplit(['\\', '/']).next()?.to_string())
    }
}

/// Walk everything started under `root` and return the family of the most
/// recently started match (highest pid — good enough to prefer the CLI you just
/// launched over one sitting in another tab). With `hop_parent`, the walk starts
/// at the root's parent instead: a console host's CLI is its sibling, not its
/// child.
#[cfg(windows)]
fn family_in_tree(root: u32, hop_parent: bool) -> Option<Family> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut procs: Vec<(u32, u32, String)> = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                procs.push((
                    entry.th32ProcessID,
                    entry.th32ParentProcessID,
                    String::from_utf16_lossy(&entry.szExeFile[..end]).to_lowercase(),
                ));
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }

    let root = if hop_parent {
        procs
            .iter()
            .find(|(pid, _, _)| *pid == root)
            .map(|(_, ppid, _)| *ppid)
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
        for (pid, ppid, name) in &procs {
            if *ppid != parent || seen.contains(pid) {
                continue;
            }
            seen.push(*pid);
            frontier.push(*pid);
            if let Some(f) = direct(name) {
                if best.map_or(true, |(bp, _)| *pid > bp) {
                    best = Some((*pid, f));
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
#[cfg(not(windows))]
fn process_name(_pid: u32) -> Option<String> {
    None
}
#[cfg(not(windows))]
fn family_in_tree(_root: u32, _hop_parent: bool) -> Option<Family> {
    None
}
