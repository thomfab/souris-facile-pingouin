use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    AtomEnum, ClipOrdering, ColormapAlloc, ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, StackMode,
    VisualClass, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_FROM_PARENT;

use super::Res;
use crate::geometry::{divide, Rect};
use crate::platform::Display;

const GRID_LINE_WIDTH: f64 = 2.0;
// Premultiplied ARGB: white at 35%, black at 50%.
const GRID_LINE_COLOR: u32 = 0x5959_5959;
const DIM_COLOR: u32 = 0x8000_0000;
// The grid lines without a compositor, opaque.
const GRID_LINE_OPAQUE: u32 = 0x00A0_A0A0;

// Borderless, click-through windows the window manager leaves alone.
pub struct Overlays {
    screen: usize,
    // A 32-bit visual and its colormap, for translucent windows.
    argb: Option<(Visualid, u32)>,
    grid: Vec<Window>,
    grid_shown: Option<(Rect, usize, usize)>,
    dimmers: Vec<Window>,
}

impl Overlays {
    pub fn new(conn: &RustConnection, screen: usize) -> Res<Overlays> {
        let s = &conn.setup().roots[screen];
        let visual = s
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| &d.visuals)
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id);
        let argb = match visual {
            Some(visual) => {
                let colormap = conn.generate_id()?;
                conn.create_colormap(ColormapAlloc::NONE, colormap, s.root, visual)?;
                Some((visual, colormap))
            }
            None => None,
        };
        Ok(Overlays { screen, argb, grid: Vec::new(), grid_shown: None, dimmers: Vec::new() })
    }

    // Translucent windows need both a 32-bit visual and a compositor.
    fn composited(&self, conn: &RustConnection) -> bool {
        let Some(_) = self.argb else { return false };
        let name = format!("_NET_WM_CM_S{}", self.screen);
        let owner = conn
            .intern_atom(true, name.as_bytes())
            .ok()
            .and_then(|c| c.reply().ok())
            .filter(|r| r.atom != 0)
            .and_then(|r| conn.get_selection_owner(r.atom).ok()?.reply().ok());
        owner.is_some_and(|o| o.owner != 0)
    }

    fn create(&self, conn: &RustConnection, root: Window, frame: Rect, color: u32, translucent: bool) -> Option<Window> {
        let window = conn.generate_id().ok()?;
        let aux = CreateWindowAux::new().override_redirect(1).border_pixel(0).background_pixel(color);
        let (depth, visual, aux) = match self.argb.filter(|_| translucent) {
            Some((visual, colormap)) => (32, visual, aux.colormap(colormap)),
            None => (COPY_FROM_PARENT as u8, COPY_FROM_PARENT, aux),
        };
        let (w, h) = (frame.width.max(1.0) as u16, frame.height.max(1.0) as u16);
        conn.create_window(depth, window, root, frame.x as i16, frame.y as i16, w, h, 0, WindowClass::INPUT_OUTPUT,
                           visual, &aux)
            .ok()?;
        // Named, for compositor rules (no shadows, no fading).
        let class = b"souris-facile-pingouin\0souris-facile-pingouin\0";
        let _ = conn.change_property8(x11rb::protocol::xproto::PropMode::REPLACE, window, AtomEnum::WM_CLASS,
                                      AtomEnum::STRING, class);
        // Clicks go through to whatever is below.
        let _ = conn.shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, window, 0, 0, &[]);
        Some(window)
    }

    // Adds the grid lines over `area` (a display's work area) on top of
    // everything, inner lines only.
    pub fn add_grid(&mut self, conn: &RustConnection, root: Window, area: Rect, columns: usize, rows: usize) {
        let translucent = self.composited(conn);
        let color = if translucent { GRID_LINE_COLOR } else { GRID_LINE_OPAQUE };
        let half = GRID_LINE_WIDTH / 2.0;
        let xs = divide(area.min_x(), area.width, columns);
        let ys = divide(area.min_y(), area.height, rows);
        let lines = xs[1..xs.len() - 1]
            .iter()
            .map(|&x| Rect::new(x - half, area.min_y(), GRID_LINE_WIDTH, area.height))
            .chain(ys[1..ys.len() - 1].iter().map(|&y| Rect::new(area.min_x(), y - half, area.width, GRID_LINE_WIDTH)));
        for line in lines {
            if let Some(window) = self.create(conn, root, line, color, translucent) {
                let _ = conn.map_window(window);
                let _ = conn.configure_window(window, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
                self.grid.push(window);
            }
        }
    }

    pub fn show_grid(&mut self, conn: &RustConnection, root: Window, area: Rect, columns: usize, rows: usize) {
        if self.grid_shown == Some((area, columns, rows)) {
            return;
        }
        self.hide_grid(conn);
        self.add_grid(conn, root, area, columns, rows);
        self.grid_shown = Some((area, columns, rows));
        let _ = conn.flush();
    }

    pub fn hide_grid(&mut self, conn: &RustConnection) {
        if self.grid.is_empty() {
            return;
        }
        for window in self.grid.drain(..) {
            let _ = conn.destroy_window(window);
        }
        self.grid_shown = None;
        let _ = conn.flush();
    }

    // Darkens every display, right below the given window so it stays
    // bright. Without a compositor, it would black them out: nothing then.
    pub fn show_dimmer(&mut self, conn: &RustConnection, root: Window, below: Window, displays: &[Display]) {
        self.hide_dimmer(conn);
        if !self.composited(conn) {
            return;
        }
        let Some(frame) = top_level(conn, root, below) else { return };
        for display in displays {
            if let Some(window) = self.create(conn, root, display.frame, DIM_COLOR, true) {
                let aux = ConfigureWindowAux::new().sibling(frame).stack_mode(StackMode::BELOW);
                let _ = conn.configure_window(window, &aux);
                let _ = conn.map_window(window);
                self.dimmers.push(window);
            }
        }
        let _ = conn.flush();
    }

    pub fn hide_dimmer(&mut self, conn: &RustConnection) {
        if self.dimmers.is_empty() {
            return;
        }
        for window in self.dimmers.drain(..) {
            let _ = conn.destroy_window(window);
        }
        let _ = conn.flush();
    }
}

// The child of the root holding `window`: the frame the window manager
// put it in, if any.
fn top_level(conn: &RustConnection, root: Window, mut window: Window) -> Option<Window> {
    loop {
        let parent = conn.query_tree(window).ok()?.reply().ok()?.parent;
        if parent == root || parent == 0 {
            return Some(window);
        }
        window = parent;
    }
}
