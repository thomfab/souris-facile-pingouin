use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::geometry::{transfer, Rect};

// Where an app's windows last were on a display, with the display's work
// area at the time, to adapt the frame if the display changed since (a new
// resolution, a panel moved).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RememberedFrame {
    pub frame: Rect,
    pub area: Rect,
}

impl RememberedFrame {
    // The frame on the same display, whose work area is now `area`.
    pub fn restored(&self, area: &Rect, keep_size: bool) -> Rect {
        transfer(&self.frame, &self.area, area, keep_size)
    }
}

// The remembered frames, by display ID then by app (its WM_CLASS). X11
// gives windows no ID that survives an app restart, so they're per app.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WindowMemory {
    frames: BTreeMap<String, BTreeMap<String, RememberedFrame>>,
}

impl WindowMemory {
    // Nothing, or garbage, gives an empty memory.
    pub fn from_json(json: Option<&str>) -> WindowMemory {
        json.and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.frames).unwrap_or_default()
    }

    pub fn frame(&self, app: &str, display: &str) -> Option<RememberedFrame> {
        self.frames.get(display)?.get(app).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.values().all(BTreeMap::is_empty)
    }

    // How many apps have a frame remembered on the display.
    pub fn count(&self, display: &str) -> usize {
        self.frames.get(display).map_or(0, BTreeMap::len)
    }

    pub fn displays(&self) -> impl Iterator<Item = &String> {
        self.frames.iter().filter(|(_, apps)| !apps.is_empty()).map(|(display, _)| display)
    }

    pub fn forget(&mut self, display: &str) {
        self.frames.remove(display);
    }

    pub fn remember(&mut self, frame: Rect, area: Rect, app: &str, display: &str) {
        self.frames
            .entry(display.to_string())
            .or_default()
            .insert(app.to_string(), RememberedFrame { frame, area });
    }
}

// Where the memory is kept: a file, read again each time as another
// instance (or the user) may have changed it, or just memory for tests.
pub enum MemoryStore {
    File(PathBuf),
    #[cfg(test)]
    InMemory(WindowMemory),
}

impl MemoryStore {
    pub fn load(&self) -> WindowMemory {
        match self {
            MemoryStore::File(path) => WindowMemory::from_json(std::fs::read_to_string(path).ok().as_deref()),
            #[cfg(test)]
            MemoryStore::InMemory(memory) => memory.clone(),
        }
    }

    pub fn save(&mut self, memory: WindowMemory) {
        match self {
            MemoryStore::File(path) => {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(&*path, memory.to_json()) {
                    eprintln!("Can't save window sizes to {}: {e}", path.display());
                }
            }
            #[cfg(test)]
            MemoryStore::InMemory(stored) => *stored = memory,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: &str = "laptop-id";
    const OFFICE: &str = "office-id";
    const AREA: Rect = Rect { x: 0.0, y: 32.0, width: 1512.0, height: 900.0 };

    #[test]
    fn remembers_per_app_and_display() {
        let mut memory = WindowMemory::default();
        let frame = Rect::new(0.0, 32.0, 756.0, 900.0);
        memory.remember(frame, AREA, "firefox", LAPTOP);
        assert_eq!(memory.frame("firefox", LAPTOP), Some(RememberedFrame { frame, area: AREA }));
        assert_eq!(memory.frame("firefox", OFFICE), None);
        assert_eq!(memory.frame("thunderbird", LAPTOP), None);
    }

    #[test]
    fn the_last_frame_wins() {
        let mut memory = WindowMemory::default();
        memory.remember(Rect::new(0.0, 32.0, 100.0, 100.0), AREA, "app", LAPTOP);
        memory.remember(Rect::new(50.0, 75.0, 200.0, 200.0), AREA, "app", LAPTOP);
        assert_eq!(memory.frame("app", LAPTOP).unwrap().frame, Rect::new(50.0, 75.0, 200.0, 200.0));
    }

    #[test]
    fn encoding_round_trips() {
        let mut memory = WindowMemory::default();
        memory.remember(Rect::new(10.0, 35.0, 700.0, 500.0), AREA, "app", OFFICE);
        assert_eq!(WindowMemory::from_json(Some(&memory.to_json())), memory);
        assert_eq!(WindowMemory::from_json(None), WindowMemory::default());
        assert_eq!(WindowMemory::from_json(Some("garbage")), WindowMemory::default());
    }

    #[test]
    fn restoring_on_an_unchanged_display_gives_the_same_frame() {
        let remembered = RememberedFrame { frame: Rect::new(100.0, 125.0, 700.0, 500.0), area: AREA };
        assert_eq!(remembered.restored(&AREA, false), remembered.frame);
    }

    #[test]
    fn restoring_follows_a_changed_display() {
        // The panel went from the top (32 px high) to the left (76 px wide).
        let remembered = RememberedFrame { frame: Rect::new(0.0, 32.0, 756.0, 900.0), area: AREA };
        let now = Rect::new(76.0, 0.0, 1436.0, 932.0);
        let restored = remembered.restored(&now, false);
        assert_eq!(restored.min_x(), 76.0);
        assert_eq!(restored.width, 718.0);
        assert_eq!(restored.height, 932.0);
    }
}
