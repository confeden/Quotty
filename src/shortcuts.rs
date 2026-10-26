//! Desktop and autostart shortcuts.
//!
//! Windows: `.lnk` files. Linux: freedesktop `.desktop` entries, in the Desktop
//! directory and in `~/.config/autostart`.

use std::path::PathBuf;

fn exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod platform {
    use mslnk::ShellLink;
    use std::path::PathBuf;

    use super::exe;

    fn startup_dir() -> Option<PathBuf> {
        // %APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup
        dirs::config_dir().map(|p| {
            p.join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Startup")
        })
    }

    fn startup_lnk() -> Option<PathBuf> {
        startup_dir().map(|d| d.join("Quotty.lnk"))
    }

    fn desktop_lnk() -> Option<PathBuf> {
        dirs::desktop_dir().map(|d| d.join("Quotty.lnk"))
    }

    fn make_lnk(target: &PathBuf, dest: &PathBuf) -> Result<(), String> {
        if let Some(dir) = dest.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut link = ShellLink::new(target).map_err(|e| format!("shell link: {e}"))?;
        if let Some(dir) = target.parent() {
            link.set_working_dir(Some(dir.to_string_lossy().into_owned()));
        }
        link.create_lnk(dest)
            .map_err(|e| format!("create lnk: {e}"))?;
        Ok(())
    }

    /// Create a launch shortcut on the Desktop (idempotent).
    pub fn ensure_desktop_shortcut() -> Result<(), String> {
        let target = exe().ok_or("no exe path")?;
        let dest = desktop_lnk().ok_or("no desktop dir")?;
        if dest.exists() {
            return Ok(());
        }
        make_lnk(&target, &dest)
    }

    /// Create/overwrite the Desktop shortcut unconditionally.
    pub fn force_desktop_shortcut() -> Result<(), String> {
        let target = exe().ok_or("no exe path")?;
        let dest = desktop_lnk().ok_or("no desktop dir")?;
        make_lnk(&target, &dest)
    }

    pub fn is_autostart_enabled() -> bool {
        startup_lnk().map(|p| p.exists()).unwrap_or(false)
    }

    pub fn set_autostart(enabled: bool) -> Result<(), String> {
        let dest = startup_lnk().ok_or("no startup dir")?;
        if enabled {
            let target = exe().ok_or("no exe path")?;
            make_lnk(&target, &dest)
        } else {
            if dest.exists() {
                std::fs::remove_file(&dest).map_err(|e| format!("remove lnk: {e}"))?;
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Linux: .desktop entries. The Desktop copy is marked executable, which is what
// file managers need before they will launch it.
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod platform {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::exe;

    fn autostart_entry() -> Option<PathBuf> {
        dirs::config_dir().map(|c| c.join("autostart").join("Quotty.desktop"))
    }

    fn desktop_entry() -> Option<PathBuf> {
        dirs::desktop_dir().map(|d| d.join("Quotty.desktop"))
    }

    /// The entry body. `Exec` is quoted, so a path with spaces still launches.
    fn entry_text(target: &PathBuf) -> String {
        // `%` starts a field code in `Exec`, so a literal one is written `%%`.
        let quoted = target
            .to_string_lossy()
            .replace('%', "%%")
            .replace('"', "\\\"");
        format!(
            "[Desktop Entry]\nType=Application\nName=Quotty\nComment=AI quota strip\n\
             Exec=\"{quoted}\"\nTerminal=false\nCategories=Utility;\nStartupNotify=false\n"
        )
    }

    fn write_entry(dest: &PathBuf, executable: bool) -> Result<(), String> {
        let target = exe().ok_or("no exe path")?;
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create dir: {e}"))?;
        }
        std::fs::write(dest, entry_text(&target)).map_err(|e| format!("write entry: {e}"))?;
        if executable {
            std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("chmod: {e}"))?;
        }
        Ok(())
    }

    /// Create a launch entry on the Desktop (idempotent).
    pub fn ensure_desktop_shortcut() -> Result<(), String> {
        let dest = desktop_entry().ok_or("no desktop dir")?;
        if dest.exists() {
            return Ok(());
        }
        write_entry(&dest, true)
    }

    /// Create/overwrite the Desktop entry unconditionally.
    pub fn force_desktop_shortcut() -> Result<(), String> {
        let dest = desktop_entry().ok_or("no desktop dir")?;
        write_entry(&dest, true)
    }

    pub fn is_autostart_enabled() -> bool {
        autostart_entry().map(|p| p.exists()).unwrap_or(false)
    }

    pub fn set_autostart(enabled: bool) -> Result<(), String> {
        let dest = autostart_entry().ok_or("no autostart dir")?;
        if enabled {
            write_entry(&dest, false)
        } else {
            if dest.exists() {
                std::fs::remove_file(&dest).map_err(|e| format!("remove entry: {e}"))?;
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Anything else: no shortcut support.
// ---------------------------------------------------------------------------

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    pub fn ensure_desktop_shortcut() -> Result<(), String> {
        Err("shortcuts are not supported on this platform".into())
    }
    pub fn force_desktop_shortcut() -> Result<(), String> {
        Err("shortcuts are not supported on this platform".into())
    }
    pub fn is_autostart_enabled() -> bool {
        false
    }
    pub fn set_autostart(_enabled: bool) -> Result<(), String> {
        Err("autostart is not supported on this platform".into())
    }
}

pub use platform::{
    ensure_desktop_shortcut, force_desktop_shortcut, is_autostart_enabled, set_autostart,
};
