use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::geometry::Size;
use crate::platform::Display;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Shift,
    Control,
    Alt,
    Super,
}

impl Modifier {
    pub const ALL: [Modifier; 4] = [Modifier::Shift, Modifier::Control, Modifier::Alt, Modifier::Super];

    pub fn name(self) -> &'static str {
        match self {
            Modifier::Shift => "shift",
            Modifier::Control => "control",
            Modifier::Alt => "alt",
            Modifier::Super => "super",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Modifier::Shift => "Shift",
            Modifier::Control => "Ctrl",
            Modifier::Alt => "Alt",
            Modifier::Super => "Super",
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }

    pub fn from_name(name: &str) -> Option<Modifier> {
        Modifier::ALL.into_iter().find(|m| m.name() == name)
    }
}

// A set of held modifier keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);

    pub fn with(self, modifier: Modifier) -> Mods {
        Mods(self.0 | modifier.bit())
    }

    pub fn contains(self, modifier: Modifier) -> bool {
        self.0 & modifier.bit() != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    pub fn iter(self) -> impl Iterator<Item = Modifier> {
        Modifier::ALL.into_iter().filter(move |&m| self.contains(m))
    }

    // Unknown names are ignored.
    pub fn parse<S: AsRef<str>>(names: &[S]) -> Mods {
        names
            .iter()
            .filter_map(|n| Modifier::from_name(n.as_ref()))
            .fold(Mods::NONE, Mods::with)
    }

    pub fn label(self) -> String {
        self.iter().map(Modifier::label).collect::<Vec<_>>().join(" + ")
    }
}

// Problems with the move and resize combinations, to warn about.
pub fn warnings(move_names: &[String], resize_names: &[String]) -> Vec<String> {
    let mut warnings = Vec::new();
    for name in move_names.iter().chain(resize_names) {
        if Modifier::from_name(name).is_none() {
            warnings.push(format!("Unknown key \"{name}\": use shift, control, alt or super."));
        }
    }
    let (move_mods, resize_mods) = (Mods::parse(move_names), Mods::parse(resize_names));
    if !move_mods.is_empty() && move_mods == resize_mods {
        warnings.push("Move and Resize use the same keys, so only resizing works.".into());
    }
    for (name, mods) in [("Move", move_mods), ("Resize", resize_mods)] {
        if mods.len() == 1 {
            warnings.push(format!(
                "{name} uses {} alone, which is held for ordinary shortcuts and typing. Add another key.",
                mods.label()
            ));
        }
    }
    warnings
}

// What the held modifier keys do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    None,
    Move,
    Resize,
}

pub const GRID_RANGE: std::ops::RangeInclusive<usize> = 1..=48;

pub struct GridPreset {
    pub name: &'static str,
    pub columns: usize,
    pub rows: usize,
}

pub const GRID_PRESETS: [GridPreset; 6] = [
    GridPreset { name: "Laptop 13–14″", columns: 2, rows: 2 },
    GridPreset { name: "Laptop 15–16″", columns: 3, rows: 2 },
    GridPreset { name: "24–27″", columns: 4, rows: 3 },
    GridPreset { name: "Ultrawide", columns: 6, rows: 2 },
    GridPreset { name: "Ultrawide, fine", columns: 8, rows: 4 },
    GridPreset { name: "Portrait", columns: 2, rows: 3 },
];

// A grid that suits a display, for displays whose grid was never set. X11
// has no per-display scale, so the physical size decides when it's known.
pub fn sensible_grid(pixels: Size, mm: Size) -> (usize, usize) {
    if pixels.width <= 0.0 || pixels.height <= 0.0 {
        return (4, 3);
    }
    if pixels.height > pixels.width {
        return (2, 3);
    }
    if pixels.width / pixels.height >= 2.1 {
        return (8, 4); // Ultrawide
    }
    let inches = mm.width.hypot(mm.height) / 25.4;
    // Smaller sizes are made up (projectors, TVs reporting 16 × 9 mm).
    if inches >= 10.0 {
        return if inches < 15.0 {
            (2, 2) // 13–14″ laptops
        } else if inches < 19.0 {
            (3, 2) // 15–17″ laptops
        } else {
            (4, 3)
        };
    }
    if pixels.width < 1600.0 {
        (2, 2)
    } else if pixels.width < 2000.0 {
        (3, 2)
    } else {
        (4, 3)
    }
}

// The grid of one display, remembered by the display's ID so it applies
// again whenever that display is plugged back in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayGrid {
    // The display's name, for whoever edits the settings.
    #[serde(default)]
    pub name: String,
    pub columns: usize,
    pub rows: usize,
}

fn clamp_grid(count: usize) -> usize {
    count.clamp(*GRID_RANGE.start(), *GRID_RANGE.end())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Settings {
    pub enabled: bool,
    pub move_modifiers: Vec<String>,
    pub resize_modifiers: Vec<String>,
    pub raise_window: bool,
    pub resize_nearest_corner: bool,
    pub snap_threshold: f64,
    pub snap_to_windows: bool,
    pub show_grid: bool,
    pub dim_windows: bool,
    pub send_with_scroll: bool,
    pub send_with_arrows: bool,
    pub remember_window_frames: bool,
    pub displays: BTreeMap<String, DisplayGrid>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            move_modifiers: vec!["control".into(), "alt".into()],
            resize_modifiers: vec!["control".into(), "alt".into(), "super".into()],
            raise_window: false,
            resize_nearest_corner: true,
            snap_threshold: 16.0,
            snap_to_windows: true,
            show_grid: true,
            dim_windows: true,
            send_with_scroll: true,
            send_with_arrows: true,
            remember_window_frames: true,
            displays: BTreeMap::new(),
        }
    }
}

// Written on first launch, so the settings are easy to find and change.
pub const DEFAULT_SETTINGS: &str = r#"# Souris facile pingouin settings. They're read again whenever this file
# changes, no need to restart.

enabled = true

# Hold these keys and move the mouse to move the window under the pointer.
# Any of: shift, control, alt, super. The combination must match exactly,
# and an empty list turns it off.
move-modifiers = ["control", "alt"]
# Hold these to resize it instead.
resize-modifiers = ["control", "alt", "super"]

# Raise the window when it starts being moved or resized.
raise-window = false
# Pull the corner nearest the pointer, or else always the bottom-right one.
resize-nearest-corner = true

# Window edges snap to display edges, and to other windows', within this
# many pixels. 0 turns snapping off.
snap-threshold = 16
snap-to-windows = true

# While you hold the click, edges land on the grid of the display.
show-grid = true
# Dim the other windows meanwhile (needs a compositor).
dim-windows = true

# While holding the move keys, scroll or swipe (or press an arrow key) to
# send the window to another display.
send-with-scroll = true
send-with-arrows = true
# Remember where each app's windows were on each display.
remember-window-frames = true

# Each display gets a grid suiting its size. To choose one, run
# `souris-facile-pingouin displays` and paste a section like this one:
#
# [displays."DEL-A0FE-12345678"]
# name = "DELL U3421WE"
# columns = 6
# rows = 2
"#;

impl Settings {
    pub fn move_mods(&self) -> Mods {
        Mods::parse(&self.move_modifiers)
    }

    pub fn resize_mods(&self) -> Mods {
        Mods::parse(&self.resize_modifiers)
    }

    // `mods` must match a combination exactly.
    pub fn action(&self, mods: Mods) -> KeyAction {
        let (move_mods, resize_mods) = (self.move_mods(), self.resize_mods());
        if !resize_mods.is_empty() && mods == resize_mods {
            KeyAction::Resize
        } else if !move_mods.is_empty() && mods == move_mods {
            KeyAction::Move
        } else {
            KeyAction::None
        }
    }

    // The grid of a display, which suits its size if it was never set.
    pub fn grid(&self, display: &Display) -> (usize, usize) {
        match self.displays.get(&display.id) {
            Some(grid) => (clamp_grid(grid.columns), clamp_grid(grid.rows)),
            None => sensible_grid(display.frame.size(), display.size_mm),
        }
    }

    pub fn parse(text: &str) -> Result<Settings, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    // Writes the default settings if there's no file yet.
    pub fn load(path: &Path) -> Result<Settings, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Settings::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(path, DEFAULT_SETTINGS);
                Ok(Settings::default())
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(variable) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback),
    }
}

pub fn settings_path() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config").join("souris-facile-pingouin/settings.toml")
}

pub fn window_frames_path() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state").join("souris-facile-pingouin/window-frames.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Rect;

    fn names(list: &str) -> Vec<String> {
        list.split(',').filter(|s| !s.is_empty()).map(String::from).collect()
    }

    fn mods(list: &str) -> Mods {
        Mods::parse(&names(list))
    }

    fn settings(move_keys: &str, resize_keys: &str) -> Settings {
        Settings { move_modifiers: names(move_keys), resize_modifiers: names(resize_keys), ..Settings::default() }
    }

    #[test]
    fn parse_ignores_unknown_names() {
        assert_eq!(mods("control,bogus,alt"), mods("alt,control"));
        assert_eq!(mods(""), Mods::NONE);
    }

    #[test]
    fn labels_use_a_stable_order() {
        assert_eq!(mods("super,control,alt").label(), "Ctrl + Alt + Super");
    }

    #[test]
    fn defaults_are_control_alt_to_move() {
        let s = Settings::default();
        assert_eq!(s.move_mods(), mods("control,alt"));
        assert_eq!(s.resize_mods(), mods("control,alt,super"));
        assert!(warnings(&s.move_modifiers, &s.resize_modifiers).is_empty());
    }

    #[test]
    fn warns_about_identical_combinations() {
        assert_eq!(warnings(&names("control,alt"), &names("alt,control")).len(), 1);
        // Both turned off is a way to disable the feature, not a conflict.
        assert!(warnings(&[], &[]).is_empty());
    }

    #[test]
    fn warns_about_a_single_key_and_unknown_names() {
        assert_eq!(warnings(&names("super"), &names("control,alt,super")).len(), 1);
        assert_eq!(warnings(&names("shift"), &names("shift")).len(), 3);
        assert_eq!(warnings(&names("control,option"), &names("control,alt,super")).len(), 2);
    }

    #[test]
    fn actions_with_the_defaults() {
        let s = Settings::default();
        assert_eq!(s.action(mods("control,alt")), KeyAction::Move);
        assert_eq!(s.action(mods("control,alt,super")), KeyAction::Resize);
        assert_eq!(s.action(mods("control,alt,shift")), KeyAction::None);
        assert_eq!(s.action(mods("alt")), KeyAction::None);
        assert_eq!(s.action(Mods::NONE), KeyAction::None);
    }

    #[test]
    fn turned_off_combinations_never_match() {
        assert_eq!(settings("", "").action(Mods::NONE), KeyAction::None);
        assert_eq!(settings("control,alt", "").action(mods("control,alt")), KeyAction::Move);
    }

    #[test]
    fn the_default_file_gives_the_default_settings() {
        assert_eq!(Settings::parse(DEFAULT_SETTINGS), Ok(Settings::default()));
    }

    #[test]
    fn missing_settings_take_their_default() {
        let s = Settings::parse("raise-window = true").unwrap();
        assert!(s.raise_window);
        assert_eq!(s.move_modifiers, Settings::default().move_modifiers);
        assert!(Settings::parse("raise-window = 3").is_err());
    }

    const OFFICE: &str = "DEL-A0FE-12345678";

    fn display(id: &str, width: f64, height: f64, mm: (f64, f64)) -> Display {
        let frame = Rect::new(0.0, 0.0, width, height);
        Display { id: id.into(), name: String::new(), frame, work_area: frame, size_mm: Size::new(mm.0, mm.1) }
    }

    #[test]
    fn a_display_uses_its_own_grid() {
        let s = Settings::parse(&format!("[displays.\"{OFFICE}\"]\nname = \"DELL\"\ncolumns = 6\nrows = 2")).unwrap();
        assert_eq!(s.grid(&display(OFFICE, 3440.0, 1440.0, (800.0, 335.0))), (6, 2));
        // Others get a grid suiting their size.
        assert_eq!(s.grid(&display("another", 3440.0, 1440.0, (800.0, 335.0))), (8, 4));
    }

    #[test]
    fn stored_sizes_are_clamped() {
        let s = Settings::parse(&format!("[displays.\"{OFFICE}\"]\ncolumns = 0\nrows = 500")).unwrap();
        assert_eq!(s.grid(&display(OFFICE, 3440.0, 1440.0, (0.0, 0.0))), (1, 48));
    }

    fn sensible(width: f64, height: f64, mm: (f64, f64)) -> (usize, usize) {
        sensible_grid(Size::new(width, height), Size::new(mm.0, mm.1))
    }

    #[test]
    fn sensible_grids_follow_the_physical_size() {
        assert_eq!(sensible(2880.0, 1800.0, (286.0, 179.0)), (2, 2)); // 13.3″
        assert_eq!(sensible(1920.0, 1200.0, (302.0, 189.0)), (2, 2)); // 14″
        assert_eq!(sensible(2560.0, 1600.0, (345.0, 215.0)), (3, 2)); // 16″
        assert_eq!(sensible(1920.0, 1080.0, (527.0, 296.0)), (4, 3)); // 24″
        assert_eq!(sensible(3840.0, 2160.0, (597.0, 336.0)), (4, 3)); // 27″
        assert_eq!(sensible(3440.0, 1440.0, (800.0, 335.0)), (8, 4)); // 21:9
        assert_eq!(sensible(5120.0, 1440.0, (1190.0, 335.0)), (8, 4)); // 32:9
        assert_eq!(sensible(1440.0, 2560.0, (335.0, 597.0)), (2, 3)); // Portrait
    }

    #[test]
    fn sensible_grids_follow_the_pixels_without_a_physical_size() {
        assert_eq!(sensible(1512.0, 982.0, (0.0, 0.0)), (2, 2));
        assert_eq!(sensible(1920.0, 1080.0, (160.0, 90.0)), (3, 2)); // A made-up size
        assert_eq!(sensible(2560.0, 1440.0, (0.0, 0.0)), (4, 3));
        assert_eq!(sensible(0.0, 0.0, (0.0, 0.0)), (4, 3));
    }

    #[test]
    fn sensible_grids_are_among_the_presets() {
        for (w, h, mm) in [(1512.0, 982.0, (0.0, 0.0)), (1728.0, 1117.0, (0.0, 0.0)), (2560.0, 1440.0, (597.0, 336.0)),
                           (3440.0, 1440.0, (0.0, 0.0)), (1440.0, 2560.0, (0.0, 0.0))] {
            let grid = sensible(w, h, mm);
            assert!(GRID_PRESETS.iter().any(|p| (p.columns, p.rows) == grid));
        }
    }
}
