// The X11 backend: XInput2 to see the keys and pointer everywhere and to
// swallow the clicks, scrolls and arrow keys used, EWMH to move windows,
// RandR for the displays.

mod displays;
mod input;
mod overlays;
mod settings_window;
mod surface;
mod tray;

use std::collections::HashMap;
use std::error::Error;

use x11rb::connection::Connection;
use x11rb::properties::{WmClass, WmSizeHints};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask, GetPropertyReply, KeyButMask, Window,
};
use x11rb::rust_connection::{ReplyError, RustConnection};
use x11rb::{CURRENT_TIME, NONE};

use crate::geometry::{Point, Rect, Size};
use crate::platform::{Display, Platform, WindowInfo};

pub use displays::print_displays;
pub use input::run;
pub use settings_window::run as run_settings;
pub use tray::spawn as spawn_tray;

pub type Res<T> = Result<T, Box<dyn Error>>;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        _NET_CLIENT_LIST,
        _NET_CLIENT_LIST_STACKING,
        _NET_CURRENT_DESKTOP,
        _NET_WORKAREA,
        _NET_FRAME_EXTENTS,
        _GTK_FRAME_EXTENTS,
        _NET_WM_STATE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WM_WINDOW_TYPE_TOOLBAR,
        _NET_WM_ALLOWED_ACTIONS,
        _NET_WM_ACTION_MOVE,
        _NET_WM_ACTION_RESIZE,
        _NET_WM_DESKTOP,
        _NET_WM_STRUT,
        _NET_WM_STRUT_PARTIAL,
        _NET_MOVERESIZE_WINDOW,
        _NET_ACTIVE_WINDOW,
        EDID,
        NATURAL_SCROLLING: b"libinput Natural Scrolling Enabled",
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_ICON,
        UTF8_STRING,
        MANAGER,
        _NET_SYSTEM_TRAY_OPCODE,
        _NET_SYSTEM_TRAY_VISUAL,
        _XEMBED_INFO,
        _NET_WM_WINDOW_TYPE_POPUP_MENU,
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        INSTANCE: b"_SOURIS_FACILE_PINGOUIN",
        SETTINGS_INSTANCE: b"_SOURIS_FACILE_PINGOUIN_SETTINGS",
    }
}

// How far the visible frame reaches beyond the client window on each side:
// the window manager's decorations, minus client-side shadows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Extents {
    left: f64,
    right: f64,
    top: f64,
    bottom: f64,
    // The client-side shadows on the left and top, between the window
    // manager's frame and the visible frame.
    shadow_left: f64,
    shadow_top: f64,
}

impl Extents {
    fn frame(&self, client: Rect) -> Rect {
        Rect::new(
            client.x - self.left,
            client.y - self.top,
            client.width + self.left + self.right,
            client.height + self.top + self.bottom,
        )
    }

    #[cfg(test)]
    fn client(&self, frame: Rect) -> Rect {
        Rect::new(
            frame.x + self.left,
            frame.y + self.top,
            frame.width - self.left - self.right,
            frame.height - self.top - self.bottom,
        )
    }

    fn width(&self) -> f64 {
        self.left + self.right
    }

    fn height(&self) -> f64 {
        self.top + self.bottom
    }
}

// What's known about a top-level client window.
struct Client {
    window: Window,
    frame: Rect,
    extents: Extents,
    state: Vec<u32>,
    window_type: Option<u32>,
    allowed_actions: Option<Vec<u32>>,
    desktop: Option<u32>,
    class: Option<String>,
    hints: Option<WmSizeHints>,
}

fn values32(reply: Result<GetPropertyReply, ReplyError>) -> Option<Vec<u32>> {
    let reply = reply.ok()?;
    let values: Vec<u32> = reply.value32()?.collect();
    (!values.is_empty()).then_some(values)
}

pub struct X11 {
    conn: RustConnection,
    screen: usize,
    root: Window,
    atoms: Atoms,
    // By client window, from the last time it was looked at.
    extents: HashMap<Window, Extents>,
    overlays: overlays::Overlays,
}

impl X11 {
    pub fn connect() -> Res<X11> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms::new(&conn)?.reply()?;
        let overlays = overlays::Overlays::new(&conn, screen)?;
        Ok(X11 { conn, screen, root, atoms, extents: HashMap::new(), overlays })
    }

    fn root_property(&self, atom: u32) -> Option<Vec<u32>> {
        let cookie = self.conn.get_property(false, self.root, atom, AtomEnum::ANY, 0, 4096).ok()?;
        values32(cookie.reply())
    }

    fn current_desktop(&self) -> Option<u32> {
        self.root_property(self.atoms._NET_CURRENT_DESKTOP)?.first().copied()
    }

    // Looks the windows up, with one round trip for all of them. None for
    // the windows that are gone.
    fn clients(&self, windows: &[Window]) -> Vec<Option<Client>> {
        let a = &self.atoms;
        let c = &self.conn;
        let prop = |w, atom| c.get_property(false, w, atom, AtomEnum::ANY, 0, 1024);
        let cookies: Vec<_> = windows
            .iter()
            .map(|&w| {
                (
                    c.get_geometry(w),
                    c.translate_coordinates(w, self.root, 0, 0),
                    prop(w, a._NET_FRAME_EXTENTS),
                    prop(w, a._GTK_FRAME_EXTENTS),
                    prop(w, a._NET_WM_STATE),
                    prop(w, a._NET_WM_WINDOW_TYPE),
                    prop(w, a._NET_WM_ALLOWED_ACTIONS),
                    prop(w, a._NET_WM_DESKTOP),
                    WmClass::get(c, w),
                    WmSizeHints::get_normal_hints(c, w),
                )
            })
            .collect();
        windows
            .iter()
            .zip(cookies)
            .map(|(&window, (geometry, translate, net, gtk, state, kind, allowed, desktop, class, hints))| {
                let geometry = geometry.ok()?.reply().ok()?;
                let origin = translate.ok()?.reply().ok()?;
                let net = net.ok().and_then(|c| values32(c.reply())).filter(|v| v.len() == 4).unwrap_or(vec![0; 4]);
                let gtk = gtk.ok().and_then(|c| values32(c.reply())).filter(|v| v.len() == 4).unwrap_or(vec![0; 4]);
                // _NET_FRAME_EXTENTS and _GTK_FRAME_EXTENTS are left, right, top, bottom.
                let side = |i: usize| net[i] as f64 - gtk[i] as f64;
                let extents = Extents {
                    left: side(0),
                    right: side(1),
                    top: side(2),
                    bottom: side(3),
                    shadow_left: gtk[0] as f64,
                    shadow_top: gtk[2] as f64,
                };
                let client =
                    Rect::new(origin.dst_x as f64, origin.dst_y as f64, geometry.width as f64, geometry.height as f64);
                Some(Client {
                    window,
                    frame: extents.frame(client),
                    extents,
                    state: state.ok().and_then(|c| values32(c.reply())).unwrap_or_default(),
                    window_type: kind.ok().and_then(|c| values32(c.reply())).and_then(|v| v.first().copied()),
                    allowed_actions: allowed.ok().and_then(|c| values32(c.reply())),
                    desktop: desktop.ok().and_then(|c| values32(c.reply())).and_then(|v| v.first().copied()),
                    class: class
                        .ok()
                        .and_then(|c| c.reply().ok().flatten())
                        .map(|c| String::from_utf8_lossy(c.class()).into_owned())
                        .filter(|c| !c.is_empty()),
                    hints: hints.ok().and_then(|c| c.reply().ok().flatten()),
                })
            })
            .collect()
    }

    // The client windows, bottom to top.
    fn stacking(&self) -> Vec<Window> {
        self.root_property(self.atoms._NET_CLIENT_LIST_STACKING)
            .or_else(|| self.root_property(self.atoms._NET_CLIENT_LIST))
            .unwrap_or_default()
    }

    // The visible clients on the current desktop, bottom to top.
    fn visible_clients(&self) -> Vec<Client> {
        let desktop = self.current_desktop();
        self.clients(&self.stacking())
            .into_iter()
            .flatten()
            .filter(|c| {
                !c.state.contains(&self.atoms._NET_WM_STATE_HIDDEN)
                    && match (c.desktop, desktop) {
                        (Some(d), Some(current)) => d == current || d == 0xFFFF_FFFF,
                        _ => true,
                    }
            })
            .collect()
    }

    // Ordinary windows, not panels, docks or the desktop.
    fn is_ordinary(&self, client: &Client) -> bool {
        let a = &self.atoms;
        client.window_type.is_none_or(|t| {
            [a._NET_WM_WINDOW_TYPE_NORMAL, a._NET_WM_WINDOW_TYPE_DIALOG, a._NET_WM_WINDOW_TYPE_UTILITY,
             a._NET_WM_WINDOW_TYPE_TOOLBAR]
                .contains(&t)
        })
    }

    fn allows(&self, client: &Client, action: u32) -> bool {
        client.allowed_actions.as_ref().is_none_or(|actions| actions.contains(&action))
    }

    fn send_client_message(&self, window: Window, message: u32, data: [u32; 5]) {
        let event = ClientMessageEvent::new(32, window, message, data);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        let _ = self.conn.send_event(false, self.root, mask, event);
    }

    // Moves the visible frame to `origin`, and sets its size if given.
    fn move_resize(&self, window: Window, origin: Point, size: Option<Size>) -> bool {
        let e = self.extents.get(&window).copied().unwrap_or_default();
        // North-west gravity: the position is the top-left of the window
        // manager's frame, which holds the client-side shadows. (Static
        // gravity would depend on the client's border before it was framed.)
        const NORTH_WEST_GRAVITY: u32 = 1;
        let (x, y) = (origin.x - e.shadow_left, origin.y - e.shadow_top);
        let (width, height) = size.map_or((0.0, 0.0), |s| (s.width - e.width(), s.height - e.height()));
        // The flags say which values are set, and that a pager-like tool asks.
        let mut flags = NORTH_WEST_GRAVITY | 1 << 8 | 1 << 9 | 2 << 12;
        if size.is_some() {
            if width < 1.0 || height < 1.0 {
                return true;
            }
            flags |= 1 << 10 | 1 << 11;
        }
        let data = [flags, x as i32 as u32, y as i32 as u32, width as u32, height as u32];
        self.send_client_message(window, self.atoms._NET_MOVERESIZE_WINDOW, data);
        let _ = self.conn.flush();
        true
    }

    fn query_pointer(&self) -> Option<(Point, u16)> {
        let reply = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((Point::new(reply.root_x as f64, reply.root_y as f64), u16::from(reply.mask)))
    }
}

impl Platform for X11 {
    type Window = Window;

    fn pointer(&mut self) -> Point {
        self.query_pointer().map_or(Point::default(), |p| p.0)
    }

    fn warp_pointer(&mut self, point: Point) {
        let _ = self.conn.warp_pointer(NONE, self.root, 0, 0, 0, 0, point.x as i16, point.y as i16);
        let _ = self.conn.flush();
    }

    fn button_held(&mut self) -> bool {
        self.query_pointer().is_some_and(|p| p.1 & u16::from(KeyButMask::BUTTON1) != 0)
    }

    fn window_at(&mut self, point: Point) -> Option<Window> {
        self.visible_clients().into_iter().rev().find(|c| c.frame.contains(point)).map(|c| c.window)
    }

    fn window_info(&mut self, window: &Window) -> Option<WindowInfo> {
        let client = self.clients(&[*window]).pop().flatten()?;
        let a = &self.atoms;
        self.extents.insert(*window, client.extents);
        let usable = self.is_ordinary(&client)
            && !client.state.contains(&a._NET_WM_STATE_FULLSCREEN)
            && !client.state.contains(&a._NET_WM_STATE_HIDDEN);
        let e = client.extents;
        let hints = client.hints.as_ref();
        let to_frame = |(w, h): (i32, i32)| Size::new(w as f64 + e.width(), h as f64 + e.height());
        let min_size = hints.and_then(|h| h.min_size).map(to_frame).unwrap_or_default();
        let max_size = hints.and_then(|h| h.max_size).filter(|&(w, h)| w > 0 && h > 0).map(to_frame);
        let fixed = max_size.is_some_and(|max| max == min_size);
        Some(WindowInfo {
            frame: client.frame,
            can_move: usable && self.allows(&client, a._NET_WM_ACTION_MOVE),
            can_resize: usable && !fixed && self.allows(&client, a._NET_WM_ACTION_RESIZE),
            maximized: client.state.contains(&a._NET_WM_STATE_MAXIMIZED_VERT)
                || client.state.contains(&a._NET_WM_STATE_MAXIMIZED_HORZ),
            app: client.class,
            min_size,
            max_size,
        })
    }

    fn other_windows(&mut self, window: &Window) -> Vec<Rect> {
        self.visible_clients()
            .into_iter()
            .filter(|c| c.window != *window && self.is_ordinary(c))
            .map(|c| c.frame)
            .collect()
    }

    fn displays(&mut self) -> Vec<Display> {
        displays::displays(self).unwrap_or_else(|e| {
            eprintln!("Can't list the displays: {e}");
            Vec::new()
        })
    }

    fn set_frame(&mut self, window: &Window, frame: Rect) -> bool {
        self.move_resize(*window, frame.origin(), Some(frame.size()))
    }

    fn set_origin(&mut self, window: &Window, origin: Point) -> bool {
        self.move_resize(*window, origin, None)
    }

    fn unmaximize(&mut self, window: &Window) {
        const REMOVE: u32 = 0;
        let a = &self.atoms;
        let data = [REMOVE, a._NET_WM_STATE_MAXIMIZED_VERT, a._NET_WM_STATE_MAXIMIZED_HORZ, 2, 0];
        self.send_client_message(*window, a._NET_WM_STATE, data);
    }

    fn raise(&mut self, window: &Window) {
        self.send_client_message(*window, self.atoms._NET_ACTIVE_WINDOW, [2, CURRENT_TIME, 0, 0, 0]);
    }

    fn show_grid(&mut self, area: Rect, columns: usize, rows: usize) {
        self.overlays.show_grid(&self.conn, self.root, area, columns, rows);
    }

    fn hide_grid(&mut self) {
        self.overlays.hide_grid(&self.conn);
    }

    fn show_dimmer(&mut self, below: &Window) {
        let displays = self.displays();
        self.overlays.show_dimmer(&self.conn, self.root, *below, &displays);
    }

    fn hide_dimmer(&mut self) {
        self.overlays.hide_dimmer(&self.conn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extents_convert_both_ways() {
        // A server-side title bar, or client-side shadows.
        let title_bar = Extents { top: 37.0, ..Extents::default() };
        let shadows = Extents { left: -23.0, right: -23.0, top: -15.0, bottom: -31.0, shadow_left: 23.0, shadow_top: 15.0 };
        for e in [title_bar, shadows] {
            let client = Rect::new(100.0, 200.0, 640.0, 480.0);
            assert_eq!(e.client(e.frame(client)), client);
        }
        assert_eq!(shadows.frame(Rect::new(100.0, 200.0, 640.0, 480.0)), Rect::new(123.0, 215.0, 594.0, 434.0));
    }
}
