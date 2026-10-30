use crate::geometry::{Point, Rect, Size};

// A display (monitor), as the platform sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    // Identifies the physical display, and stays the same when it's
    // unplugged and plugged back in, or after a restart.
    pub id: String,
    pub name: String,
    pub frame: Rect,
    // The frame minus panels and docks.
    pub work_area: Rect,
    // The physical size, or zero if unknown.
    pub size_mm: Size,
    // A laptop's own screen.
    pub builtin: bool,
}

// The display under `point`, or the first one (the primary display).
pub fn display_containing(displays: &[Display], point: Point) -> Option<usize> {
    displays
        .iter()
        .position(|d| d.frame.contains(point))
        .or(if displays.is_empty() { None } else { Some(0) })
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    // What the user sees as the window: decorations included, client-side
    // shadows excluded.
    pub frame: Rect,
    pub can_move: bool,
    pub can_resize: bool,
    pub maximized: bool,
    // Identifies the window's app, to remember where its windows were on
    // each display.
    pub app: Option<String>,
    pub min_size: Size,
    pub max_size: Option<Size>,
}

// What the window mover needs from a windowing system. Window frames are in
// global screen coordinates.
pub trait Platform {
    type Window: Clone + PartialEq + std::fmt::Debug;

    fn pointer(&mut self) -> Point;
    fn warp_pointer(&mut self, point: Point);
    // Whether the main button is held down.
    fn button_held(&mut self) -> bool;

    // The topmost window under `point`.
    fn window_at(&mut self, point: Point) -> Option<Self::Window>;
    fn window_info(&mut self, window: &Self::Window) -> Option<WindowInfo>;
    // The frames of the other visible windows, to snap to.
    fn other_windows(&mut self, window: &Self::Window) -> Vec<Rect>;
    // Primary display first.
    fn displays(&mut self) -> Vec<Display>;

    // Return false if the window is gone.
    fn set_frame(&mut self, window: &Self::Window, frame: Rect) -> bool;
    fn set_origin(&mut self, window: &Self::Window, origin: Point) -> bool;
    fn unmaximize(&mut self, window: &Self::Window);
    fn raise(&mut self, window: &Self::Window);

    // Shows the grid lines over `area` (a display's work area).
    fn show_grid(&mut self, area: Rect, columns: usize, rows: usize);
    fn hide_grid(&mut self);
    // Darkens every display, right below the given window so it stays bright.
    fn show_dimmer(&mut self, below: &Self::Window);
    fn hide_dimmer(&mut self);
}
