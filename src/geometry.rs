use serde::{Deserialize, Serialize};

// Everything works in global screen coordinates: origin at the top-left of
// the whole screen area, y going down, in pixels.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub fn new(width: f64, height: f64) -> Self {
        Size { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Rect { x, y, width, height }
    }

    pub fn from_origin_size(origin: Point, size: Size) -> Self {
        Rect::new(origin.x, origin.y, size.width, size.height)
    }

    pub fn min_x(&self) -> f64 {
        self.x
    }
    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }
    pub fn mid_x(&self) -> f64 {
        self.x + self.width / 2.0
    }
    pub fn min_y(&self) -> f64 {
        self.y
    }
    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }
    pub fn mid_y(&self) -> f64 {
        self.y + self.height / 2.0
    }
    pub fn origin(&self) -> Point {
        Point::new(self.x, self.y)
    }
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
    pub fn center(&self) -> Point {
        Point::new(self.mid_x(), self.mid_y())
    }

    // Like CGRect: the left and top edges are inside, the others aren't.
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.min_x() && p.x < self.max_x() && p.y >= self.min_y() && p.y < self.max_y()
    }

    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x = self.min_x().max(other.min_x());
        let y = self.min_y().max(other.min_y());
        let max_x = self.max_x().min(other.max_x());
        let max_y = self.max_y().min(other.max_y());
        (max_x > x && max_y > y).then(|| Rect::new(x, y, max_x - x, max_y - y))
    }
}

// A direction on screen (y going down).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    // The main direction of a movement.
    pub fn from_delta(dx: f64, dy: f64) -> Self {
        if dx.abs() >= dy.abs() {
            if dx > 0.0 { Direction::Right } else { Direction::Left }
        } else if dy > 0.0 {
            Direction::Down
        } else {
            Direction::Up
        }
    }

    pub fn vector(self) -> Point {
        match self {
            Direction::Left => Point::new(-1.0, 0.0),
            Direction::Right => Point::new(1.0, 0.0),
            Direction::Up => Point::new(0.0, -1.0),
            Direction::Down => Point::new(0.0, 1.0),
        }
    }
}

// Evenly spaced positions from start to start + length, both included.
pub fn divide(start: f64, length: f64, count: usize) -> Vec<f64> {
    (0..=count)
        .map(|i| (start + i as f64 * length / count as f64).round())
        .collect()
}

// How far apart two frames are: the largest difference of any edge or size.
pub fn frame_distance(a: &Rect, b: &Rect) -> f64 {
    (a.min_x() - b.min_x())
        .abs()
        .max((a.min_y() - b.min_y()).abs())
        .max((a.width - b.width).abs())
        .max((a.height - b.height).abs())
}

// The shift that brings the closest of `edges` onto one of `lines`,
// or 0 if none is within `threshold`.
pub fn snap_delta(edges: &[f64], lines: &[f64], threshold: f64) -> f64 {
    let mut best = 0.0;
    let mut best_dist = threshold;
    for &edge in edges {
        for &line in lines {
            let dist = (line - edge).abs();
            if dist <= best_dist {
                best = line - edge;
                best_dist = dist;
            }
        }
    }
    best
}

// The display in `direction` from display `source`, among display frames:
// the closest one that way, preferring those lined up with it.
pub fn display_toward(direction: Direction, source: usize, displays: &[Rect]) -> Option<usize> {
    let from = displays[source];
    let v = direction.vector();
    let mut best = None;
    let mut best_score = f64::INFINITY;
    for (index, frame) in displays.iter().enumerate() {
        if index == source {
            continue;
        }
        let dx = frame.mid_x() - from.mid_x();
        let dy = frame.mid_y() - from.mid_y();
        let along = dx * v.x + dy * v.y;
        if along <= 0.0 {
            continue;
        }
        let across = (dx * v.y - dy * v.x).abs();
        let score = along + 2.0 * across;
        if score < best_score {
            best = Some(index);
            best_score = score;
        }
    }
    best
}

// The display `step` places after display `index`, going left to right
// (then top to bottom) and wrapping around.
pub fn display_after(index: usize, step: i32, displays: &[Rect]) -> Option<usize> {
    if displays.len() < 2 {
        return None;
    }
    let mut order: Vec<usize> = (0..displays.len()).collect();
    order.sort_by(|&a, &b| {
        (displays[a].min_x(), displays[a].min_y())
            .partial_cmp(&(displays[b].min_x(), displays[b].min_y()))
            .unwrap()
    });
    let position = order.iter().position(|&i| i == index)? as i32;
    let count = order.len() as i32;
    Some(order[((position + step) % count + count) as usize % count as usize])
}

// Where `frame` lands when it goes from area `from` to area `to` (work
// areas): same relative place and size, fitted inside `to`. With
// `keep_size`, for windows that can't be resized, only the place changes.
pub fn transfer(frame: &Rect, from: &Rect, to: &Rect, keep_size: bool) -> Rect {
    let sx = to.width / from.width;
    let sy = to.height / from.height;
    let (mut width, mut height) = (frame.width, frame.height);
    if !keep_size {
        width = (width * sx).min(to.width);
        height = (height * sy).min(to.height);
    }
    // The center keeps its relative place.
    let mut x = to.min_x() + (frame.mid_x() - from.min_x()) * sx - width / 2.0;
    let mut y = to.min_y() + (frame.mid_y() - from.min_y()) * sy - height / 2.0;
    x = x.min(to.max_x() - width).max(to.min_x());
    y = y.min(to.max_y() - height).max(to.min_y());
    Rect::new(x.round(), y.round(), width.round(), height.round())
}

// Moves each edge of `frame` to the nearest line of the grid over `area`,
// keeping it at least one cell wide and tall.
pub fn fit_to_grid(frame: &Rect, area: &Rect, columns: usize, rows: usize) -> Rect {
    fn nearest(value: f64, lines: &[f64]) -> usize {
        let mut best = 0;
        for i in 1..lines.len() {
            if (lines[i] - value).abs() < (lines[best] - value).abs() {
                best = i;
            }
        }
        best
    }
    fn span(low: f64, high: f64, lines: &[f64]) -> (f64, f64) {
        let (mut first, mut last) = (nearest(low, lines), nearest(high, lines));
        if first == last {
            if last < lines.len() - 1 {
                last += 1;
            } else {
                first -= 1;
            }
        }
        (lines[first], lines[last])
    }
    let (left, right) = span(frame.min_x(), frame.max_x(), &divide(area.min_x(), area.width, columns));
    let (top, bottom) = span(frame.min_y(), frame.max_y(), &divide(area.min_y(), area.height, rows));
    Rect::new(left, top, right - left, bottom - top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divide_includes_both_ends() {
        assert_eq!(divide(0.0, 100.0, 4), [0.0, 25.0, 50.0, 75.0, 100.0]);
        assert_eq!(divide(10.0, 90.0, 1), [10.0, 100.0]);
    }

    #[test]
    fn divide_rounds_to_whole_pixels() {
        assert_eq!(divide(0.0, 100.0, 3), [0.0, 33.0, 67.0, 100.0]);
    }

    #[test]
    fn snap_delta_picks_the_closest_line() {
        assert_eq!(snap_delta(&[10.0, 110.0], &[0.0, 200.0], 16.0), -10.0);
        assert_eq!(snap_delta(&[10.0, 195.0], &[0.0, 200.0], 16.0), 5.0);
    }

    #[test]
    fn snap_delta_ignores_lines_beyond_the_threshold() {
        assert_eq!(snap_delta(&[50.0], &[0.0, 100.0], 16.0), 0.0);
        assert_eq!(snap_delta(&[16.0], &[0.0], 16.0), -16.0);
    }

    #[test]
    fn frame_distance_is_the_largest_difference() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(frame_distance(&a, &a), 0.0);
        assert_eq!(frame_distance(&a, &Rect::new(1.0, -3.0, 102.0, 100.0)), 3.0);
    }

    // A laptop, a wide monitor to its right, and a monitor above it.
    const LAPTOP: Rect = Rect { x: 0.0, y: 0.0, width: 1512.0, height: 982.0 };
    const WIDE: Rect = Rect { x: 1512.0, y: -400.0, width: 3440.0, height: 1440.0 };
    const ABOVE: Rect = Rect { x: 0.0, y: -1080.0, width: 1512.0, height: 1080.0 };
    const DISPLAYS: [Rect; 3] = [LAPTOP, WIDE, ABOVE];

    #[test]
    fn direction_follows_the_main_axis() {
        assert_eq!(Direction::from_delta(50.0, -20.0), Direction::Right);
        assert_eq!(Direction::from_delta(-10.0, 40.0), Direction::Down);
    }

    #[test]
    fn display_toward_a_direction() {
        assert_eq!(display_toward(Direction::Right, 0, &DISPLAYS), Some(1));
        assert_eq!(display_toward(Direction::Up, 0, &DISPLAYS), Some(2));
        assert_eq!(display_toward(Direction::Left, 0, &DISPLAYS), None);
        assert_eq!(display_toward(Direction::Left, 1, &DISPLAYS), Some(0));
        assert_eq!(display_toward(Direction::Down, 2, &DISPLAYS), Some(0));
    }

    #[test]
    fn next_and_previous_displays_wrap_around() {
        // Left to right, then top to bottom: above, laptop, wide.
        assert_eq!(display_after(0, 1, &DISPLAYS), Some(1));
        assert_eq!(display_after(1, 1, &DISPLAYS), Some(2));
        assert_eq!(display_after(0, -1, &DISPLAYS), Some(2));
        assert_eq!(display_after(0, 1, &[LAPTOP]), None);
    }

    #[test]
    fn transfer_keeps_the_relative_place_and_size() {
        let from = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let to = Rect::new(1000.0, 0.0, 2000.0, 1000.0);
        let left_half = Rect::new(0.0, 0.0, 500.0, 800.0);
        assert_eq!(transfer(&left_half, &from, &to, false), Rect::new(1000.0, 0.0, 1000.0, 1000.0));
    }

    #[test]
    fn transfer_can_keep_the_size() {
        let from = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let to = Rect::new(1000.0, 0.0, 2000.0, 1000.0);
        assert_eq!(
            transfer(&Rect::new(100.0, 100.0, 300.0, 200.0), &from, &to, true),
            Rect::new(1350.0, 150.0, 300.0, 200.0)
        );
    }

    #[test]
    fn transfer_keeps_the_top_left_of_a_window_too_big_to_fit() {
        let from = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let small = Rect::new(1000.0, 0.0, 500.0, 400.0);
        assert_eq!(transfer(&from, &from, &small, true), Rect::new(1000.0, 0.0, 1000.0, 800.0));
    }

    #[test]
    fn fit_to_grid_moves_edges_to_the_nearest_lines() {
        let area = Rect::new(0.0, 0.0, 1200.0, 900.0);
        assert_eq!(
            fit_to_grid(&Rect::new(290.0, 10.0, 320.0, 580.0), &area, 4, 3),
            Rect::new(300.0, 0.0, 300.0, 600.0)
        );
    }

    #[test]
    fn fit_to_grid_keeps_at_least_one_cell() {
        let area = Rect::new(0.0, 0.0, 1200.0, 900.0);
        assert_eq!(
            fit_to_grid(&Rect::new(310.0, 310.0, 20.0, 20.0), &area, 4, 3),
            Rect::new(300.0, 300.0, 300.0, 300.0)
        );
        assert_eq!(
            fit_to_grid(&Rect::new(1190.0, 890.0, 5.0, 5.0), &area, 4, 3),
            Rect::new(900.0, 600.0, 300.0, 300.0)
        );
    }

    #[test]
    fn intersection() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(a.intersection(&Rect::new(50.0, 50.0, 100.0, 100.0)), Some(Rect::new(50.0, 50.0, 50.0, 50.0)));
        assert_eq!(a.intersection(&Rect::new(100.0, 0.0, 10.0, 10.0)), None);
    }
}
