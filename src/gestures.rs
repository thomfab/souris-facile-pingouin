use crate::geometry::Direction;

// The phases of a touchpad scroll, for devices that report them.
pub mod scroll_phase {
    pub const NONE: i64 = 0;
    pub const BEGAN: i64 = 1;
    pub const ENDED: i64 = 4;
    pub const CANCELLED: i64 = 8;
    pub const MAY_BEGIN: i64 = 128;
}

// Turns a swipe, a stream of small scroll events from a touchpad, into a
// single direction once the fingers have travelled far enough.
#[derive(Debug)]
pub struct SwipeTracker {
    dx: f64,
    dy: f64,
    fired: bool,
    last_time: f64,
}

impl Default for SwipeTracker {
    fn default() -> Self {
        SwipeTracker { dx: 0.0, dy: 0.0, fired: false, last_time: f64::NEG_INFINITY }
    }
}

impl SwipeTracker {
    pub const DISTANCE: f64 = 60.0;
    // Events further apart than this (in seconds) belong to separate
    // swipes, for devices that don't report phases.
    pub const PAUSE: f64 = 0.3;

    // `dx` and `dy` follow the fingers. Returns a direction at most once per swipe.
    pub fn feed(&mut self, phase: i64, momentum: bool, dx: f64, dy: f64, time: f64) -> Option<Direction> {
        // The inertia after the fingers lift isn't part of the swipe.
        if momentum {
            return None;
        }
        if phase == scroll_phase::ENDED || phase == scroll_phase::CANCELLED {
            self.reset();
            return None;
        }
        if phase == scroll_phase::BEGAN || phase == scroll_phase::MAY_BEGIN || time - self.last_time > Self::PAUSE {
            self.reset();
        }
        self.last_time = time;

        if self.fired {
            return None;
        }
        self.dx += dx;
        self.dy += dy;
        if self.dx.hypot(self.dy) < Self::DISTANCE {
            return None;
        }
        self.fired = true;
        Some(Direction::from_delta(self.dx, self.dy))
    }

    pub fn reset(&mut self) {
        *self = SwipeTracker::default();
    }
}

#[cfg(test)]
mod tests {
    use super::scroll_phase::*;
    use super::*;

    #[test]
    fn a_swipe_gives_one_direction_once_far_enough() {
        let mut swipe = SwipeTracker::default();
        assert_eq!(swipe.feed(BEGAN, false, 5.0, 0.0, 0.0), None);
        assert_eq!(swipe.feed(2, false, 30.0, 10.0, 0.01), None);
        assert_eq!(swipe.feed(2, false, 40.0, 0.0, 0.02), Some(Direction::Right));
        // The rest of the same swipe does nothing.
        assert_eq!(swipe.feed(2, false, 100.0, 0.0, 0.03), None);
        assert_eq!(swipe.feed(ENDED, false, 0.0, 0.0, 0.04), None);
        // Nor does the inertia after it.
        assert_eq!(swipe.feed(NONE, true, 100.0, 0.0, 0.05), None);
        // A new swipe counts again.
        assert_eq!(swipe.feed(BEGAN, false, 0.0, -30.0, 1.0), None);
        assert_eq!(swipe.feed(2, false, 0.0, -40.0, 1.01), Some(Direction::Up));
    }

    #[test]
    fn swipes_without_phases_are_separated_by_a_pause() {
        let mut swipe = SwipeTracker::default();
        assert_eq!(swipe.feed(NONE, false, -70.0, 0.0, 0.0), Some(Direction::Left));
        assert_eq!(swipe.feed(NONE, false, -70.0, 0.0, 0.1), None);
        assert_eq!(swipe.feed(NONE, false, -70.0, 0.0, 1.0), Some(Direction::Left));
    }
}
