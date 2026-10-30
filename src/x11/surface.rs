// What the app's own windows (settings, tray menu) need: showing what was
// drawn, waiting for events with a timeout, the desktop's scale and theme.

use std::os::fd::AsRawFd;
use std::time::Duration;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Gcontext, ImageFormat, PropMode, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::Res;
use crate::ui::Canvas;

// Waits until the X server sends something, or `timeout` passes. Events
// already received must have been taken with `poll_for_event` first.
pub fn wait(conn: &RustConnection, timeout: Option<Duration>) -> Res<()> {
    conn.flush()?;
    let mut fd = libc::pollfd { fd: conn.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ms = timeout.map_or(-1, |t| t.as_millis().min(i32::MAX as u128) as i32);
    // SAFETY: a single valid pollfd.
    let result = unsafe { libc::poll(&mut fd, 1, ms) };
    if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

// From the Xft.dpi resource, as set by the desktop's scaling.
pub fn scale(conn: &RustConnection, root: Window) -> f32 {
    let resources = conn
        .get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 16)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| String::from_utf8_lossy(&r.value).into_owned())
        .unwrap_or_default();
    let dpi = resources
        .lines()
        .find_map(|l| l.strip_prefix("Xft.dpi:"))
        .and_then(|v| v.trim().parse::<f32>().ok())
        .unwrap_or(96.0);
    (dpi / 96.0).clamp(1.0, 4.0)
}

fn gsetting(key: &str) -> String {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", key])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase())
        .unwrap_or_default()
}

// Whether the desktop prefers dark colours.
pub fn dark() -> bool {
    if let Ok(theme) = std::env::var("GTK_THEME") {
        return theme.to_lowercase().contains("dark");
    }
    gsetting("color-scheme").contains("dark") || gsetting("gtk-theme").contains("dark")
}

// Shows what was drawn on `canvas` in `window`, which has the screen's
// 24-bit visual (or a 32-bit one), in strips that fit in a request.
pub fn present(conn: &RustConnection, window: Window, gc: Gcontext, depth: u8, canvas: &Canvas) -> Res<()> {
    let (width, height) = (canvas.pixmap.width() as usize, canvas.pixmap.height() as usize);
    let data = canvas.bgra();
    let row = width * 4;
    let rows = ((conn.maximum_request_bytes() - 64) / row).clamp(1, height);
    for y in (0..height).step_by(rows) {
        let n = rows.min(height - y);
        conn.put_image(ImageFormat::Z_PIXMAP, window, gc, width as u16, n as u16, 0, y as i16, 0, depth,
                       &data[y * row..(y + n) * row])?;
    }
    conn.flush()?;
    Ok(())
}

// The window's title, class and icon, for the window manager and taskbars.
pub fn describe(conn: &RustConnection, window: Window, atoms: &super::Atoms, title: &str) -> Res<()> {
    conn.change_property8(PropMode::REPLACE, window, AtomEnum::WM_NAME, AtomEnum::STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, window, atoms._NET_WM_NAME, atoms.UTF8_STRING, title.as_bytes())?;
    conn.change_property8(PropMode::REPLACE, window, AtomEnum::WM_CLASS, AtomEnum::STRING,
                          b"souris-facile-pingouin\0Souris-facile-pingouin\0")?;
    conn.change_property32(PropMode::REPLACE, window, atoms._NET_WM_PID, AtomEnum::CARDINAL, &[std::process::id()])?;
    let mut icon = Vec::new();
    for size in [16, 24, 32, 48, 64, 128] {
        icon.extend([size, size]);
        icon.extend(crate::icon::argb(size));
    }
    conn.change_property32(PropMode::REPLACE, window, atoms._NET_WM_ICON, AtomEnum::CARDINAL, &icon)?;
    Ok(())
}
