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

#[cfg(not(windows))]
pub fn snapshot() -> Vec<Proc> {
    Vec::new()
}
#[cfg(not(windows))]
pub fn image_path(_pid: u32) -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn command_line(_pid: u32) -> Option<String> {
    None
}
#[cfg(not(windows))]
pub fn listening_ports() -> Vec<(u32, u16)> {
    Vec::new()
}
