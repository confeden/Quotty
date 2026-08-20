//! Desktop and autostart shortcuts (.lnk), per the request to wire autostart
//! "by adding a shortcut".

use mslnk::ShellLink;
use std::path::PathBuf;

fn exe() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

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
