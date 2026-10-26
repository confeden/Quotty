//! System-tray icon and its menu.
//!
//! Deliberately minimal: everything with options of its own (opacity, sources,
//! poll interval) lives in the settings window, so this stays a two-click menu.
//!
//! Windows draws it with `tray-icon`. Linux exposes a StatusNotifierItem over
//! D-Bus (`ksni`), which KDE shows natively and GNOME shows through its
//! AppIndicator extension. Either way the menu reports choices through
//! `drain()`, and the app acts on them on its own thread.

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Settings,
    Refresh,
    Autostart,
    Quit,
}

#[cfg(windows)]
mod imp {
    use super::{egui, Action};
    use crate::icon;
    use std::sync::mpsc::{channel, Receiver};
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    pub struct Tray {
        tray: TrayIcon,
        autostart_item: CheckMenuItem,
        rx: Receiver<Action>,
    }

    impl Tray {
        pub fn new(autostart_on: bool, wake: egui::Context) -> Result<Self, String> {
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

            // Menu choices come through this handler, which also wakes the UI so
            // a choice is applied at once, not on the next timer tick.
            let ids = [
                (autostart_item.id().clone(), Action::Autostart),
                (settings.id().clone(), Action::Settings),
                (refresh.id().clone(), Action::Refresh),
                (quit.id().clone(), Action::Quit),
            ];
            let (tx, rx) = channel();
            MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
                if let Some((_, action)) = ids.iter().find(|(id, _)| *id == ev.id) {
                    let _ = tx.send(*action);
                }
                wake.request_repaint();
            }));

            Ok(Self {
                tray,
                autostart_item,
                rx,
            })
        }

        pub fn drain(&self) -> Vec<Action> {
            self.rx.try_iter().collect()
        }

        pub fn set_autostart_checked(&self, on: bool) {
            self.autostart_item.set_checked(on);
        }

        /// Hover text — also where a pending update is announced.
        pub fn set_tooltip(&self, text: &str) {
            let _ = self.tray.set_tooltip(Some(text));
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{egui, Action};
    use crate::icon;
    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::{CheckmarkItem, StandardItem};
    use ksni::MenuItem;
    use std::sync::mpsc::{channel, Receiver, Sender};

    /// What the StatusNotifierItem host shows. Its callbacks run on the D-Bus
    /// thread, so they only forward the choice and wake the UI.
    struct Sni {
        autostart: bool,
        tooltip: String,
        tx: Sender<Action>,
        wake: egui::Context,
    }

    impl Sni {
        fn send(&self, action: Action) {
            let _ = self.tx.send(action);
            self.wake.request_repaint();
        }
    }

    impl ksni::Tray for Sni {
        fn id(&self) -> String {
            "quotty".into()
        }

        fn title(&self) -> String {
            "Quotty".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            // StatusNotifierItem wants ARGB32 in network byte order; `icon::rgba`
            // is RGBA in memory order.
            let data = icon::rgba()
                .chunks_exact(4)
                .flat_map(|p| [p[3], p[0], p[1], p[2]])
                .collect();
            vec![ksni::Icon {
                width: icon::SIZE as i32,
                height: icon::SIZE as i32,
                data,
            }]
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                icon_name: String::new(),
                icon_pixmap: Vec::new(),
                title: self.tooltip.clone(),
                description: String::new(),
            }
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            vec![
                MenuItem::Standard(StandardItem {
                    label: "Quotty".into(),
                    enabled: false,
                    ..Default::default()
                }),
                MenuItem::Separator,
                MenuItem::Checkmark(CheckmarkItem {
                    label: "Автозапуск (ярлык)".into(),
                    checked: self.autostart,
                    activate: Box::new(|s: &mut Self| s.send(Action::Autostart)),
                    ..Default::default()
                }),
                MenuItem::Standard(StandardItem {
                    label: "Настройки…".into(),
                    activate: Box::new(|s: &mut Self| s.send(Action::Settings)),
                    ..Default::default()
                }),
                MenuItem::Standard(StandardItem {
                    label: "Обновить сейчас".into(),
                    activate: Box::new(|s: &mut Self| s.send(Action::Refresh)),
                    ..Default::default()
                }),
                MenuItem::Separator,
                MenuItem::Standard(StandardItem {
                    label: "Выход".into(),
                    activate: Box::new(|s: &mut Self| s.send(Action::Quit)),
                    ..Default::default()
                }),
            ]
        }
    }

    pub struct Tray {
        handle: Handle<Sni>,
        rx: Receiver<Action>,
    }

    impl Tray {
        /// Fails when there is no D-Bus session or no StatusNotifierItem host
        /// to show the icon; the strip works without it.
        pub fn new(autostart_on: bool, wake: egui::Context) -> Result<Self, String> {
            let (tx, rx) = channel();
            let handle = Sni {
                autostart: autostart_on,
                tooltip: "Quotty".into(),
                tx,
                wake,
            }
            .spawn()
            .map_err(|e| format!("tray: {e}"))?;
            Ok(Self { handle, rx })
        }

        pub fn drain(&self) -> Vec<Action> {
            self.rx.try_iter().collect()
        }

        pub fn set_autostart_checked(&self, on: bool) {
            self.handle.update(|s| s.autostart = on);
        }

        pub fn set_tooltip(&self, text: &str) {
            let text = text.to_string();
            self.handle.update(|s| s.tooltip = text);
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::{egui, Action};

    pub struct Tray;

    impl Tray {
        pub fn new(_autostart_on: bool, _wake: egui::Context) -> Result<Self, String> {
            Err("no tray on this platform".into())
        }
        pub fn drain(&self) -> Vec<Action> {
            Vec::new()
        }
        pub fn set_autostart_checked(&self, _on: bool) {}
        pub fn set_tooltip(&self, _text: &str) {}
    }
}

pub use imp::Tray;
