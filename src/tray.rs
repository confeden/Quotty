//! System-tray icon and its right-click menu.
//!
//! Deliberately minimal: everything with options of its own (opacity, sources,
//! poll interval) lives in the settings window, so this stays a two-click menu.

use crate::icon;
use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub struct Tray {
    tray: TrayIcon,
    pub autostart_item: CheckMenuItem,
    pub id_autostart: MenuId,
    pub id_settings: MenuId,
    pub id_refresh: MenuId,
    pub id_quit: MenuId,
}

impl Tray {
    pub fn new(autostart_on: bool) -> Result<Self, String> {
        let ic = Icon::from_rgba(icon::rgba(), icon::SIZE, icon::SIZE)
            .map_err(|e| format!("icon: {e}"))?;

        let menu = Menu::new();

        let header = MenuItem::new("Quotty", false, None);
        let autostart_item = CheckMenuItem::new("Автозапуск (ярлык)", true, autostart_on, None);
        let settings = MenuItem::new("Настройки…", true, None);
        let refresh = MenuItem::new("Обновить сейчас", true, None);
        let quit = MenuItem::new("Выход", true, None);

        let _ = menu.append_items(&[
            &header,
            &PredefinedMenuItem::separator(),
            &autostart_item,
            &settings,
            &refresh,
            &PredefinedMenuItem::separator(),
            &quit,
        ]);

        let tray = TrayIconBuilder::new()
            .with_tooltip("Quotty")
            .with_icon(ic)
            .with_menu(Box::new(menu))
            .build()
            .map_err(|e| format!("tray build: {e}"))?;

        Ok(Self {
            tray,
            id_autostart: autostart_item.id().clone(),
            autostart_item,
            id_settings: settings.id().clone(),
            id_refresh: refresh.id().clone(),
            id_quit: quit.id().clone(),
        })
    }

    /// Hover text — also where a pending update is announced.
    pub fn set_tooltip(&self, text: &str) {
        let _ = self.tray.set_tooltip(Some(text));
    }
}
