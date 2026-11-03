// The settings window: a sidebar of pages, like the macOS version's, that
// edits the settings file. The running app reads it again on its own.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use x11rb::connection::Connection;
use x11rb::properties::{WmHints, WmSizeHints};
use x11rb::protocol::randr::{ConnectionExt as _, NotifyMask};
use x11rb::protocol::xproto::{
    ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, Keycode, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_FROM_PARENT, CURRENT_TIME};

use super::{surface, Res, X11};
use crate::geometry::{Point, Rect};
use crate::memory::{MemoryStore, WindowMemory};
use crate::platform::Display;
use crate::settings::{
    self, sensible_grid, DisplayGrid, Modifier, Mods, Settings, SettingsFile, GRID_PRESETS, GRID_RANGE,
};
use crate::ui::{Canvas, Fonts, Rgba, Weight};

const WIDTH: f64 = 760.0;
const HEIGHT: f64 = 560.0;
const SIDEBAR: f64 = 220.0;
const PADDING: f64 = 28.0;
const CARD_PADDING: f64 = 16.0;
const SIZE: f32 = 14.0;
const SMALL: f32 = 12.5;
const PREVIEW_TIME: Duration = Duration::from_secs(2);

const XK_ESCAPE: u32 = 0xFF1B;
const XK_W: u32 = 0x77;
const XK_Q: u32 = 0x71;

struct Theme {
    sidebar: Rgba,
    background: Rgba,
    card: Rgba,
    border: Rgba,
    separator: Rgba,
    text: Rgba,
    secondary: Rgba,
    accent: Rgba,
    selected: Rgba,
    hover: Rgba,
    warning: Rgba,
    off: Rgba,
    button: Rgba,
    button_hover: Rgba,
    popup: Rgba,
}

const BLACK: Rgba = Rgba::rgb(0x000000);
const WHITE: Rgba = Rgba::rgb(0xFFFFFF);

impl Theme {
    fn new(dark: bool) -> Theme {
        if dark {
            Theme {
                sidebar: Rgba::rgb(0x2A2A2E),
                background: Rgba::rgb(0x1F1F22),
                card: Rgba::rgb(0x2E2E33),
                border: WHITE.alpha(28),
                separator: WHITE.alpha(22),
                text: Rgba::rgb(0xF2F2F2),
                secondary: Rgba::rgb(0xA0A0A6),
                accent: Rgba::rgb(0x3584E4),
                selected: WHITE.alpha(30),
                hover: WHITE.alpha(14),
                warning: Rgba::rgb(0xF5A524),
                off: WHITE.alpha(50),
                button: WHITE.alpha(24),
                button_hover: WHITE.alpha(40),
                popup: Rgba::rgb(0x38383D),
            }
        } else {
            Theme {
                sidebar: Rgba::rgb(0xEBEBED),
                background: Rgba::rgb(0xFAFAFA),
                card: WHITE,
                border: BLACK.alpha(22),
                separator: BLACK.alpha(16),
                text: Rgba::rgb(0x2E2E30),
                secondary: Rgba::rgb(0x6E6E73),
                accent: Rgba::rgb(0x3584E4),
                selected: BLACK.alpha(26),
                hover: BLACK.alpha(12),
                warning: Rgba::rgb(0xB85C00),
                off: BLACK.alpha(40),
                button: BLACK.alpha(16),
                button_hover: BLACK.alpha(28),
                popup: WHITE,
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Page {
    General,
    Move,
    Resize,
    Snapping,
    Send,
    Display(String),
    Unplugged,
}

#[derive(Clone, Copy, PartialEq)]
enum Icon {
    General,
    Move,
    Resize,
    Snapping,
    Send,
    Monitor,
    Laptop,
    Unplugged,
}

#[derive(Clone, Debug, PartialEq)]
enum Action {
    Page(Page),
    Toggle(&'static str),
    Key(&'static str, Modifier),
    Snap(i64),
    Picker(String),
    Preset(String, usize),
    Grid(String, i64, i64),
    Preview(String),
    ForgetFrames(String),
    ForgetDisplay(String),
    ForgetAll,
    LaunchAtLogin,
    OpenFile,
    Dismiss,
}

// The settings, by their name in the file.
fn flag(s: &Settings, key: &str) -> bool {
    match key {
        "enabled" => s.enabled,
        "raise-window" => s.raise_window,
        "resize-nearest-corner" => s.resize_nearest_corner,
        "snap-to-windows" => s.snap_to_windows,
        "show-grid" => s.show_grid,
        "dim-windows" => s.dim_windows,
        "send-with-scroll" => s.send_with_scroll,
        "send-with-arrows" => s.send_with_arrows,
        "remember-window-frames" => s.remember_window_frames,
        _ => false,
    }
}

enum Control {
    None,
    Switch(bool, Action),
    // A value, then the actions to lower and raise it, if possible.
    Stepper(String, Option<Action>, Option<Action>),
    Button(&'static str, Option<Action>),
    Keys(&'static str, Mods),
    Picker(String, Action),
}

struct Row {
    title: String,
    subtitle: Option<String>,
    control: Control,
    enabled: bool,
    warning: bool,
}

impl Row {
    fn new(title: impl Into<String>, control: Control) -> Row {
        Row { title: title.into(), subtitle: None, control, enabled: true, warning: false }
    }

    fn subtitle(mut self, text: impl Into<String>) -> Row {
        self.subtitle = Some(text.into());
        self
    }

    fn enabled(mut self, enabled: bool) -> Row {
        self.enabled = enabled;
        self
    }

    fn warning(text: impl Into<String>) -> Row {
        Row { warning: true, ..Row::new(text, Control::None) }
    }
}

const SWITCH: (f64, f64) = (40.0, 22.0);
const CONTROL_HEIGHT: f64 = 28.0;

// Lays out and draws a page, recording where its controls are.
struct Form<'a> {
    c: &'a mut Canvas,
    t: &'a Theme,
    // In the form's coordinates.
    mouse: Option<Point>,
    hits: &'a mut Vec<(Rect, Action)>,
    // From the form's coordinates to the window's, and the part shown.
    offset: Point,
    view: Rect,
    y: f64,
    left: f64,
    width: f64,
    // Where the open preset list goes, in the window.
    picker: Option<Rect>,
}

impl Form<'_> {
    // Records a control, and tells whether the pointer is over it.
    fn hit(&mut self, r: Rect, action: Option<Action>) -> bool {
        let Some(action) = action else { return false };
        let window = Rect::new(r.x + self.offset.x, r.y + self.offset.y, r.width, r.height);
        if let Some(shown) = window.intersection(&self.view) {
            self.hits.push((shown, action));
        }
        self.mouse.is_some_and(|m| r.contains(m))
    }

    fn text(&mut self, x: f64, y: f64, text: &str, size: f32, weight: Weight, color: Rgba) -> f64 {
        self.c.text(x, y, text, size, weight, color)
    }

    // Wrapped text, giving its height.
    fn paragraph(&mut self, x: f64, y: f64, width: f64, text: &str, size: f32, color: Rgba) -> f64 {
        let line = size as f64 * 1.4;
        let lines = self.c.wrap(text, size, Weight::Regular, width);
        for (i, l) in lines.iter().enumerate() {
            self.c.text(x, y + i as f64 * line, l, size, Weight::Regular, color);
        }
        lines.len() as f64 * line
    }

    fn title(&mut self, title: &str, subtitle: Option<&str>) {
        self.y += 24.0;
        self.text(self.left, self.y, title, 21.0, Weight::Bold, self.t.text);
        self.y += 32.0;
        if let Some(subtitle) = subtitle {
            let (left, width, color) = (self.left, self.width, self.t.secondary);
            self.y += self.paragraph(left, self.y, width, subtitle, SMALL, color);
        }
        self.y += 10.0;
    }

    fn control_width(&self, control: &Control) -> f64 {
        let c = &self.c;
        match control {
            Control::None => 0.0,
            Control::Switch(..) => SWITCH.0,
            Control::Stepper(value, ..) => c.text_width(value, SIZE, Weight::Regular) + 12.0 + 72.0,
            Control::Button(label, _) => c.text_width(label, SIZE, Weight::Regular) + 28.0,
            Control::Keys(..) => {
                Modifier::ALL.iter().map(|m| c.text_width(m.label(), SMALL, Weight::Bold) + 22.0).sum::<f64>()
                    + 6.0 * 3.0
            }
            Control::Picker(label, _) => c.text_width(label, SIZE, Weight::Regular) + 40.0,
        }
    }

    fn text_width(&self, row: &Row) -> f64 {
        let control = self.control_width(&row.control);
        let icon = if row.warning { 26.0 } else { 0.0 };
        self.width - 2.0 * CARD_PADDING - icon - if control > 0.0 { control + 16.0 } else { 0.0 }
    }

    fn row_height(&self, row: &Row) -> f64 {
        let width = self.text_width(row);
        let lines = |text: &str, size| self.c.wrap(text, size, Weight::Regular, width).len() as f64 * size as f64 * 1.4;
        let title = if row.warning { lines(&row.title, SIZE) } else { 20.0 };
        let subtitle = row.subtitle.as_deref().map_or(0.0, |s| 2.0 + lines(s, SMALL));
        (title + subtitle + 2.0 * 13.0).max(50.0)
    }

    fn section(&mut self, header: Option<&str>, rows: Vec<Row>, footer: Option<&str>) {
        if let Some(header) = header {
            self.text(self.left + 2.0, self.y, header, SIZE, Weight::Bold, self.t.text);
            self.y += 26.0;
        }
        let heights: Vec<f64> = rows.iter().map(|r| self.row_height(r)).collect();
        let card = Rect::new(self.left, self.y, self.width, heights.iter().sum());
        self.c.fill_rect(card, 10.0, self.t.card);
        self.c.stroke_rect(card, 10.0, 1.0, self.t.border);
        for (i, (row, height)) in rows.into_iter().zip(heights).enumerate() {
            let r = Rect::new(self.left, self.y, self.width, height);
            if i > 0 {
                let line = Rect::new(r.x + CARD_PADDING, r.y, r.width - 2.0 * CARD_PADDING, 1.0);
                self.c.fill_rect(line, 0.0, self.t.separator);
            }
            self.row(r, row);
            self.y += height;
        }
        self.y += 8.0;
        if let Some(footer) = footer {
            let (left, width, color) = (self.left + 2.0, self.width - 4.0, self.t.secondary);
            self.y += self.paragraph(left, self.y, width, footer, SMALL, color);
        }
        self.y += 22.0;
    }

    fn row(&mut self, r: Rect, row: Row) {
        let t = self.t;
        let dim = |c: Rgba| if row.enabled { c } else { c.alpha(c.3 / 5 * 2) };
        let mut x = r.x + CARD_PADDING;
        let width = self.text_width(&row);
        if row.warning {
            warning_icon(self.c, x, r.y + 15.0, t.warning);
            x += 26.0;
            self.paragraph(x, r.y + 14.0, width, &row.title, SIZE, t.warning);
            return;
        }
        let text_height = 20.0 + row.subtitle.as_deref().map_or(0.0, |s| {
            2.0 + self.c.wrap(s, SMALL, Weight::Regular, width).len() as f64 * SMALL as f64 * 1.4
        });
        let top = r.y + (r.height - text_height) / 2.0;
        self.text(x, top + 1.0, &row.title, SIZE, Weight::Regular, dim(t.text));
        if let Some(subtitle) = &row.subtitle {
            self.paragraph(x, top + 22.0, width, subtitle, SMALL, dim(t.secondary));
        }

        let cw = self.control_width(&row.control);
        let right = r.max_x() - CARD_PADDING;
        let mid = r.mid_y();
        let enabled = row.enabled;
        let action = |a: Action| enabled.then_some(a);
        match row.control {
            Control::None => {}
            Control::Switch(on, a) => {
                let s = Rect::new(right - SWITCH.0, mid - SWITCH.1 / 2.0, SWITCH.0, SWITCH.1);
                let hovered = self.hit(Rect::new(r.x, r.y, r.width, r.height), action(a));
                let track = if on { t.accent } else if hovered { t.off.alpha(t.off.3 + 20) } else { t.off };
                self.c.fill_rect(s, 11.0, dim(track));
                let knob = if on { s.max_x() - 11.0 } else { s.x + 11.0 };
                self.c.fill_circle(knob, mid, 9.0, dim(WHITE));
            }
            Control::Stepper(value, minus, plus) => {
                let w = self.c.text_width(&value, SIZE, Weight::Regular);
                self.text(right - 72.0 - 12.0 - w, mid - 10.0, &value, SIZE, Weight::Regular, dim(t.text));
                let box_ = Rect::new(right - 72.0, mid - CONTROL_HEIGHT / 2.0, 72.0, CONTROL_HEIGHT);
                self.c.fill_rect(box_, 7.0, dim(t.button));
                for (i, a) in [minus, plus].into_iter().enumerate() {
                    let half = Rect::new(box_.x + 36.0 * i as f64, box_.y, 36.0, box_.height);
                    let active = a.is_some() && enabled;
                    if self.hit(half, a.filter(|_| enabled)) {
                        self.c.fill_rect(half, 7.0, t.button_hover);
                    }
                    let color = if active { t.text } else { t.text.alpha(t.text.3 / 3) };
                    let (cx, cy) = (half.mid_x(), half.mid_y());
                    self.c.stroke(cx, cy, 1.6, color, |pb| {
                        pb.move_to(-5.0, 0.0);
                        pb.line_to(5.0, 0.0);
                        if i == 1 {
                            pb.move_to(0.0, -5.0);
                            pb.line_to(0.0, 5.0);
                        }
                    });
                }
                self.c.fill_rect(Rect::new(box_.mid_x() - 0.5, box_.y + 6.0, 1.0, box_.height - 12.0), 0.0,
                                 t.separator);
            }
            Control::Button(label, a) => {
                let b = Rect::new(right - cw, mid - CONTROL_HEIGHT / 2.0, cw, CONTROL_HEIGHT);
                let active = a.is_some() && enabled;
                let hovered = self.hit(b, a.filter(|_| enabled));
                self.c.fill_rect(b, 7.0, if hovered { t.button_hover } else { t.button });
                let color = if active { t.text } else { t.text.alpha(t.text.3 / 3) };
                self.text(b.x + 14.0, mid - 10.0, label, SIZE, Weight::Regular, color);
            }
            Control::Keys(key, mods) => {
                let mut x = right - cw;
                for m in Modifier::ALL {
                    let w = self.c.text_width(m.label(), SMALL, Weight::Bold) + 22.0;
                    let chip = Rect::new(x, mid - CONTROL_HEIGHT / 2.0, w, CONTROL_HEIGHT);
                    let on = mods.contains(m);
                    let hovered = self.hit(chip, action(Action::Key(key, m)));
                    let (fill, color) = match (on, hovered) {
                        (true, _) => (t.accent, WHITE),
                        (false, true) => (t.button_hover, t.text),
                        (false, false) => (t.button, t.text),
                    };
                    self.c.fill_rect(chip, 7.0, dim(fill));
                    self.text(chip.x + 11.0, mid - 9.0, m.label(), SMALL, Weight::Bold, dim(color));
                    x += w + 6.0;
                }
            }
            Control::Picker(label, a) => {
                let b = Rect::new(right - cw, mid - CONTROL_HEIGHT / 2.0, cw, CONTROL_HEIGHT);
                let hovered = self.hit(b, action(a));
                self.c.fill_rect(b, 7.0, if hovered { t.button_hover } else { t.button });
                self.text(b.x + 12.0, mid - 10.0, &label, SIZE, Weight::Regular, dim(t.text));
                let (cx, cy) = (b.max_x() - 15.0, mid);
                self.c.stroke(cx, cy, 1.6, dim(t.text), |pb| {
                    pb.move_to(-4.0, -2.0);
                    pb.line_to(0.0, 2.0);
                    pb.line_to(4.0, -2.0);
                });
                self.picker = Some(Rect::new(b.x + self.offset.x, b.y + self.offset.y, b.width, b.height));
            }
        }
    }

    // A small drawing of the display and its grid.
    fn display_preview(&mut self, display: &Display, columns: usize, rows: usize) {
        let t = self.t;
        let (max_w, max_h) = (240.0, 120.0);
        let aspect = (display.frame.width / display.frame.height.max(1.0)).max(0.1);
        let (w, h) = if aspect > max_w / max_h { (max_w, max_w / aspect) } else { (max_h * aspect, max_h) };
        let x = self.left + (self.width - w) / 2.0;
        let y = self.y + 6.0;
        let bezel = Rect::new(x - 6.0, y - 6.0, w + 12.0, h + 12.0);
        self.c.fill_rect(bezel, 9.0, t.card);
        self.c.stroke_rect(bezel, 9.0, 1.5, t.border);
        if display.builtin {
            let base = Rect::new(x - 26.0, bezel.max_y(), w + 52.0, 8.0);
            self.c.fill_rect(base, 4.0, t.off);
        } else {
            self.c.fill_rect(Rect::new(x + w / 2.0 - 14.0, bezel.max_y(), 28.0, 12.0), 0.0, t.off);
            self.c.fill_rect(Rect::new(x + w / 2.0 - 34.0, bezel.max_y() + 12.0, 68.0, 4.0), 2.0, t.off);
        }
        let screen = Rect::new(x, y, w, h);
        self.c.fill_rect(screen, 4.0, t.accent.alpha(40));
        for i in 1..columns {
            let lx = x + w * i as f64 / columns as f64;
            self.c.fill_rect(Rect::new(lx - 0.5, y, 1.0, h), 0.0, t.accent.alpha(170));
        }
        for i in 1..rows {
            let ly = y + h * i as f64 / rows as f64;
            self.c.fill_rect(Rect::new(x, ly - 0.5, w, 1.0), 0.0, t.accent.alpha(170));
        }
        self.y += h + 44.0;
    }
}

fn warning_icon(c: &mut Canvas, x: f64, y: f64, color: Rgba) {
    c.fill(x, y, color, |pb| {
        pb.move_to(9.0, 0.5);
        pb.line_to(17.5, 16.0);
        pb.line_to(0.5, 16.0);
        pb.close();
    });
    c.stroke(x, y, 1.8, WHITE, |pb| {
        pb.move_to(9.0, 6.0);
        pb.line_to(9.0, 10.0);
        pb.move_to(9.0, 13.2);
        pb.line_to(9.0, 13.3);
    });
}

// The sidebar's icons, in a 16 × 16 box.
fn sidebar_icon(c: &mut Canvas, icon: Icon, x: f64, y: f64, color: Rgba) {
    let w = 1.5;
    match icon {
        Icon::General => {
            c.stroke(x, y, w, color, |pb| {
                pb.push_circle(8.0, 8.0, 2.6);
                for i in 0..8 {
                    let a = i as f32 * std::f32::consts::FRAC_PI_4;
                    pb.move_to(8.0 + 4.6 * a.cos(), 8.0 + 4.6 * a.sin());
                    pb.line_to(8.0 + 7.2 * a.cos(), 8.0 + 7.2 * a.sin());
                }
                pb.push_circle(8.0, 8.0, 5.0);
            });
        }
        Icon::Move => c.stroke(x, y, w, color, |pb| {
            pb.move_to(8.0, 1.0);
            pb.line_to(8.0, 15.0);
            pb.move_to(1.0, 8.0);
            pb.line_to(15.0, 8.0);
            for (dx, dy) in [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0f32)] {
                let (tx, ty) = (8.0 + 7.0 * dx, 8.0 + 7.0 * dy);
                pb.move_to(tx - 2.5 * dx - 2.5 * dy.abs(), ty - 2.5 * dy - 2.5 * dx.abs());
                pb.line_to(tx, ty);
                pb.line_to(tx - 2.5 * dx + 2.5 * dy.abs(), ty - 2.5 * dy + 2.5 * dx.abs());
            }
        }),
        Icon::Resize => c.stroke(x, y, w, color, |pb| {
            pb.move_to(2.0, 14.0);
            pb.line_to(14.0, 2.0);
            pb.move_to(8.0, 2.0);
            pb.line_to(14.0, 2.0);
            pb.line_to(14.0, 8.0);
            pb.move_to(2.0, 8.0);
            pb.line_to(2.0, 14.0);
            pb.line_to(8.0, 14.0);
        }),
        Icon::Snapping => c.stroke(x, y, w, color, |pb| {
            pb.push_rect(tiny_skia::Rect::from_xywh(1.0, 1.0, 14.0, 14.0).unwrap());
            for v in [5.67, 10.33] {
                pb.move_to(v, 1.0);
                pb.line_to(v, 15.0);
                pb.move_to(1.0, v);
                pb.line_to(15.0, v);
            }
        }),
        Icon::Send => c.stroke(x, y, w, color, |pb| {
            pb.push_rect(tiny_skia::Rect::from_xywh(1.0, 4.0, 10.0, 8.0).unwrap());
            pb.move_to(5.0, 1.0);
            pb.line_to(15.0, 1.0);
            pb.line_to(15.0, 9.0);
            pb.line_to(13.0, 9.0);
        }),
        Icon::Monitor => c.stroke(x, y, w, color, |pb| {
            pb.push_rect(tiny_skia::Rect::from_xywh(1.0, 2.0, 14.0, 9.5).unwrap());
            pb.move_to(8.0, 11.5);
            pb.line_to(8.0, 14.5);
            pb.move_to(4.5, 14.5);
            pb.line_to(11.5, 14.5);
        }),
        Icon::Laptop => c.stroke(x, y, w, color, |pb| {
            pb.push_rect(tiny_skia::Rect::from_xywh(2.5, 3.0, 11.0, 8.0).unwrap());
            pb.move_to(0.5, 13.5);
            pb.line_to(15.5, 13.5);
        }),
        Icon::Unplugged => c.stroke(x, y, w, color, |pb| {
            pb.move_to(5.5, 1.0);
            pb.line_to(5.5, 4.5);
            pb.move_to(10.5, 1.0);
            pb.line_to(10.5, 4.5);
            pb.move_to(3.0, 4.5);
            pb.line_to(13.0, 4.5);
            pb.line_to(13.0, 7.5);
            pb.cubic_to(13.0, 9.5, 11.0, 11.0, 8.0, 11.0);
            pb.cubic_to(5.0, 11.0, 3.0, 9.5, 3.0, 7.5);
            pb.close();
            pb.move_to(8.0, 11.0);
            pb.line_to(8.0, 15.0);
        }),
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

struct App {
    x: X11,
    window: u32,
    gc: u32,
    depth: u8,
    scale: f32,
    fonts: Rc<Fonts>,
    theme: Theme,
    keys: Vec<(Keycode, u32)>,
    settings_path: PathBuf,
    frames: MemoryStore,
    settings: Settings,
    error: Option<String>,
    settings_time: Option<SystemTime>,
    displays: Vec<Display>,
    launch_at_login: bool,
    page: Page,
    scroll: f64,
    content_height: f64,
    mouse: Option<Point>,
    pressed: Option<Action>,
    hits: Vec<(Rect, Action)>,
    picker: Option<String>,
    preview_until: Option<Instant>,
    canvas: Canvas,
}

impl App {
    fn reload(&mut self) {
        self.settings_time = modified(&self.settings_path);
        match std::fs::read_to_string(&self.settings_path) {
            Ok(text) => match Settings::parse(&text) {
                Ok(settings) => {
                    self.settings = settings;
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            },
            Err(_) => {
                self.settings = Settings::default();
                self.error = None;
            }
        }
        self.launch_at_login = settings::autostart_path().exists();
    }

    fn edit(&mut self, change: impl FnOnce(&mut SettingsFile)) {
        let result = SettingsFile::open(&self.settings_path).and_then(|mut file| {
            change(&mut file);
            file.settings()?;
            file.save()
        });
        if let Err(e) = result {
            self.error = Some(e);
            return;
        }
        self.reload();
    }

    fn memory(&self) -> WindowMemory {
        self.frames.load()
    }

    fn grid(&self, id: &str) -> Option<(&Display, (usize, usize))> {
        let display = self.displays.iter().find(|d| d.id == id)?;
        Some((display, self.settings.grid(display)))
    }

    fn set_grid(&mut self, id: &str, columns: usize, rows: usize) {
        let Some((display, _)) = self.grid(id) else { return };
        let grid = DisplayGrid { name: display.name.clone(), columns, rows };
        let id = id.to_string();
        self.edit(|f| f.set_grid(&id, &grid));
        self.preview(&id);
    }

    fn preview(&mut self, id: &str) {
        let Some((display, (columns, rows))) = self.grid(id) else { return };
        let area = display.work_area;
        self.x.overlays.show_grid(&self.x.conn, self.x.root, area, columns, rows);
        self.preview_until = Some(Instant::now() + PREVIEW_TIME);
    }

    // Unplugged displays with settings or remembered window sizes.
    fn unplugged(&self) -> Vec<(String, String, String)> {
        let memory = self.memory();
        let mut ids: Vec<&String> = self.settings.displays.keys().collect();
        ids.extend(memory.displays());
        ids.sort();
        ids.dedup();
        ids.into_iter()
            .filter(|id| !self.displays.iter().any(|d| &d.id == *id))
            .map(|id| {
                let grid = self.settings.displays.get(id);
                let name = grid.map(|g| g.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
                let mut summary = Vec::new();
                if let Some(g) = grid {
                    summary.push(format!("Grid {} × {}", g.columns, g.rows));
                }
                match memory.count(id) {
                    0 => {}
                    1 => summary.push("1 app's window size".into()),
                    n => summary.push(format!("{n} apps' window sizes")),
                }
                (id.clone(), name, summary.join(", "))
            })
            .collect()
    }

    fn act(&mut self, action: Action) {
        match action {
            Action::Page(page) => {
                self.page = page;
                self.scroll = 0.0;
                self.picker = None;
            }
            Action::Toggle(key) => {
                let on = flag(&self.settings, key);
                self.edit(|f| f.set_bool(key, !on));
            }
            Action::Key(key, modifier) => {
                let mods = if key == "move-modifiers" { self.settings.move_mods() } else { self.settings.resize_mods() };
                self.edit(|f| f.set_mods(key, mods.toggled(modifier)));
            }
            Action::Snap(delta) => {
                let value = (self.settings.snap_threshold.round() as i64 + delta).clamp(0, 200);
                self.edit(|f| f.set_int("snap-threshold", value));
            }
            Action::Picker(id) => self.picker = Some(id),
            Action::Dismiss => self.picker = None,
            Action::Preset(id, i) => {
                self.picker = None;
                self.set_grid(&id, GRID_PRESETS[i].columns, GRID_PRESETS[i].rows);
            }
            Action::Grid(id, dc, dr) => {
                if let Some((_, (columns, rows))) = self.grid(&id) {
                    let clamp = |v: usize, d: i64| {
                        (v as i64 + d).clamp(*GRID_RANGE.start() as i64, *GRID_RANGE.end() as i64) as usize
                    };
                    self.set_grid(&id, clamp(columns, dc), clamp(rows, dr));
                }
            }
            Action::Preview(id) => self.preview(&id),
            Action::ForgetFrames(id) => {
                let mut memory = self.memory();
                memory.forget(&id);
                self.frames.save(memory);
            }
            Action::ForgetDisplay(id) => {
                let mut memory = self.memory();
                memory.forget(&id);
                self.frames.save(memory);
                self.edit(|f| f.remove_grid(&id));
            }
            Action::ForgetAll => self.frames.save(WindowMemory::default()),
            Action::LaunchAtLogin => {
                let path = settings::autostart_path();
                let result = if self.launch_at_login {
                    std::fs::remove_file(&path)
                } else {
                    let exe = std::env::current_exe().unwrap_or_else(|_| "souris-facile-pingouin".into());
                    path.parent().map_or(Ok(()), std::fs::create_dir_all)
                        .and_then(|_| std::fs::write(&path, settings::desktop_entry(&exe)))
                };
                if let Err(e) = result {
                    self.error = Some(format!("{}: {e}", path.display()));
                }
                self.launch_at_login = path.exists();
            }
            Action::OpenFile => {
                // Written with its comments if there's none yet.
                let _ = Settings::load(&self.settings_path);
                let _ = std::process::Command::new("xdg-open").arg(&self.settings_path).spawn();
            }
        }
    }

    fn pages(&self) -> Vec<(Page, Icon, String)> {
        let mut pages = vec![
            (Page::General, Icon::General, "General".to_string()),
            (Page::Move, Icon::Move, "Move".into()),
            (Page::Resize, Icon::Resize, "Resize".into()),
            (Page::Snapping, Icon::Snapping, "Snapping".into()),
            (Page::Send, Icon::Send, "Send to Display".into()),
        ];
        for d in &self.displays {
            let icon = if d.builtin { Icon::Laptop } else { Icon::Monitor };
            pages.push((Page::Display(d.id.clone()), icon, d.name.clone()));
        }
        if !self.unplugged().is_empty() {
            pages.push((Page::Unplugged, Icon::Unplugged, "Unplugged Displays".into()));
        }
        pages
    }

    fn draw(&mut self) {
        let t = &self.theme;
        let mut hits = Vec::new();
        // Under the open preset list, nothing reacts to the pointer.
        let mouse = if self.picker.is_some() { None } else { self.mouse };

        // The page, scrolled.
        let view = Rect::new(SIDEBAR, 0.0, WIDTH - SIDEBAR, HEIGHT);
        let max_scroll = (self.content_height - HEIGHT).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
        let mut detail = Canvas::new(view.width, view.height, self.scale, self.fonts.clone());
        detail.clear(t.background);
        detail.set_origin(0.0, -self.scroll);
        let offset = Point::new(SIDEBAR, -self.scroll);
        let mut form = Form {
            c: &mut detail,
            t,
            mouse: mouse.map(|m| Point::new(m.x - offset.x, m.y - offset.y)),
            hits: &mut hits,
            offset,
            view,
            y: 0.0,
            left: PADDING,
            width: view.width - 2.0 * PADDING,
            picker: None,
        };
        self.page_content(&mut form);
        let picker = form.picker;
        self.content_height = form.y + 8.0;

        let pages = self.pages();
        let current = self.picker.as_ref().and_then(|id| self.grid(id)).map(|g| g.1);
        let c = &mut self.canvas;
        c.clear(t.sidebar);
        c.draw_pixmap(SIDEBAR, 0.0, &detail.pixmap);
        if self.content_height > HEIGHT {
            let h = HEIGHT * HEIGHT / self.content_height;
            let y = (HEIGHT - h) * self.scroll / (self.content_height - HEIGHT);
            c.fill_rect(Rect::new(WIDTH - 7.0, y + 3.0, 4.0, h - 6.0), 2.0, t.off);
        }
        c.fill_rect(Rect::new(SIDEBAR - 1.0, 0.0, 1.0, HEIGHT), 0.0, t.separator);

        // The sidebar: the app, then the pages.
        let icon = crate::icon::pixmap((40.0 * self.scale).round() as u32);
        c.draw_pixmap(18.0, 20.0, &icon);
        let name = c.wrap("Souris facile pingouin", SIZE, Weight::Bold, SIDEBAR - 76.0 - 12.0);
        let name_top = 40.0 - (name.len() as f64 * 19.0 + 16.0) / 2.0;
        for (i, line) in name.iter().enumerate() {
            c.text(70.0, name_top + i as f64 * 19.0, line, SIZE, Weight::Bold, t.text);
        }
        let version = format!("Version {}", env!("CARGO_PKG_VERSION"));
        c.text(70.0, name_top + name.len() as f64 * 19.0, &version, SMALL - 0.5, Weight::Regular, t.secondary);

        let mut y = 80.0;
        for (i, (page, icon, label)) in pages.into_iter().enumerate() {
            if i == 5 {
                y += 10.0;
                c.text(22.0, y, "Displays", SMALL, Weight::Bold, t.secondary);
                y += 24.0;
            }
            let r = Rect::new(10.0, y, SIDEBAR - 20.0, 34.0);
            let hovered = mouse.is_some_and(|m| r.contains(m));
            if page == self.page {
                c.fill_rect(r, 7.0, t.selected);
            } else if hovered {
                c.fill_rect(r, 7.0, t.hover);
            }
            sidebar_icon(c, icon, 22.0, y + 9.0, t.text);
            let width = SIDEBAR - 20.0 - 50.0;
            let mut label = label;
            while c.text_width(&label, SIZE, Weight::Regular) > width && label.chars().count() > 2 {
                label.pop();
                label = format!("{}…", label.trim_end_matches('…').trim_end());
            }
            c.text(48.0, y + 7.0, &label, SIZE, Weight::Regular, t.text);
            hits.push((r, Action::Page(page)));
            y += 36.0;
        }

        // The open preset list, over everything.
        if let (Some(id), Some(anchor)) = (&self.picker, picker) {
            hits.push((Rect::new(0.0, 0.0, WIDTH, HEIGHT), Action::Dismiss));
            let item_height = 30.0;
            let width = GRID_PRESETS
                .iter()
                .map(|p| c.text_width(&preset_label(p.name, p.columns, p.rows), SIZE, Weight::Regular))
                .fold(anchor.width, f64::max)
                + 48.0;
            let height = GRID_PRESETS.len() as f64 * item_height + 12.0;
            let x = (anchor.max_x() - width).max(SIDEBAR + 8.0);
            let y = if anchor.max_y() + 4.0 + height <= HEIGHT - 8.0 { anchor.max_y() + 4.0 } else { anchor.y - 4.0 - height };
            let popup = Rect::new(x, y, width, height);
            c.fill_rect(Rect::new(x - 1.0, y + 1.0, width + 2.0, height + 2.0), 11.0, BLACK.alpha(30));
            c.fill_rect(popup, 10.0, t.popup);
            c.stroke_rect(popup, 10.0, 1.0, t.border);
            for (i, p) in GRID_PRESETS.iter().enumerate() {
                let r = Rect::new(x + 6.0, y + 6.0 + i as f64 * item_height, width - 12.0, item_height);
                if self.mouse.is_some_and(|m| r.contains(m)) {
                    c.fill_rect(r, 6.0, t.accent);
                }
                let hovered = self.mouse.is_some_and(|m| r.contains(m));
                let color = if hovered { WHITE } else { t.text };
                if current == Some((p.columns, p.rows)) {
                    c.stroke(r.x + 10.0, r.y + 15.0, 1.8, color, |pb| {
                        pb.move_to(0.0, 0.0);
                        pb.line_to(3.5, 3.5);
                        pb.line_to(10.0, -4.0);
                    });
                }
                c.text(r.x + 30.0, r.y + 5.0, &preset_label(p.name, p.columns, p.rows), SIZE, Weight::Regular, color);
                hits.push((r, Action::Preset(id.clone(), i)));
            }
        }
        self.hits = hits;
    }

    fn page_content(&self, f: &mut Form) {
        let s = &self.settings;
        if let Some(error) = &self.error {
            f.y += 20.0;
            let message = format!("The settings file has an error, fix it to change the settings here: {error}");
            f.section(None, vec![Row::warning(message)], None);
            f.y -= 20.0;
        }
        let switch = |key: &'static str| Control::Switch(flag(s, key), Action::Toggle(key));
        match &self.page {
            Page::General => {
                f.title("General", None);
                f.section(None, vec![
                    Row::new("Enabled", switch("enabled"))
                        .subtitle("Turn off to leave the keys and the mouse alone."),
                    Row::new("Launch at login", Control::Switch(self.launch_at_login, Action::LaunchAtLogin)),
                    Row::new("Raise window", switch("raise-window"))
                        .subtitle("Bring the window to the front when you start moving or resizing it."),
                ], None);
                let path = self.settings_path.display().to_string();
                let home = std::env::var("HOME").unwrap_or_default();
                let path = match path.strip_prefix(&home) {
                    Some(rest) if !home.is_empty() => format!("~{rest}"),
                    _ => path,
                };
                f.section(Some("Settings File"), vec![
                    Row::new("settings.toml", Control::Button("Open", Some(Action::OpenFile))).subtitle(path),
                ], Some("Changes here are saved to this file right away. It can also be edited by hand: \
                         Souris facile pingouin reads it again whenever it changes."));
            }
            Page::Move | Page::Resize => {
                let moving = self.page == Page::Move;
                let (title, key, mods) = if moving {
                    ("Move", "move-modifiers", s.move_mods())
                } else {
                    ("Resize", "resize-modifiers", s.resize_mods())
                };
                f.title(title, None);
                let hold = if mods.is_empty() { "Off".to_string() } else { format!("Hold {}", mods.label()) };
                let footer = if moving {
                    "Hold these keys and move the mouse to move the window under the pointer. Turn all off to disable."
                } else {
                    "Hold these keys and move the mouse to resize the window under the pointer. Turn all off to disable."
                };
                f.section(None, vec![Row::new("Keys", Control::Keys(key, mods)).subtitle(hold)], Some(footer));
                let warnings = settings::warnings(&s.move_modifiers, &s.resize_modifiers);
                if !warnings.is_empty() {
                    f.section(None, warnings.into_iter().map(Row::warning).collect(), None);
                }
                if !moving {
                    f.section(None, vec![
                        Row::new("Resize from nearest corner", switch("resize-nearest-corner"))
                            .subtitle("Otherwise always from the bottom-right corner."),
                    ], None);
                }
            }
            Page::Snapping => {
                f.title("Snapping", None);
                let threshold = s.snap_threshold.round() as i64;
                let value = if threshold == 0 { "Off".to_string() } else { format!("{threshold} px") };
                let step = 4;
                f.section(Some("Edge Snapping"), vec![
                    Row::new("Snap distance", Control::Stepper(value, (threshold > 0).then_some(Action::Snap(-step)),
                                                               (threshold < 200).then_some(Action::Snap(step)))),
                    Row::new("Snap to other windows", switch("snap-to-windows")).enabled(threshold > 0),
                ], Some("Window edges snap to display edges and grid lines, and to other windows', when they come \
                         close. 0 turns snapping off."));
                f.section(Some("Grid"), vec![
                    Row::new("Show grid", switch("show-grid")),
                    Row::new("Dim other windows", switch("dim-windows")).subtitle("Needs a compositor."),
                ], Some("While moving or resizing, window edges snap to the grid of the display under the \
                         pointer when they come close to its lines. Each display's grid is set on its own page."));
            }
            Page::Send => {
                f.title("Send to Display", None);
                let can = !s.move_mods().is_empty();
                f.section(None, vec![
                    Row::new("Swipe or scroll", switch("send-with-scroll"))
                        .subtitle("Swipe two fingers on a touchpad toward a display. A mouse wheel goes to the \
                                   next or previous display.")
                        .enabled(can),
                    Row::new("Arrow keys", switch("send-with-arrows"))
                        .subtitle("Press an arrow key to go to the display in that direction.")
                        .enabled(can),
                ], Some("Hold the move keys over a window and swipe, scroll or press an arrow key. The pointer \
                         goes with the window."));
                let empty = self.memory().is_empty();
                f.section(Some("Window Sizes"), vec![
                    Row::new("Remember window sizes per display", switch("remember-window-frames"))
                        .subtitle("A window sent or dragged to a display gets the size and place its app's \
                                   windows last had there. Otherwise it keeps its relative place and size."),
                    Row::new("Remembered sizes",
                             Control::Button("Forget All", (!empty).then_some(Action::ForgetAll)))
                        .subtitle(if empty { "None yet." } else { "Forget them on every display." }),
                ], None);
            }
            Page::Display(id) => {
                let Some((display, (columns, rows))) = self.grid(id) else {
                    f.title("Display", None);
                    f.section(None, vec![Row::new("This display isn't plugged in anymore.", Control::None)], None);
                    return;
                };
                let mut info = vec![format!("{} × {}", display.frame.width, display.frame.height)];
                let inches = display.size_mm.width.hypot(display.size_mm.height) / 25.4;
                if inches >= 10.0 {
                    info.push(format!("{inches:.0}″"));
                }
                if self.displays.first().is_some_and(|d| d.id == *id) {
                    info.push("primary".into());
                }
                f.title(&display.name, Some(&info.join(", ")));
                f.display_preview(display, columns, rows);
                let preset = GRID_PRESETS.iter().find(|p| (p.columns, p.rows) == (columns, rows));
                let label = preset.map_or("Custom".to_string(), |p| p.name.to_string());
                let set = s.displays.contains_key(id);
                let suits = sensible_grid(display.frame.size(), display.size_mm) == (columns, rows);
                let (min, max) = (*GRID_RANGE.start(), *GRID_RANGE.end());
                let stepper = |value: usize, dc: i64, dr: i64| {
                    Control::Stepper(value.to_string(), (value > min).then(|| Action::Grid(id.clone(), -dc, -dr)),
                                     (value < max).then(|| Action::Grid(id.clone(), dc, dr)))
                };
                let mut size = Row::new("Grid size", Control::Picker(label, Action::Picker(id.clone())));
                if !set && suits {
                    size = size.subtitle("Suits this display's size.");
                }
                f.section(Some("Grid"), vec![
                    size,
                    Row::new("Columns", stepper(columns, 1, 0)),
                    Row::new("Rows", stepper(rows, 0, 1)),
                    Row::new("Show it on the display", Control::Button("Preview", Some(Action::Preview(id.clone())))),
                ], Some("Window edges snap to its lines when they come close. Remembered for this display, and used again \
                         whenever it's plugged in. A new display gets a grid suiting its size."));
                let count = self.memory().count(id);
                let apps = match count {
                    0 => "None yet.".to_string(),
                    1 => "1 app.".into(),
                    n => format!("{n} apps."),
                };
                f.section(Some("Remembered Window Sizes"), vec![
                    Row::new("Window sizes on this display",
                             Control::Button("Forget", (count > 0).then(|| Action::ForgetFrames(id.clone()))))
                        .subtitle(apps),
                ], Some("Where apps' windows last were on this display, used when a window is sent or dragged here."));
            }
            Page::Unplugged => {
                f.title("Unplugged Displays", None);
                let rows = self
                    .unplugged()
                    .into_iter()
                    .map(|(id, name, summary)| {
                        Row::new(name, Control::Button("Forget", Some(Action::ForgetDisplay(id)))).subtitle(summary)
                    })
                    .collect();
                f.section(None, rows, Some("Settings of displays that aren't plugged in. They apply again as soon \
                                            as the display is. Forgetting a display also forgets its remembered \
                                            window sizes."));
            }
        }
    }

    fn hit(&self, point: Point) -> Option<Action> {
        self.hits.iter().rev().find(|(r, _)| r.contains(point)).map(|(_, a)| a.clone())
    }

    fn present(&self) -> Res<()> {
        surface::present(&self.x.conn, self.window, self.gc, self.depth, &self.canvas)
    }

    fn keysym(&self, code: Keycode) -> u32 {
        self.keys.iter().find(|k| k.0 == code).map_or(0, |k| k.1)
    }
}

fn preset_label(name: &str, columns: usize, rows: usize) -> String {
    format!("{name}  ·  {columns} × {rows}")
}

// The first keysym of each keycode.
pub(super) fn keymap(x: &X11) -> Res<Vec<(Keycode, u32)>> {
    let setup = x.conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let mapping = x.conn.get_keyboard_mapping(min, max - min + 1)?.reply()?;
    let per = mapping.keysyms_per_keycode as usize;
    Ok((min..=max).zip(mapping.keysyms.chunks(per)).map(|(code, syms)| (code, syms[0])).collect())
}

pub fn run(settings_path: &Path, frames_path: &Path) -> Res<()> {
    let x = X11::connect()?;
    let a = &x.atoms;

    // A single settings window: bring back the one that's open.
    let owner = x.conn.get_selection_owner(a.SETTINGS_INSTANCE)?.reply()?.owner;
    if owner != 0 {
        x.send_client_message(owner, a._NET_ACTIVE_WINDOW, [2, CURRENT_TIME, 0, 0, 0]);
        x.conn.flush()?;
        return Ok(());
    }

    let scale = surface::scale(&x.conn, x.root);
    let fonts = Fonts::load()?;
    let screen = &x.conn.setup().roots[x.screen];
    let depth = screen.root_depth;
    let (w, h) = ((WIDTH * scale as f64).round() as u16, (HEIGHT * scale as f64).round() as u16);
    let window = x.conn.generate_id()?;
    let events = EventMask::EXPOSURE
        | EventMask::BUTTON_PRESS
        | EventMask::BUTTON_RELEASE
        | EventMask::POINTER_MOTION
        | EventMask::LEAVE_WINDOW
        | EventMask::KEY_PRESS
        | EventMask::STRUCTURE_NOTIFY;
    let theme = Theme::new(surface::dark());
    let background = u32::from_be_bytes([0, theme.sidebar.0, theme.sidebar.1, theme.sidebar.2]);
    x.conn.create_window(COPY_FROM_PARENT as u8, window, x.root, 0, 0, w, h, 0, WindowClass::INPUT_OUTPUT,
                         COPY_FROM_PARENT, &CreateWindowAux::new().event_mask(events).background_pixel(background))?;
    surface::describe(&x.conn, window, a, "Souris facile pingouin Settings")?;
    let mut hints = WmSizeHints::new();
    hints.min_size = Some((w as i32, h as i32));
    hints.max_size = Some((w as i32, h as i32));
    hints.set_normal_hints(&x.conn, window)?;
    let mut wm_hints = WmHints::new();
    wm_hints.input = Some(true);
    wm_hints.set(&x.conn, window)?;
    x.conn.change_property32(x11rb::protocol::xproto::PropMode::REPLACE, window, a.WM_PROTOCOLS,
                             x11rb::protocol::xproto::AtomEnum::ATOM, &[a.WM_DELETE_WINDOW])?;
    x.conn.set_selection_owner(window, a.SETTINGS_INSTANCE, CURRENT_TIME)?;
    x.conn.randr_select_input(x.root, NotifyMask::SCREEN_CHANGE | NotifyMask::CRTC_CHANGE)?;
    let gc = x.conn.generate_id()?;
    x.conn.create_gc(gc, window, &CreateGCAux::new().graphics_exposures(0))?;
    x.conn.map_window(window)?;

    let displays = super::displays::displays(&x).unwrap_or_default();
    let keys = keymap(&x)?;
    let canvas = Canvas::new(WIDTH, HEIGHT, scale, fonts.clone());
    let mut app = App {
        x,
        window,
        gc,
        depth,
        scale,
        fonts,
        theme,
        keys,
        settings_path: settings_path.to_path_buf(),
        frames: MemoryStore::File(frames_path.to_path_buf()),
        settings: Settings::default(),
        error: None,
        settings_time: None,
        displays,
        launch_at_login: false,
        page: Page::General,
        scroll: 0.0,
        content_height: 0.0,
        mouse: None,
        pressed: None,
        hits: Vec::new(),
        picker: None,
        preview_until: None,
        canvas,
    };
    app.reload();
    app.draw();

    let mut last_check = Instant::now();
    let mut dirty = false;
    let to_logical = |x: i16, y: i16| Point::new(x as f64 / scale as f64, y as f64 / scale as f64);
    loop {
        while let Some(event) = app.x.conn.poll_for_event()? {
            match event {
                Event::Expose(e) if e.count == 0 => app.present()?,
                Event::MotionNotify(e) => {
                    let before = app.mouse.and_then(|m| app.hit(m));
                    app.mouse = Some(to_logical(e.event_x, e.event_y));
                    if app.mouse.and_then(|m| app.hit(m)) != before {
                        dirty = true;
                    }
                }
                Event::LeaveNotify(_) => {
                    app.mouse = None;
                    dirty = true;
                }
                Event::ButtonPress(e) => {
                    let point = to_logical(e.event_x, e.event_y);
                    match e.detail {
                        1 => app.pressed = app.hit(point),
                        4 | 5 if app.picker.is_none() && point.x > SIDEBAR => {
                            app.scroll += if e.detail == 4 { -48.0 } else { 48.0 };
                            dirty = true;
                        }
                        _ => {}
                    }
                }
                Event::ButtonRelease(e) if e.detail == 1 => {
                    let point = to_logical(e.event_x, e.event_y);
                    if let Some(action) = app.pressed.take().filter(|a| app.hit(point).as_ref() == Some(a)) {
                        app.act(action);
                        dirty = true;
                    }
                }
                Event::KeyPress(e) => match app.keysym(e.detail) {
                    XK_ESCAPE if app.picker.is_some() => {
                        app.picker = None;
                        dirty = true;
                    }
                    XK_ESCAPE => return Ok(()),
                    XK_W | XK_Q if e.state.contains(x11rb::protocol::xproto::KeyButMask::CONTROL) => return Ok(()),
                    _ => {}
                },
                Event::ClientMessage(e) if e.data.as_data32()[0] == app.x.atoms.WM_DELETE_WINDOW => return Ok(()),
                Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_) => {
                    app.displays = super::displays::displays(&app.x).unwrap_or_default();
                    dirty = true;
                }
                Event::MappingNotify(_) => app.keys = keymap(&app.x)?,
                _ => {}
            }
        }

        if app.preview_until.is_some_and(|t| Instant::now() >= t) {
            app.preview_until = None;
            app.x.overlays.hide_grid(&app.x.conn);
        }
        // Changed elsewhere: by hand, or from the tray menu.
        if last_check.elapsed() >= Duration::from_secs(1) {
            last_check = Instant::now();
            if modified(&app.settings_path) != app.settings_time {
                app.reload();
                dirty = true;
            }
        }
        if dirty {
            dirty = false;
            app.draw();
            // Drawing may change what's under the pointer, or the scroll.
            app.present()?;
        }

        let mut timeout = Duration::from_secs(1).saturating_sub(last_check.elapsed());
        if let Some(t) = app.preview_until {
            timeout = timeout.min(t.saturating_duration_since(Instant::now()));
        }
        surface::wait(&app.x.conn, Some(timeout.max(Duration::from_millis(10))))?;
    }
}
