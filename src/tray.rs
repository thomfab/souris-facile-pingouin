// The tray icon and its menu: a StatusNotifierItem where the desktop shows
// them (KDE, Xfce, Cinnamon, GNOME with the AppIndicator extension...), or
// else an icon in the X11 system tray.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, StandardItem};
use ksni::MenuItem;

use crate::settings::SettingsFile;

pub const NAME: &str = "Souris facile pingouin";
const PROJECT_PAGE: &str = "https://github.com/thomfab/souris-facile-pingouin";

// The project page, with the releases, in the browser.
pub fn open_about() {
    match std::process::Command::new("xdg-open").arg(PROJECT_PAGE).spawn() {
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => eprintln!("Can't open {PROJECT_PAGE}: {e}"),
    }
}

// Opens the settings window, a process of its own.
pub fn open_settings() {
    let exe = std::env::current_exe().unwrap_or_else(|_| "souris-facile-pingouin".into());
    match std::process::Command::new(exe).arg("settings").spawn() {
        // Waited for, so that it doesn't linger as a zombie.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => eprintln!("Can't open the settings: {e}"),
    }
}

// Through the settings file, which the app reads again on its own.
pub fn set_enabled(settings_path: &Path, enabled: bool) {
    let result = SettingsFile::open(settings_path).and_then(|mut file| {
        file.set_bool("enabled", enabled);
        file.save()
    });
    if let Err(e) = result {
        eprintln!("{e}");
    }
}

pub fn quit() -> ! {
    std::process::exit(0)
}

pub struct Item {
    enabled: bool,
    settings_path: PathBuf,
}

impl ksni::Tray for Item {
    // Like the macOS version, a click shows the menu.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        env!("CARGO_PKG_NAME").into()
    }

    fn title(&self) -> String {
        NAME.into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [16, 22, 24, 32, 48, 64]
            .into_iter()
            .map(|size| ksni::Icon {
                width: size as i32,
                height: size as i32,
                data: crate::icon::argb(size).into_iter().flat_map(u32::to_be_bytes).collect(),
            })
            .collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let state = if self.enabled { "" } else { " (off)" };
        ksni::ToolTip { title: format!("{NAME}{state}"), ..Default::default() }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: format!("About {NAME}"),
                icon_name: "help-about".into(),
                activate: Box::new(|_: &mut Item| open_about()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Enabled".into(),
                checked: self.enabled,
                activate: Box::new(|item: &mut Item| {
                    item.enabled = !item.enabled;
                    set_enabled(&item.settings_path, item.enabled);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Settings…".into(),
                activate: Box::new(|_: &mut Item| open_settings()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: format!("Quit {NAME}"),
                icon_name: "application-exit".into(),
                activate: Box::new(|_: &mut Item| quit()),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub enum Tray {
    Item(Handle<Item>),
    // The X11 system tray's icon, which reads this when its menu opens.
    System(Arc<AtomicBool>),
}

impl Tray {
    pub fn start(settings_path: &Path, enabled: bool) -> Tray {
        let item = Item { enabled, settings_path: settings_path.to_path_buf() };
        match item.spawn() {
            Ok(handle) => Tray::Item(handle),
            Err(e) => {
                eprintln!("No StatusNotifierItem support ({e}): using the X11 system tray.");
                let enabled = Arc::new(AtomicBool::new(enabled));
                crate::x11::spawn_tray(settings_path.to_path_buf(), enabled.clone());
                Tray::System(enabled)
            }
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        match self {
            Tray::Item(handle) => drop(handle.update(|item| item.enabled = enabled)),
            Tray::System(state) => state.store(enabled, Ordering::Relaxed),
        }
    }
}
