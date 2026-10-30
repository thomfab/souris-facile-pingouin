use crate::geometry::{
    display_after, display_toward, divide, fit_to_grid, frame_distance, snap_delta, transfer, Direction, Point, Rect,
    Size,
};
use crate::gestures::{scroll_phase, SwipeTracker};
use crate::memory::{MemoryStore, RememberedFrame};
use crate::platform::{display_containing, Display, Platform};
use crate::settings::{KeyAction, Mods, Settings};

const MIN_WIDTH: f64 = 100.0;
const MIN_HEIGHT: f64 = 60.0;
// Wheel events closer than this (in seconds) are one spin.
const WHEEL_PAUSE: f64 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    None,
    Move,
    Resize,
}

enum SendTarget {
    Toward(Direction),
    // The next (1) or previous (-1) display.
    Cycle(i32),
}

struct Grid {
    area: Rect,
    columns: usize,
    rows: usize,
}

// Moves or resizes the window under the pointer while modifier keys are
// held. The platform feeds it input and carries out what it decides.
pub struct Mover<P: Platform> {
    // Replaced only while no operation is running.
    pub settings: Settings,
    memory: MemoryStore,
    op: Op,
    window: Option<P::Window>,
    start: Point,
    start_rect: Rect,
    // Where the pointer sits inside the window, as a fraction of its size.
    grab_ratio: Point,
    // Which edges a resize moves: -1 for left/top, 1 for right/bottom.
    resize_dir_x: f64,
    resize_dir_y: f64,
    // Whether the button is held down with the keys, to snap to the grid.
    grid_click: bool,
    // Releasing that button ends the operation: the keys then do nothing
    // until they're released.
    wait_for_keys_up: bool,
    dimmed: bool,
    // None until the window is first moved, if it starts maximized.
    last_frame: Option<Rect>,
    // Unmaximized when it first moves, rather than when the keys are
    // pressed, as they're also held for ordinary shortcuts.
    maximized: bool,
    // The window's app, to remember where its windows were on each display.
    app: Option<String>,
    resizable: bool,
    min_size: Size,
    max_size: Option<Size>,
    // The frame when the operation began, until the window is dragged to
    // another display, when it's remembered as its frame on the first one.
    begin_frame: Option<Rect>,
    // The display under the pointer while moving.
    move_display: Option<String>,
    displays: Vec<Display>,
    other_rects: Vec<Rect>,
    swipe: SwipeTracker,
    last_wheel_send: f64,
}

impl<P: Platform> Mover<P> {
    pub fn new(settings: Settings, memory: MemoryStore) -> Self {
        Mover {
            settings,
            memory,
            op: Op::None,
            window: None,
            start: Point::default(),
            start_rect: Rect::default(),
            grab_ratio: Point::default(),
            resize_dir_x: 1.0,
            resize_dir_y: 1.0,
            grid_click: false,
            wait_for_keys_up: false,
            dimmed: false,
            last_frame: None,
            maximized: false,
            app: None,
            resizable: false,
            min_size: Size::default(),
            max_size: None,
            begin_frame: None,
            move_display: None,
            displays: Vec::new(),
            other_rects: Vec::new(),
            swipe: SwipeTracker::default(),
            last_wheel_send: f64::NEG_INFINITY,
        }
    }

    // Whether no operation is running, so the settings can be replaced.
    pub fn is_idle(&self) -> bool {
        self.op == Op::None
    }

    // Called after every pointer or modifier change, with their state.
    pub fn update(&mut self, p: &mut P, point: Point, mods: Mods) {
        let action = if self.settings.enabled { self.settings.action(mods) } else { KeyAction::None };
        if action == KeyAction::None {
            self.wait_for_keys_up = false;
        }
        let new_op = match if self.wait_for_keys_up { KeyAction::None } else { action } {
            KeyAction::None => Op::None,
            KeyAction::Move => Op::Move,
            KeyAction::Resize => Op::Resize,
        };

        if new_op != self.op {
            self.end_operation(p);
            if new_op != Op::None {
                self.begin_operation(p, new_op, point, None);
            }
        } else {
            match self.op {
                Op::Move => self.update_move(p, point),
                Op::Resize => self.update_resize(p, point),
                Op::None => {}
            }
        }
        self.update_dimmer(p);
    }

    // The other windows are dimmed while snapping to the grid.
    fn update_dimmer(&mut self, p: &mut P) {
        let dim = self.op != Op::None && self.grid_click && self.settings.dim_windows;
        let Some(window) = &self.window else { return };
        if dim == self.dimmed {
            return;
        }
        if dim {
            p.show_dimmer(window);
        } else {
            p.hide_dimmer();
        }
        self.dimmed = dim;
    }

    // `frame` is where the window is known to be, when the platform may not
    // have caught up yet.
    fn begin_operation(&mut self, p: &mut P, new_op: Op, point: Point, known: Option<(P::Window, Rect)>) {
        let Some(window) = known.as_ref().map(|k| k.0.clone()).or_else(|| p.window_at(point)) else { return };
        let Some(info) = p.window_info(&window) else { return };
        let rect = known.map_or(info.frame, |k| k.1);
        let allowed = if new_op == Op::Move { info.can_move } else { info.can_resize };
        if !allowed || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        self.op = new_op;
        // In case the button's release was missed.
        if self.grid_click && !p.button_held() {
            self.grid_click = false;
        }
        self.start = point;
        self.start_rect = rect;
        self.maximized = info.maximized;
        self.last_frame = if info.maximized { None } else { Some(rect) };
        self.app = info.app;
        self.resizable = info.can_resize;
        self.min_size = Size::new(info.min_size.width.max(MIN_WIDTH), info.min_size.height.max(MIN_HEIGHT));
        self.max_size = info.max_size;
        self.begin_frame = Some(rect);
        self.displays = p.displays();
        self.move_display = display_containing(&self.displays, point).map(|i| self.displays[i].id.clone());
        self.grab_ratio = Point::new((point.x - rect.min_x()) / rect.width, (point.y - rect.min_y()) / rect.height);

        if self.settings.resize_nearest_corner {
            self.resize_dir_x = if self.grab_ratio.x < 0.5 { -1.0 } else { 1.0 };
            self.resize_dir_y = if self.grab_ratio.y < 0.5 { -1.0 } else { 1.0 };
        } else {
            self.resize_dir_x = 1.0;
            self.resize_dir_y = 1.0;
        }

        // Never snap to the window's own starting frame.
        self.other_rects = p.other_windows(&window).into_iter().filter(|r| frame_distance(r, &rect) > 1.0).collect();

        if self.settings.raise_window {
            p.raise(&window);
        }
        self.window = Some(window);
    }

    fn end_operation(&mut self, p: &mut P) {
        if self.op != Op::None && self.last_frame != self.begin_frame {
            if let Some(frame) = self.last_frame {
                let (app, displays) = (self.app.take(), std::mem::take(&mut self.displays));
                self.remember(frame, app.as_deref(), &displays);
            }
        }
        self.op = Op::None;
        self.window = None;
        self.app = None;
        self.begin_frame = None;
        self.move_display = None;
        self.dimmed = false;
        self.displays.clear();
        self.other_rects.clear();
        p.hide_grid();
        p.hide_dimmer();
    }

    // Sets the frame, unmaximizing the window first if needed. Ends the
    // operation if the window is gone.
    fn apply(&mut self, p: &mut P, frame: Rect) {
        let Some(window) = self.window.clone() else { return };
        let resized = self.last_frame.is_none_or(|last| last.size() != frame.size());
        self.last_frame = Some(frame);
        if self.maximized {
            p.unmaximize(&window);
            self.maximized = false;
        }
        let ok = if resized { p.set_frame(&window, frame) } else { p.set_origin(&window, frame.origin()) };
        if !ok {
            self.end_operation(p);
        }
    }

    fn update_move(&mut self, p: &mut P, point: Point) {
        self.enter_display(point);
        let size = self.start_rect.size();
        let mut x = (point.x - self.grab_ratio.x * size.width).round();
        let mut y = (point.y - self.grab_ratio.y * size.height).round();

        let grid = self.current_grid(p, point);
        x += self.snap(&[x, x + size.width], &self.snap_lines_x(y, size.height, &grid), &grid);
        y += self.snap(&[y, y + size.height], &self.snap_lines_y(x, size.width, &grid), &grid);

        let frame = Rect::new(x, y, size.width, size.height);
        if self.last_frame != Some(frame) {
            self.apply(p, frame);
        }
    }

    // When a drag crosses onto another display, takes the size its app's
    // windows last had there, keeping the pointer at the same spot in it.
    fn enter_display(&mut self, point: Point) {
        let Some(index) = display_containing(&self.displays, point) else { return };
        let display = &self.displays[index];
        if self.move_display.as_deref() == Some(&display.id) {
            return;
        }
        self.move_display = Some(display.id.clone());
        let (area, id) = (display.work_area, display.id.clone());
        if let Some(begin) = self.begin_frame.take() {
            let displays = std::mem::take(&mut self.displays);
            self.remember(begin, self.app.clone().as_deref(), &displays);
            self.displays = displays;
        }
        if self.resizable {
            if let Some(remembered) = self.remembered(self.app.as_deref(), &id) {
                self.start_rect = Rect::from_origin_size(
                    self.start_rect.origin(),
                    remembered.restored(&area, false).size(),
                );
            }
        }
    }

    fn update_resize(&mut self, p: &mut P, point: Point) {
        let dx = point.x - self.start.x;
        let dy = point.y - self.start.y;
        let r = self.start_rect;
        let (mut left, mut right, mut top, mut bottom) = (r.min_x(), r.max_x(), r.min_y(), r.max_y());
        let (dir_x, dir_y) = (self.resize_dir_x, self.resize_dir_y);

        if dir_x > 0.0 { right += dx } else { left += dx }
        if dir_y > 0.0 { bottom += dy } else { top += dy }

        let grid = self.current_grid(p, point);
        let lines_x = self.snap_lines_x(top, bottom - top, &grid);
        if dir_x > 0.0 {
            right += self.snap(&[right], &lines_x, &grid);
        } else {
            left += self.snap(&[left], &lines_x, &grid);
        }
        let lines_y = self.snap_lines_y(left, right - left, &grid);
        if dir_y > 0.0 {
            bottom += self.snap(&[bottom], &lines_y, &grid);
        } else {
            top += self.snap(&[top], &lines_y, &grid);
        }

        // Keep within the window's own limits, moving only the pulled edges.
        let width = (right - left).max(self.min_size.width).min(self.max_size.map_or(f64::INFINITY, |s| s.width));
        let height =
            (bottom - top).max(self.min_size.height).min(self.max_size.map_or(f64::INFINITY, |s| s.height));
        if dir_x > 0.0 { right = left + width } else { left = right - width }
        if dir_y > 0.0 { bottom = top + height } else { top = bottom - height }

        let frame = Rect::new(left.round(), top.round(), (right - left).round(), (bottom - top).round());
        if self.last_frame != Some(frame) {
            self.apply(p, frame);
        }
    }

    // Pressing the button while holding the keys snaps to the grid until
    // it's released. Returns true to keep the click from the app under the
    // pointer.
    pub fn click_down(&mut self, mods: Mods) -> bool {
        if !self.settings.enabled || self.wait_for_keys_up || self.settings.action(mods) == KeyAction::None {
            return false;
        }
        self.grid_click = true;
        true
    }

    pub fn click_up(&mut self) -> bool {
        if !self.grid_click {
            return false;
        }
        self.grid_click = false;
        // The window stays where it snapped.
        self.wait_for_keys_up = true;
        true
    }

    // Sending to another display

    fn holds_move_keys(&self, mods: Mods) -> bool {
        let move_mods = self.settings.move_mods();
        self.settings.enabled && !self.wait_for_keys_up && !move_mods.is_empty() && mods == move_mods
    }

    // A scroll while holding the move keys. `dx` and `dy` follow the fingers
    // or the wheel: x going right, y going down. `continuous` is for
    // touchpads, `time` in seconds. Returns true to swallow the scroll.
    pub fn scroll(&mut self, p: &mut P, mods: Mods, dx: f64, dy: f64, continuous: bool, time: f64) -> bool {
        if !self.holds_move_keys(mods) || !self.settings.send_with_scroll {
            self.swipe.reset();
            return false;
        }
        if continuous {
            // A touchpad swipe goes toward a display.
            if let Some(direction) = self.swipe.feed(scroll_phase::NONE, false, dx, dy, time) {
                self.send(p, SendTarget::Toward(direction));
            }
        } else if (dx != 0.0 || dy != 0.0) && time - self.last_wheel_send > WHEEL_PAUSE {
            // A wheel notch goes to the next or previous display. A fast
            // spin sends a burst of events: only the first one counts.
            self.last_wheel_send = time;
            let forward = if dy.abs() >= dx.abs() { dy > 0.0 } else { dx > 0.0 };
            self.send(p, SendTarget::Cycle(if forward { 1 } else { -1 }));
        }
        true
    }

    // An arrow key pressed while holding the move keys. Returns true to
    // swallow it.
    pub fn arrow(&mut self, p: &mut P, mods: Mods, direction: Direction, repeat: bool) -> bool {
        if !self.holds_move_keys(mods) || !self.settings.send_with_arrows {
            return false;
        }
        // Holding the key down doesn't keep sending.
        if !repeat {
            self.send(p, SendTarget::Toward(direction));
        }
        true
    }

    // Sends the window under the pointer (or the one being moved) to another
    // display, at the same relative place and size, and the pointer with it.
    fn send(&mut self, p: &mut P, target: SendTarget) {
        let point = p.pointer();
        let moving = self.op == Op::Move;
        let Some(window) = (if moving { self.window.clone() } else { p.window_at(point) }) else { return };
        let Some(info) = p.window_info(&window) else { return };
        // The platform may not have caught up with the last move yet.
        let frame = if moving { self.last_frame.unwrap_or(info.frame) } else { info.frame };
        if !info.can_move || frame.width <= 0.0 || frame.height <= 0.0 {
            return;
        }

        let displays = p.displays();
        let Some(from) = display_containing(&displays, frame.center()) else { return };
        let frames: Vec<Rect> = displays.iter().map(|d| d.frame).collect();
        let to = match target {
            SendTarget::Toward(direction) => display_toward(direction, from, &frames),
            SendTarget::Cycle(step) => display_after(from, step, &frames),
        };
        let Some(to) = to else { return };

        let area = displays[to].work_area;
        let resizable = info.can_resize;
        let app = info.app.as_deref();
        self.remember(frame, app, &displays);
        // Where its app's windows last were on that display, or the same
        // relative place and size as here.
        let mut new_frame = match self.remembered(app, &displays[to].id) {
            Some(remembered) => remembered.restored(&area, !resizable),
            None => transfer(&frame, &displays[from].work_area, &area, !resizable),
        };
        if self.grid_click && resizable {
            let (columns, rows) = self.settings.grid(&displays[to]);
            new_frame = fit_to_grid(&new_frame, &area, columns, rows);
        }

        self.end_operation(p);
        if info.maximized {
            p.unmaximize(&window);
        }
        let ok = if resizable { p.set_frame(&window, new_frame) } else { p.set_origin(&window, new_frame.origin()) };
        if !ok {
            return;
        }

        // The pointer follows, at the same spot in the window.
        let ratio_x = ((point.x - frame.min_x()) / frame.width).clamp(0.0, 1.0);
        let ratio_y = ((point.y - frame.min_y()) / frame.height).clamp(0.0, 1.0);
        let new_point = Point::new(
            (new_frame.min_x() + ratio_x * new_frame.width).round(),
            (new_frame.min_y() + ratio_y * new_frame.height).round(),
        );
        p.warp_pointer(new_point);

        // Carry on moving it from there, even if other windows cover that spot.
        if moving {
            self.begin_operation(p, Op::Move, new_point, Some((window, new_frame)));
        }
    }

    // Remembering frames per display

    // Records `frame` as where the app's windows last were on the display
    // that holds most of it (the one under its center).
    fn remember(&mut self, frame: Rect, app: Option<&str>, displays: &[Display]) {
        let center = frame.center();
        let Some(app) = app.filter(|_| self.settings.remember_window_frames) else { return };
        let Some(display) = displays.iter().find(|d| d.frame.contains(center)) else { return };
        let mut memory = self.memory.load();
        memory.remember(frame, display.work_area, app, &display.id);
        self.memory.save(memory);
    }

    fn remembered(&self, app: Option<&str>, display: &str) -> Option<RememberedFrame> {
        let app = app.filter(|_| self.settings.remember_window_frames)?;
        self.memory.load().frame(app, display)
    }

    // Snapping

    fn snap(&self, edges: &[f64], lines: &[f64], grid: &Option<Grid>) -> f64 {
        // The grid always snaps; edge snapping only within the threshold.
        let threshold = if grid.is_some() { f64::INFINITY } else { self.settings.snap_threshold };
        if threshold <= 0.0 {
            return 0.0;
        }
        snap_delta(edges, lines, threshold)
    }

    // Vertical lines a window spanning [top, top + height] can snap to.
    fn snap_lines_x(&self, top: f64, height: f64, grid: &Option<Grid>) -> Vec<f64> {
        if let Some(grid) = grid {
            return divide(grid.area.min_x(), grid.area.width, grid.columns);
        }
        let mut lines: Vec<f64> = self.displays.iter().flat_map(|d| [d.work_area.min_x(), d.work_area.max_x()]).collect();
        if self.settings.snap_to_windows {
            let t = self.settings.snap_threshold;
            for r in &self.other_rects {
                if r.min_y() - t < top + height && r.max_y() + t > top {
                    lines.extend([r.min_x(), r.max_x()]);
                }
            }
        }
        lines
    }

    // Horizontal lines a window spanning [left, left + width] can snap to.
    fn snap_lines_y(&self, left: f64, width: f64, grid: &Option<Grid>) -> Vec<f64> {
        if let Some(grid) = grid {
            return divide(grid.area.min_y(), grid.area.height, grid.rows);
        }
        let mut lines: Vec<f64> = self.displays.iter().flat_map(|d| [d.work_area.min_y(), d.work_area.max_y()]).collect();
        if self.settings.snap_to_windows {
            let t = self.settings.snap_threshold;
            for r in &self.other_rects {
                if r.min_x() - t < left + width && r.max_x() + t > left {
                    lines.extend([r.min_y(), r.max_y()]);
                }
            }
        }
        lines
    }

    // The grid of the display under the pointer while the button is held,
    // or None (edge snapping then applies).
    fn current_grid(&self, p: &mut P, point: Point) -> Option<Grid> {
        let index = display_containing(&self.displays, point).filter(|_| self.grid_click);
        let Some(index) = index else {
            p.hide_grid();
            return None;
        };
        let display = &self.displays[index];
        let (columns, rows) = self.settings.grid(display);
        if self.settings.show_grid {
            p.show_grid(display.work_area, columns, rows);
        }
        Some(Grid { area: display.work_area, columns, rows })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::WindowMemory;
    use crate::platform::WindowInfo;
    use crate::settings::Modifier;

    // Two displays side by side, with a 32 px panel on top of the first.
    struct Fake {
        pointer: Point,
        windows: Vec<(u32, WindowInfo)>,
        displays: Vec<Display>,
        grid: Option<(Rect, usize, usize)>,
        dimmed_below: Option<u32>,
        unmaximized: Vec<u32>,
        raised: Vec<u32>,
    }

    impl Fake {
        fn new() -> Self {
            let display = |id: &str, frame: Rect, work_area: Rect| Display {
                id: id.into(),
                name: id.into(),
                frame,
                work_area,
                size_mm: Size::default(),
                builtin: false,
            };
            Fake {
                pointer: Point::default(),
                windows: Vec::new(),
                displays: vec![
                    display("left", Rect::new(0.0, 0.0, 1920.0, 1080.0), Rect::new(0.0, 32.0, 1920.0, 1048.0)),
                    display("right", Rect::new(1920.0, 0.0, 2560.0, 1440.0), Rect::new(1920.0, 0.0, 2560.0, 1440.0)),
                ],
                grid: None,
                dimmed_below: None,
                unmaximized: Vec::new(),
                raised: Vec::new(),
            }
        }

        fn add(&mut self, id: u32, frame: Rect) {
            let info = WindowInfo {
                frame,
                can_move: true,
                can_resize: true,
                maximized: false,
                app: Some(format!("app{id}")),
                min_size: Size::default(),
                max_size: None,
            };
            self.windows.push((id, info));
        }

        fn info(&mut self, id: u32) -> &mut WindowInfo {
            &mut self.windows.iter_mut().find(|w| w.0 == id).unwrap().1
        }

        fn frame(&self, id: u32) -> Rect {
            self.windows.iter().find(|w| w.0 == id).unwrap().1.frame
        }
    }

    impl Platform for Fake {
        type Window = u32;

        fn pointer(&mut self) -> Point {
            self.pointer
        }
        fn warp_pointer(&mut self, point: Point) {
            self.pointer = point;
        }
        fn button_held(&mut self) -> bool {
            true
        }
        // The last window added is on top.
        fn window_at(&mut self, point: Point) -> Option<u32> {
            self.windows.iter().rev().find(|w| w.1.frame.contains(point)).map(|w| w.0)
        }
        fn window_info(&mut self, window: &u32) -> Option<WindowInfo> {
            self.windows.iter().find(|w| w.0 == *window).map(|w| w.1.clone())
        }
        fn other_windows(&mut self, window: &u32) -> Vec<Rect> {
            self.windows.iter().filter(|w| w.0 != *window).map(|w| w.1.frame).collect()
        }
        fn displays(&mut self) -> Vec<Display> {
            self.displays.clone()
        }
        fn set_frame(&mut self, window: &u32, frame: Rect) -> bool {
            self.info(*window).frame = frame;
            true
        }
        fn set_origin(&mut self, window: &u32, origin: Point) -> bool {
            let info = self.info(*window);
            info.frame = Rect::from_origin_size(origin, info.frame.size());
            true
        }
        fn unmaximize(&mut self, window: &u32) {
            self.unmaximized.push(*window);
        }
        fn raise(&mut self, window: &u32) {
            self.raised.push(*window);
        }
        fn show_grid(&mut self, area: Rect, columns: usize, rows: usize) {
            self.grid = Some((area, columns, rows));
        }
        fn hide_grid(&mut self) {
            self.grid = None;
        }
        fn show_dimmer(&mut self, below: &u32) {
            self.dimmed_below = Some(*below);
        }
        fn hide_dimmer(&mut self) {
            self.dimmed_below = None;
        }
    }

    fn move_keys() -> Mods {
        Mods::NONE.with(Modifier::Control).with(Modifier::Alt)
    }

    fn resize_keys() -> Mods {
        move_keys().with(Modifier::Super)
    }

    fn setup() -> (Mover<Fake>, Fake) {
        let mut fake = Fake::new();
        fake.add(1, Rect::new(200.0, 200.0, 800.0, 600.0));
        (Mover::new(Settings::default(), MemoryStore::InMemory(WindowMemory::default())), fake)
    }

    // Moves the pointer to (x, y) holding `mods`.
    fn at(mover: &mut Mover<Fake>, fake: &mut Fake, x: f64, y: f64, mods: Mods) {
        fake.pointer = Point::new(x, y);
        mover.update(fake, fake.pointer, mods);
    }

    #[test]
    fn moving_keeps_the_window_under_the_pointer() {
        let (mut mover, mut fake) = setup();
        at(&mut mover, &mut fake, 500.0, 500.0, move_keys());
        at(&mut mover, &mut fake, 600.0, 450.0, move_keys());
        assert_eq!(fake.frame(1), Rect::new(300.0, 150.0, 800.0, 600.0));
        // Released keys stop it.
        at(&mut mover, &mut fake, 700.0, 450.0, Mods::NONE);
        assert_eq!(fake.frame(1), Rect::new(300.0, 150.0, 800.0, 600.0));
    }

    #[test]
    fn other_keys_do_nothing() {
        let (mut mover, mut fake) = setup();
        let keys = move_keys().with(Modifier::Shift);
        at(&mut mover, &mut fake, 500.0, 500.0, keys);
        at(&mut mover, &mut fake, 600.0, 450.0, keys);
        assert_eq!(fake.frame(1), Rect::new(200.0, 200.0, 800.0, 600.0));
    }

    #[test]
    fn moving_snaps_to_the_work_area_and_other_windows() {
        let (mut mover, mut fake) = setup();
        fake.add(2, Rect::new(1200.0, 100.0, 400.0, 900.0));
        at(&mut mover, &mut fake, 500.0, 500.0, move_keys());
        // The top edge comes within 16 px of the panel.
        at(&mut mover, &mut fake, 500.0, 340.0, move_keys());
        assert_eq!(fake.frame(1).min_y(), 32.0);
        // The right edge comes within 16 px of the other window's left edge.
        at(&mut mover, &mut fake, 690.0, 340.0, move_keys());
        assert_eq!(fake.frame(1).max_x(), 1200.0);
    }

    #[test]
    fn resizing_pulls_the_nearest_corner() {
        let (mut mover, mut fake) = setup();
        // Near the top-left corner.
        at(&mut mover, &mut fake, 300.0, 300.0, resize_keys());
        at(&mut mover, &mut fake, 250.0, 280.0, resize_keys());
        assert_eq!(fake.frame(1), Rect::new(150.0, 180.0, 850.0, 620.0));
    }

    #[test]
    fn resizing_keeps_the_minimum_size_without_moving_the_other_edges() {
        let (mut mover, mut fake) = setup();
        fake.info(1).min_size = Size::new(400.0, 50.0);
        at(&mut mover, &mut fake, 300.0, 300.0, resize_keys());
        at(&mut mover, &mut fake, 900.0, 900.0, resize_keys());
        assert_eq!(fake.frame(1), Rect::new(600.0, 740.0, 400.0, 60.0));
    }

    #[test]
    fn adding_a_key_switches_from_moving_to_resizing() {
        let (mut mover, mut fake) = setup();
        at(&mut mover, &mut fake, 900.0, 700.0, move_keys());
        at(&mut mover, &mut fake, 910.0, 700.0, move_keys());
        at(&mut mover, &mut fake, 910.0, 700.0, resize_keys());
        at(&mut mover, &mut fake, 960.0, 750.0, resize_keys());
        assert_eq!(fake.frame(1), Rect::new(210.0, 200.0, 850.0, 650.0));
    }

    #[test]
    fn holding_the_click_snaps_to_the_grid_until_it_is_released() {
        let (mut mover, mut fake) = setup();
        at(&mut mover, &mut fake, 500.0, 500.0, move_keys());
        assert!(mover.click_down(move_keys()));
        at(&mut mover, &mut fake, 520.0, 500.0, move_keys());
        // A 1920 × 1080 display of unknown size gets a 3 × 2 grid, over its work area.
        assert_eq!(fake.grid, Some((Rect::new(0.0, 32.0, 1920.0, 1048.0), 3, 2)));
        assert_eq!(fake.dimmed_below, Some(1));
        let frame = fake.frame(1);
        assert_eq!((frame.min_x(), frame.min_y()), (0.0, 32.0));

        // Releasing it ends the move: the window stays where it snapped.
        assert!(mover.click_up());
        at(&mut mover, &mut fake, 900.0, 900.0, move_keys());
        assert_eq!(fake.frame(1), frame);
        assert_eq!((fake.grid, fake.dimmed_below), (None, None));
        // Until the keys are released and pressed again.
        at(&mut mover, &mut fake, 900.0, 900.0, Mods::NONE);
        at(&mut mover, &mut fake, 500.0, 500.0, move_keys());
        at(&mut mover, &mut fake, 600.0, 500.0, move_keys());
        assert_eq!(fake.frame(1).min_x(), frame.min_x() + 100.0);
    }

    #[test]
    fn a_click_without_the_keys_goes_to_the_app() {
        let (mut mover, _) = setup();
        assert!(!mover.click_down(Mods::NONE));
        assert!(!mover.click_up());
    }

    #[test]
    fn a_maximized_window_is_unmaximized_only_once_it_moves() {
        let (mut mover, mut fake) = setup();
        fake.info(1).maximized = true;
        at(&mut mover, &mut fake, 500.0, 500.0, move_keys());
        assert!(fake.unmaximized.is_empty());
        at(&mut mover, &mut fake, 510.0, 500.0, move_keys());
        assert_eq!(fake.unmaximized, [1]);
        // With its size, as the platform restores it to an older one.
        assert_eq!(fake.frame(1), Rect::new(210.0, 200.0, 800.0, 600.0));
    }

    #[test]
    fn windows_that_cant_move_are_left_alone() {
        let (mut mover, mut fake) = setup();
        fake.info(1).can_move = false;
        at(&mut mover, &mut fake, 500.0, 500.0, move_keys());
        at(&mut mover, &mut fake, 600.0, 500.0, move_keys());
        assert_eq!(fake.frame(1), Rect::new(200.0, 200.0, 800.0, 600.0));
    }

    #[test]
    fn an_arrow_sends_the_window_to_the_next_display_and_the_pointer_with_it() {
        let (mut mover, mut fake) = setup();
        at(&mut mover, &mut fake, 600.0, 500.0, move_keys());
        assert!(mover.arrow(&mut fake, move_keys(), Direction::Right, false));
        // Same relative place and size in the work area.
        let frame = fake.frame(1);
        assert_eq!(frame, transfer(&Rect::new(200.0, 200.0, 800.0, 600.0),
                                   &Rect::new(0.0, 32.0, 1920.0, 1048.0),
                                   &Rect::new(1920.0, 0.0, 2560.0, 1440.0), false));
        assert!(frame.contains(fake.pointer));
        // Moving carries on from there.
        let pointer = fake.pointer;
        at(&mut mover, &mut fake, pointer.x + 10.0, pointer.y, move_keys());
        assert_eq!(fake.frame(1).min_x(), frame.min_x() + 10.0);
        // There's nothing further right.
        assert!(mover.arrow(&mut fake, move_keys(), Direction::Right, false));
        assert_eq!(fake.frame(1).min_x(), frame.min_x() + 10.0);
        // Other keys don't send.
        assert!(!mover.arrow(&mut fake, resize_keys(), Direction::Left, false));
    }

    #[test]
    fn a_window_goes_back_where_its_app_was_on_a_display() {
        let (mut mover, mut fake) = setup();
        at(&mut mover, &mut fake, 600.0, 500.0, move_keys());
        mover.arrow(&mut fake, move_keys(), Direction::Right, false);
        let pointer = fake.pointer;
        at(&mut mover, &mut fake, pointer.x, pointer.y, Mods::NONE);
        // Somewhere else on the right display.
        fake.set_frame(&1, Rect::new(2000.0, 100.0, 1000.0, 1000.0));
        fake.pointer = Point::new(2100.0, 200.0);
        mover.arrow(&mut fake, move_keys(), Direction::Left, false);
        assert_eq!(fake.frame(1), Rect::new(200.0, 200.0, 800.0, 600.0));
    }

    #[test]
    fn a_wheel_spin_sends_once() {
        let (mut mover, mut fake) = setup();
        fake.pointer = Point::new(600.0, 500.0);
        assert!(mover.scroll(&mut fake, move_keys(), 0.0, 1.0, false, 10.0));
        let frame = fake.frame(1);
        assert!(frame.min_x() >= 1920.0);
        assert!(mover.scroll(&mut fake, move_keys(), 0.0, 1.0, false, 10.1));
        assert_eq!(fake.frame(1), frame);
        // Scrolling without the keys goes to the app.
        assert!(!mover.scroll(&mut fake, Mods::NONE, 0.0, 1.0, false, 20.0));
    }

    #[test]
    fn raising_is_optional() {
        let (mut mover, mut fake) = setup();
        at(&mut mover, &mut fake, 500.0, 500.0, move_keys());
        assert!(fake.raised.is_empty());
        at(&mut mover, &mut fake, 500.0, 500.0, Mods::NONE);
        mover.settings.raise_window = true;
        at(&mut mover, &mut fake, 500.0, 500.0, move_keys());
        assert_eq!(fake.raised, [1]);
    }

    #[test]
    fn disabled_does_nothing() {
        let (mut mover, mut fake) = setup();
        mover.settings.enabled = false;
        at(&mut mover, &mut fake, 500.0, 500.0, move_keys());
        at(&mut mover, &mut fake, 600.0, 500.0, move_keys());
        assert_eq!(fake.frame(1), Rect::new(200.0, 200.0, 800.0, 600.0));
        assert!(!mover.click_down(move_keys()));
    }
}
