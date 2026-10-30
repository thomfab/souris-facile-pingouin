// The icon in the X11 system tray (XEmbed), for desktops without
// StatusNotifierItem support, with a menu of its own.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateGCAux,
    CreateWindowAux, EventMask, GrabMode, ImageFormat, PropMode, StackMode, Visualid, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_FROM_PARENT, CURRENT_TIME, NONE};

use super::{surface, Res, X11};
use crate::geometry::{Point, Rect};
use crate::tray::{self, NAME};
use crate::ui::{Canvas, Fonts, Rgba, Weight};

const REQUEST_DOCK: u32 = 0;
const PARENT_RELATIVE: u32 = 1;
const XK_ESCAPE: u32 = 0xFF1B;

pub fn spawn(settings_path: PathBuf, enabled: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        if let Err(e) = run(settings_path, enabled) {
            eprintln!("System tray: {e}");
        }
    });
}

#[derive(Clone, Copy, PartialEq)]
enum Entry {
    Enabled,
    Settings,
    Quit,
}

const ITEM: f64 = 30.0;
const SEPARATOR: f64 = 9.0;
const SIZE: f32 = 14.0;

struct Menu {
    window: Window,
    gc: u32,
    items: Vec<(Rect, Entry)>,
    canvas: Canvas,
    hovered: Option<Entry>,
    opened: Instant,
    pressed: bool,
}

struct Icon {
    window: Window,
    gc: u32,
    // A 32-bit visual offered by the tray, for real transparency.
    argb: bool,
    depth: u8,
    size: u16,
}

struct TrayIcon {
    x: X11,
    selection: u32,
    icon: Icon,
    scale: f32,
    fonts: std::rc::Rc<Fonts>,
    dark: bool,
    settings_path: PathBuf,
    enabled: Arc<AtomicBool>,
    menu: Option<Menu>,
}

impl TrayIcon {
    fn manager(&self) -> Window {
        self.x.conn.get_selection_owner(self.selection).ok().and_then(|c| c.reply().ok()).map_or(NONE, |r| r.owner)
    }

    // The tray's preferred visual, if it's a 32-bit one.
    fn argb_visual(&self, manager: Window) -> Option<Visualid> {
        let a = &self.x.atoms;
        let visual = self
            .x
            .conn
            .get_property(false, manager, a._NET_SYSTEM_TRAY_VISUAL, AtomEnum::VISUALID, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()?;
        let screen = &self.x.conn.setup().roots[self.x.screen];
        let depth = screen.allowed_depths.iter().find(|d| d.visuals.iter().any(|v| v.visual_id == visual))?;
        (depth.depth == 32).then_some(visual)
    }

    fn create_icon(&self, manager: Window) -> Res<Icon> {
        let conn = &self.x.conn;
        let window = conn.generate_id()?;
        let events = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::STRUCTURE_NOTIFY;
        let size = 22;
        let (depth, visual, aux) = match self.argb_visual(manager) {
            Some(visual) => {
                let colormap = conn.generate_id()?;
                conn.create_colormap(ColormapAlloc::NONE, colormap, self.x.root, visual)?;
                (32, visual, CreateWindowAux::new().colormap(colormap).background_pixel(0).border_pixel(0))
            }
            None => (COPY_FROM_PARENT as u8, COPY_FROM_PARENT, CreateWindowAux::new().background_pixmap(PARENT_RELATIVE)),
        };
        conn.create_window(depth, window, self.x.root, 0, 0, size, size, 0, WindowClass::INPUT_OUTPUT, visual,
                           &aux.event_mask(events))?;
        surface::describe(conn, window, &self.x.atoms, NAME)?;
        // XEmbed version 0, mapped.
        conn.change_property32(PropMode::REPLACE, window, self.x.atoms._XEMBED_INFO, self.x.atoms._XEMBED_INFO,
                               &[0, 1])?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new().graphics_exposures(0))?;
        let depth = if depth == 32 { 32 } else { conn.setup().roots[self.x.screen].root_depth };
        Ok(Icon { window, gc, argb: depth == 32, depth, size })
    }

    // Asks the tray to embed the icon, if there's one.
    fn dock(&mut self) -> Res<()> {
        let manager = self.manager();
        if manager == NONE {
            return Ok(());
        }
        let old = self.icon.window;
        self.icon = self.create_icon(manager)?;
        let _ = self.x.conn.destroy_window(old);
        let message = ClientMessageEvent::new(32, manager, self.x.atoms._NET_SYSTEM_TRAY_OPCODE,
                                              [CURRENT_TIME, REQUEST_DOCK, self.icon.window, 0, 0]);
        self.x.conn.send_event(false, manager, EventMask::NO_EVENT, message)?;
        self.x.conn.flush()?;
        Ok(())
    }

    fn draw_icon(&self) -> Res<()> {
        let conn = &self.x.conn;
        let icon = &self.icon;
        let size = icon.size.max(1);
        // The app icon a little smaller than the space, like other icons.
        let inner = ((size as f32 * 0.84).round() as u32).max(1);
        let pixmap = crate::icon::pixmap(inner);
        let offset = ((size as u32 - inner) / 2) as usize;
        let mut data = if icon.argb {
            vec![0u8; size as usize * size as usize * 4]
        } else {
            // What the tray drew behind it, to draw over.
            conn.clear_area(false, icon.window, 0, 0, 0, 0)?;
            conn.get_image(ImageFormat::Z_PIXMAP, icon.window, 0, 0, size, size, !0)?.reply()?.data
        };
        if data.len() < size as usize * size as usize * 4 {
            return Ok(());
        }
        for (i, p) in pixmap.pixels().iter().enumerate() {
            let (x, y) = (offset + i % inner as usize, offset + i / inner as usize);
            if x >= size as usize || y >= size as usize {
                continue;
            }
            let d = &mut data[(y * size as usize + x) * 4..][..4];
            let a = p.alpha() as u32;
            let over = |src: u8, dst: u8| (src as u32 + dst as u32 * (255 - a) / 255) as u8;
            d[0] = over(p.blue(), d[0]);
            d[1] = over(p.green(), d[1]);
            d[2] = over(p.red(), d[2]);
            d[3] = over(p.alpha(), d[3]);
        }
        conn.put_image(ImageFormat::Z_PIXMAP, icon.window, icon.gc, size, size, 0, 0, 0, icon.depth, &data)?;
        conn.flush()?;
        Ok(())
    }

    fn colors(&self) -> (Rgba, Rgba, Rgba, Rgba, Rgba) {
        // Background, border, text, highlight, separator.
        if self.dark {
            (Rgba::rgb(0x2E2E33), Rgba::rgb(0x4A4A50), Rgba::rgb(0xF2F2F2), Rgba::rgb(0x3584E4), Rgba::rgb(0x45454B))
        } else {
            (Rgba::rgb(0xFFFFFF), Rgba::rgb(0xC8C8CC), Rgba::rgb(0x2E2E30), Rgba::rgb(0x3584E4), Rgba::rgb(0xE2E2E5))
        }
    }

    fn draw_menu(&mut self) -> Res<()> {
        let enabled = self.enabled.load(Ordering::Relaxed);
        let (background, border, text, highlight, separator) = self.colors();
        let Some(menu) = &mut self.menu else { return Ok(()) };
        let c = &mut menu.canvas;
        let width = c.pixmap.width() as f64 / c.scale as f64;
        let height = c.pixmap.height() as f64 / c.scale as f64;
        c.clear(background);
        c.stroke_rect(Rect::new(0.0, 0.0, width, height), 0.0, 1.0, border);
        let mut previous: Option<Rect> = None;
        for &(r, entry) in &menu.items {
            if previous.is_some_and(|p| r.y - p.max_y() > 1.0) {
                c.fill_rect(Rect::new(10.0, r.y - SEPARATOR / 2.0 - 0.5, width - 20.0, 1.0), 0.0, separator);
            }
            previous = Some(r);
            let hovered = menu.hovered == Some(entry);
            if hovered {
                c.fill_rect(r, 5.0, highlight);
            }
            let color = if hovered { Rgba::rgb(0xFFFFFF) } else { text };
            if entry == Entry::Enabled && enabled {
                c.stroke(r.x + 10.0, r.mid_y(), 1.8, color, |pb| {
                    pb.move_to(0.0, 0.0);
                    pb.line_to(3.5, 3.5);
                    pb.line_to(10.0, -4.0);
                });
            }
            c.text(r.x + 30.0, r.y + 5.0, label(entry).as_str(), SIZE, Weight::Regular, color);
        }
        let depth = self.x.conn.setup().roots[self.x.screen].root_depth;
        surface::present(&self.x.conn, menu.window, menu.gc, depth, &menu.canvas)
    }

    fn open_menu(&mut self, at: Point) -> Res<()> {
        self.close_menu();
        let conn = &self.x.conn;
        let s = self.scale as f64;
        let canvas = Canvas::new(1.0, 1.0, self.scale, self.fonts.clone());
        let entries = [Some(Entry::Enabled), None, Some(Entry::Settings), None, Some(Entry::Quit)];
        let width = entries
            .iter()
            .flatten()
            .map(|&e| canvas.text_width(&label(e), SIZE, Weight::Regular))
            .fold(0.0, f64::max)
            + 30.0 + 24.0 + 12.0;
        let mut items = Vec::new();
        let mut y = 6.0;
        for entry in entries {
            match entry {
                Some(e) => {
                    items.push((Rect::new(6.0, y, width - 12.0, ITEM), e));
                    y += ITEM;
                }
                None => y += SEPARATOR,
            }
        }
        let height = y + 6.0;

        // Next to the pointer, inside its display.
        let displays = super::displays::displays(&self.x).unwrap_or_default();
        let screen = &conn.setup().roots[self.x.screen];
        let area = displays
            .iter()
            .find(|d| d.frame.contains(at))
            .map_or(Rect::new(0.0, 0.0, screen.width_in_pixels as f64, screen.height_in_pixels as f64), |d| d.frame);
        let (w, h) = (width * s, height * s);
        let x = if at.x + w <= area.max_x() { at.x } else { at.x - w }.max(area.x);
        let y = if at.y + h <= area.max_y() { at.y } else { at.y - h }.max(area.y);

        let window = conn.generate_id()?;
        let events = EventMask::EXPOSURE
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::POINTER_MOTION
            | EventMask::KEY_PRESS;
        let aux = CreateWindowAux::new().override_redirect(1).event_mask(events);
        conn.create_window(COPY_FROM_PARENT as u8, window, self.x.root, x as i16, y as i16, w.round() as u16,
                           h.round() as u16, 0, WindowClass::INPUT_OUTPUT, COPY_FROM_PARENT, &aux)?;
        surface::describe(conn, window, &self.x.atoms, NAME)?;
        let popup = self.x.atoms._NET_WM_WINDOW_TYPE_POPUP_MENU;
        conn.change_property32(PropMode::REPLACE, window, self.x.atoms._NET_WM_WINDOW_TYPE, AtomEnum::ATOM, &[popup])?;
        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new().graphics_exposures(0))?;
        conn.map_window(window)?;
        conn.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        // Clicks elsewhere close it.
        let mask = EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION;
        conn.grab_pointer(false, window, mask, GrabMode::ASYNC, GrabMode::ASYNC, NONE, NONE, CURRENT_TIME)?;
        conn.grab_keyboard(false, window, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?;
        let canvas = Canvas::new(width, height, self.scale, self.fonts.clone());
        self.menu = Some(Menu { window, gc, items, canvas, hovered: None, opened: Instant::now(), pressed: false });
        self.draw_menu()
    }

    fn close_menu(&mut self) {
        if let Some(menu) = self.menu.take() {
            let _ = self.x.conn.ungrab_pointer(CURRENT_TIME);
            let _ = self.x.conn.ungrab_keyboard(CURRENT_TIME);
            let _ = self.x.conn.destroy_window(menu.window);
            let _ = self.x.conn.flush();
        }
    }

    fn entry_at(&self, x: i16, y: i16) -> Option<Entry> {
        let menu = self.menu.as_ref()?;
        let p = Point::new(x as f64 / self.scale as f64, y as f64 / self.scale as f64);
        menu.items.iter().find(|(r, _)| r.contains(p)).map(|(_, e)| *e)
    }

    fn choose(&mut self, entry: Entry) {
        self.close_menu();
        match entry {
            Entry::Enabled => {
                let enabled = !self.enabled.load(Ordering::Relaxed);
                self.enabled.store(enabled, Ordering::Relaxed);
                tray::set_enabled(&self.settings_path, enabled);
            }
            Entry::Settings => tray::open_settings(),
            Entry::Quit => tray::quit(),
        }
    }
}

fn label(entry: Entry) -> String {
    match entry {
        Entry::Enabled => "Enabled".into(),
        Entry::Settings => "Settings…".into(),
        Entry::Quit => format!("Quit {NAME}"),
    }
}

fn run(settings_path: PathBuf, enabled: Arc<AtomicBool>) -> Res<()> {
    let x = X11::connect()?;
    let name = format!("_NET_SYSTEM_TRAY_S{}", x.screen);
    let selection = x.conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
    // To hear about trays starting.
    x.conn.change_window_attributes(x.root, &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
        .event_mask(EventMask::STRUCTURE_NOTIFY))?;
    let scale = surface::scale(&x.conn, x.root);
    let fonts = Fonts::load()?;
    let mut tray = TrayIcon {
        x,
        selection,
        icon: Icon { window: NONE, gc: NONE, argb: false, depth: 24, size: 22 },
        scale,
        fonts,
        dark: surface::dark(),
        settings_path,
        enabled,
        menu: None,
    };
    tray.dock()?;
    let keys = super::settings_window::keymap(&tray.x)?;

    loop {
        let event = tray.x.conn.wait_for_event()?;
        let icon = tray.icon.window;
        let menu = tray.menu.as_ref().map(|m| m.window);
        match event {
            Event::ClientMessage(e) if e.type_ == tray.x.atoms.MANAGER && e.data.as_data32()[1] == selection => {
                tray.dock()?;
            }
            Event::ConfigureNotify(e) if e.window == icon => {
                let size = e.width.min(e.height).max(1);
                if size != tray.icon.size {
                    tray.icon.size = size;
                    tray.draw_icon()?;
                }
            }
            Event::Expose(e) if e.window == icon && e.count == 0 => tray.draw_icon()?,
            Event::Expose(e) if Some(e.window) == menu && e.count == 0 => tray.draw_menu()?,
            Event::ButtonPress(e) if e.event == icon => {
                tray.open_menu(Point::new(e.root_x as f64, e.root_y as f64))?;
            }
            Event::MotionNotify(e) if Some(e.event) == menu => {
                let entry = tray.entry_at(e.event_x, e.event_y);
                if let Some(m) = &mut tray.menu
                    && m.hovered != entry
                {
                    m.hovered = entry;
                    tray.draw_menu()?;
                }
            }
            Event::ButtonPress(e) if Some(e.event) == menu => match tray.entry_at(e.event_x, e.event_y) {
                Some(_) => {
                    if let Some(m) = &mut tray.menu {
                        m.pressed = true;
                    }
                }
                None => tray.close_menu(),
            },
            Event::ButtonRelease(e) if Some(e.event) == menu => {
                // Not the release of the click that opened it, unless it
                // was held and dragged onto an item.
                let deliberate = tray.menu.as_ref().is_some_and(|m| m.pressed || m.opened.elapsed() > Duration::from_millis(400));
                if let Some(entry) = tray.entry_at(e.event_x, e.event_y).filter(|_| deliberate) {
                    tray.choose(entry);
                }
            }
            Event::KeyPress(e) if menu.is_some() && keys.iter().any(|&(c, sym)| c == e.detail && sym == XK_ESCAPE) => {
                tray.close_menu();
            }
            _ => {}
        }
        tray.x.conn.flush()?;
    }
}
