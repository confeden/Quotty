//! Small Win32 process helpers shared by the active-tool detector and the
//! Antigravity provider.
//!
//! Everything here works across the elevation boundary: a normal-user Quotty
//! can still see the name, command line and listening ports of a tool started
//! "as administrator", because `PROCESS_QUERY_LIMITED_INFORMATION` and the TCP
//! table are readable up the integrity ladder. Reading another process's memory
//! would not be, which is why the command line comes from
//! `NtQueryInformationProcess` and not from the PEB.

pub struct Proc {
    pub pid: u32,
    pub parent: u32,
    /// Lower-cased file name, e.g. `claude.exe`.
    pub name: String,
}

/// Every process on the machine we are allowed to see.
#[cfg(windows)]
pub fn snapshot() -> Vec<Proc> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
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
                out.push(Proc {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    name: String::from_utf16_lossy(&entry.szExeFile[..end]).to_lowercase(),
                });
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// Full image path of a process, or None when it is gone or out of reach.
#[cfg(windows)]
pub fn image_path(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// The command line a process was started with.
#[cfg(windows)]
pub fn command_line(pid: u32) -> Option<String> {
    use windows::Wdk::System::Threading::{
        NtQueryInformationProcess, ProcessCommandLineInformation,
    };
    use windows::Win32::Foundation::{CloseHandle, UNICODE_STRING};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        // First call only sizes the buffer (it returns INFO_LENGTH_MISMATCH).
        let mut need = 0u32;
        let _ = NtQueryInformationProcess(
            handle,
            ProcessCommandLineInformation,
            std::ptr::null_mut(),
            0,
            &mut need,
        );
        let cap = (need as usize).clamp(1024, 64 * 1024);
        let mut buf = vec![0u8; cap];
        let status = NtQueryInformationProcess(
            handle,
            ProcessCommandLineInformation,
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
            &mut need,
        );
        let _ = CloseHandle(handle);
        if status.is_err() {
            return None;
        }
        let text = &*(buf.as_ptr() as *const UNICODE_STRING);
        if text.Buffer.is_null() || text.Length == 0 {
            return None;
        }
        let chars = std::slice::from_raw_parts(text.Buffer.0, (text.Length / 2) as usize);
        Some(String::from_utf16_lossy(chars))
    }
}

/// (pid, port) for every IPv4 TCP socket in LISTEN state.
#[cfg(windows)]
pub fn listening_ports() -> Vec<(u32, u16)> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    };
    const AF_INET: u32 = 2;

    unsafe {
        let mut size = 0u32;
        let _ = GetExtendedTcpTable(
            None,
            &mut size,
            false,
            AF_INET,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if size == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; size as usize];
        if GetExtendedTcpTable(
            Some(buf.as_mut_ptr() as *mut _),
            &mut size,
            false,
            AF_INET,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        ) != 0
        {
            return Vec::new();
        }
        let table = &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
        rows.iter()
            .map(|r| {
                // dwLocalPort is network byte order in the low half.
                let port = (((r.dwLocalPort & 0xFF) << 8) | ((r.dwLocalPort >> 8) & 0xFF)) as u16;
                (r.dwOwningPid, port)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Linux: /proc. Everything is readable for the processes of the same user, which
// is the only set Quotty needs (the AI tools run under the logged-in user).
// ---------------------------------------------------------------------------

/// Process name as the kernel records it (`comm`, at most 15 bytes), lower-cased.
#[cfg(target_os = "linux")]
pub fn name_of(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim_end().to_lowercase())
}

#[cfg(target_os = "linux")]
pub fn snapshot() -> Vec<Proc> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let name = name_of(pid)?;
            // `/proc/<pid>/stat` is `pid (comm) state ppid ...`; `comm` may hold
            // spaces or parentheses, so split after the last `)`.
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            let parent = stat
                .rsplit_once(')')?
                .1
                .split_whitespace()
                .nth(1)?
                .parse()
                .ok()?;
            Some(Proc { pid, parent, name })
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub fn image_path(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(target_os = "linux")]
pub fn command_line(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let text = raw
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

/// (pid, port) for every IPv4 TCP socket in LISTEN state, as `/proc/net/tcp`
/// describes them, with the owning pid found through the `socket:[inode]` links.
#[cfg(target_os = "linux")]
pub fn listening_ports() -> Vec<(u32, u16)> {
    let Ok(table) = std::fs::read_to_string("/proc/net/tcp") else {
        return Vec::new();
    };
    // inode -> port, for sockets in LISTEN (state 0A).
    let listening: Vec<(u64, u16)> = table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.get(3)? != &"0A" {
                return None;
            }
            let port = u16::from_str_radix(cols.get(1)?.rsplit_once(':')?.1, 16).ok()?;
            Some((cols.get(9)?.parse().ok()?, port))
        })
        .collect();
    if listening.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in dir.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue; // another user's process: not ours to read
        };
        for fd in fds.flatten() {
            let Ok(link) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let Some(inode) = link
                .to_string_lossy()
                .strip_prefix("socket:[")
                .and_then(|s| s.strip_suffix(']'))
                .and_then(|s| s.parse::<u64>().ok())
            else {
                continue;
            };
            out.extend(
                listening
                    .iter()
                    .filter(|(ino, _)| *ino == inode)
                    .map(|(_, port)| (pid, *port)),
            );
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Other platforms: nothing to look at.
// ---------------------------------------------------------------------------

#[cfg(not(any(windows, target_os = "linux")))]
pub fn name_of(_pid: u32) -> Option<String> {
    None
}
#[cfg(not(any(windows, target_os = "linux")))]
pub fn snapshot() -> Vec<Proc> {
    Vec::new()
}
#[cfg(not(any(windows, target_os = "linux")))]
pub fn image_path(_pid: u32) -> Option<String> {
    None
}
#[cfg(not(any(windows, target_os = "linux")))]
pub fn command_line(_pid: u32) -> Option<String> {
    None
}
#[cfg(not(any(windows, target_os = "linux")))]
pub fn listening_ports() -> Vec<(u32, u16)> {
    Vec::new()
}

/// Name of the executable behind `pid`, lower-cased: the file name on Windows,
/// the kernel's `comm` on Linux (a native CLI installed under a versioned path
/// still reports its own name there).
#[cfg(windows)]
pub fn name_of(pid: u32) -> Option<String> {
    let full = image_path(pid)?;
    Some(full.rsplit(['\\', '/']).next()?.to_lowercase())
}
